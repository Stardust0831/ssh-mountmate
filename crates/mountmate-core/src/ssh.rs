use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use glob::glob;
use ring::hmac;
use thiserror::Error;
use wait_timeout::ChildExt;

use crate::paths::AppPaths;
use crate::storage::{FileLock, StorageError, atomic_write};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn known_hosts_marker(host: &str, port: &str) -> String {
    if port.trim().is_empty() || port == "22" {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

pub fn known_hosts_line_matches(line: &str, marker: &str) -> bool {
    let mut parts = line.split_whitespace();
    let first = parts.next();
    let hosts = if first.is_some_and(|value| value.starts_with('@')) {
        parts.next()
    } else {
        first
    };
    hosts.is_some_and(|hosts| hosts.split(',').any(|host| host == marker))
}

#[derive(Debug, Error)]
pub enum SshError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid SSH Include pattern {pattern:?}: {message}")]
    IncludePattern { pattern: String, message: String },
    #[error("SSH command failed: {0}")]
    Command(String),
    #[error("invalid SSH host alias: {0}")]
    InvalidHost(String),
    #[error("invalid SSH port: {0}")]
    InvalidPort(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("SSH permission update failed for {path}: {message}")]
    Permissions { path: PathBuf, message: String },
    #[error("invalid private key file: {0}")]
    InvalidPrivateKey(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHostEntry {
    pub host: String,
    pub path: PathBuf,
    pub line: usize,
    pub raw: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedSshConfig {
    values: HashMap<String, Vec<String>>,
}

impl ResolvedSshConfig {
    pub fn parse(output: &str) -> Self {
        let mut values: HashMap<String, Vec<String>> = HashMap::new();
        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once(char::is_whitespace) {
                values
                    .entry(key.to_ascii_lowercase())
                    .or_default()
                    .push(value.trim().to_owned());
            }
        }
        Self { values }
    }

    pub fn first<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.values
            .get(&key.to_ascii_lowercase())
            .and_then(|values| values.first())
            .map_or(default, String::as_str)
    }

    pub fn all(&self, key: &str) -> &[String] {
        self.values
            .get(&key.to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    pub fn first_existing_path(&self, key: &str) -> Option<PathBuf> {
        self.all(key)
            .iter()
            .map(|value| expand_home(value.trim().trim_matches('"')))
            .find(|path| path.is_file())
    }

    pub fn needs_openssh_transport(&self) -> bool {
        !matches!(
            self.first("proxyjump", "none")
                .to_ascii_lowercase()
                .as_str(),
            "" | "none"
        ) || !matches!(
            self.first("proxycommand", "none")
                .to_ascii_lowercase()
                .as_str(),
            "" | "none"
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestedTransport {
    Auto,
    Native,
    Openssh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshTransport {
    Native,
    Openssh,
}

pub fn choose_transport(
    requested: RequestedTransport,
    config: &ResolvedSshConfig,
    windows: bool,
) -> SshTransport {
    match requested {
        RequestedTransport::Native => SshTransport::Native,
        RequestedTransport::Openssh => SshTransport::Openssh,
        RequestedTransport::Auto if windows && !config.needs_openssh_transport() => {
            SshTransport::Native
        }
        RequestedTransport::Auto => SshTransport::Openssh,
    }
}

pub fn validate_host_alias(host: &str) -> Result<(), SshError> {
    let valid = !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':'));
    if valid {
        Ok(())
    } else {
        Err(SshError::InvalidHost(host.to_owned()))
    }
}

pub fn resolve_ssh_config(
    ssh: &Path,
    host: &str,
    config_path: Option<&Path>,
) -> Result<ResolvedSshConfig, SshError> {
    validate_host_alias(host)?;
    let mut command = Command::new(ssh);
    if let Some(config_path) = config_path {
        command.arg("-F").arg(config_path);
    }
    command.arg("-G").arg(host);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = command.output().map_err(|source| SshError::Io {
        path: ssh.to_owned(),
        source,
    })?;
    if !output.status.success() {
        return Err(SshError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(ResolvedSshConfig::parse(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

pub fn list_ssh_config_hosts(config_path: &Path) -> Result<Vec<SshHostEntry>, SshError> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    visit_ssh_config(config_path, &mut seen, &mut entries)?;
    Ok(entries)
}

pub fn default_ssh_config_path() -> PathBuf {
    directories::BaseDirs::new()
        .map(|directories| directories.home_dir().join(".ssh/config"))
        .unwrap_or_else(|| PathBuf::from(".ssh/config"))
}

pub trait SshPermissionControl {
    fn restrict_private_path(&self, path: &Path, directory: bool) -> Result<(), String>;
}

pub fn prepare_managed_ssh_server(
    server: &mut crate::ServerConfig,
    permissions: &dyn SshPermissionControl,
) -> Result<(), SshError> {
    let ssh_dir = default_ssh_config_path()
        .parent()
        .unwrap_or_else(|| Path::new(".ssh"))
        .to_owned();
    prepare_managed_ssh_server_at(server, permissions, &ssh_dir)
}

fn prepare_managed_ssh_server_at(
    server: &mut crate::ServerConfig,
    permissions: &dyn SshPermissionControl,
    ssh_dir: &Path,
) -> Result<(), SshError> {
    if !server.ssh_config_managed {
        return Ok(());
    }
    validate_host_alias(&server.host_alias)?;
    validate_config_scalar(&server.host, "HostName")?;
    validate_config_scalar(&server.user, "User")?;
    validate_port(&server.port)?;

    let managed_dir = ssh_dir.join("ssh-mountmate.d");
    let lock_path = ssh_dir.join("ssh-mountmate.lock");
    fs::create_dir_all(&managed_dir).map_err(|source| SshError::Io {
        path: managed_dir.clone(),
        source,
    })?;
    restrict_path(permissions, ssh_dir, true)?;
    restrict_path(permissions, &managed_dir, true)?;
    let _lock = FileLock::acquire(&lock_path, Duration::from_secs(30))?;

    if server.copy_key_to_ssh_dir {
        server.key_file = copy_private_key(
            Path::new(&server.key_file),
            ssh_dir,
            &server.host_alias,
            permissions,
        )?
        .display()
        .to_string();
    } else if !server.key_file.trim().is_empty() && !expand_home(&server.key_file).is_file() {
        return Err(SshError::InvalidPrivateKey(expand_home(&server.key_file)));
    }

    ensure_managed_include(ssh_dir, permissions)?;
    let target = managed_dir.join(format!("{}.conf", safe_ssh_filename(&server.host_alias)));
    let mut content = format!(
        "# Managed by SSH MountMate.\n# Prefer editing this Host from the SSH MountMate app.\nHost {}\n    HostName {}\n    User {}\n    Port {}\n",
        quote_ssh_value(&server.host_alias),
        quote_ssh_value(&server.host),
        quote_ssh_value(&server.user),
        server.port,
    );
    if !server.key_file.trim().is_empty() {
        content.push_str(&format!(
            "    IdentityFile {}\n    IdentitiesOnly yes\n",
            quote_ssh_value(&home_relative_ssh_path(Path::new(&server.key_file)))
        ));
    }
    atomic_write(&target, content.as_bytes())?;
    restrict_path(permissions, &target, false)?;
    server.managed_ssh_config_path = target.display().to_string();
    Ok(())
}

pub fn remove_managed_ssh_server(server: &crate::ServerConfig) -> Result<(), SshError> {
    let managed_dir = default_ssh_config_path()
        .parent()
        .unwrap_or_else(|| Path::new(".ssh"))
        .join("ssh-mountmate.d");
    let candidates = [
        (!server.managed_ssh_config_path.trim().is_empty())
            .then(|| expand_home(&server.managed_ssh_config_path)),
        (!server.host_alias.trim().is_empty())
            .then(|| managed_dir.join(format!("{}.conf", safe_ssh_filename(&server.host_alias)))),
    ];
    let managed_identity = fs::canonicalize(&managed_dir).unwrap_or(managed_dir);
    for candidate in candidates.into_iter().flatten() {
        let identity = fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if identity.parent() == Some(managed_identity.as_path()) {
            match fs::remove_file(&candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(SshError::Io {
                        path: candidate,
                        source,
                    });
                }
            }
        }
    }
    Ok(())
}

fn restrict_path(
    permissions: &dyn SshPermissionControl,
    path: &Path,
    directory: bool,
) -> Result<(), SshError> {
    permissions
        .restrict_private_path(path, directory)
        .map_err(|message| SshError::Permissions {
            path: path.to_owned(),
            message,
        })
}

fn ensure_managed_include(
    ssh_dir: &Path,
    permissions: &dyn SshPermissionControl,
) -> Result<(), SshError> {
    let config = ssh_dir.join("config");
    let include = "Include ~/.ssh/ssh-mountmate.d/*.conf";
    let mut content = match fs::read_to_string(&config) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(SshError::Io {
                path: config,
                source,
            });
        }
    };
    if !content
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case(include))
    {
        if !content.is_empty() && !content.ends_with(['\n', '\r']) {
            content.push('\n');
        }
        content.push_str(include);
        content.push('\n');
        atomic_write(&config, content.as_bytes())?;
    }
    restrict_path(permissions, &config, false)
}

fn copy_private_key(
    source: &Path,
    ssh_dir: &Path,
    host_alias: &str,
    permissions: &dyn SshPermissionControl,
) -> Result<PathBuf, SshError> {
    let source = expand_home(&source.to_string_lossy());
    if !source.is_file()
        || source
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case("pub"))
    {
        return Err(SshError::InvalidPrivateKey(source));
    }
    let stem = safe_ssh_filename(host_alias);
    let suffix = source
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let mut target = ssh_dir.join(format!("{stem}{suffix}"));
    if fs::canonicalize(&source).ok() != fs::canonicalize(&target).ok() && target.exists() {
        target = (2..1000)
            .map(|index| ssh_dir.join(format!("{stem}-{index}{suffix}")))
            .find(|candidate| !candidate.exists())
            .unwrap_or_else(|| {
                ssh_dir.join(format!("{stem}-{}{suffix}", uuid::Uuid::new_v4().simple()))
            });
    }
    if fs::canonicalize(&source).ok() != fs::canonicalize(&target).ok() {
        let content = fs::read(&source).map_err(|source_error| SshError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        atomic_write(&target, &content)?;
    }
    restrict_path(permissions, &target, false)?;
    Ok(target)
}

fn validate_config_scalar(value: &str, field: &str) -> Result<(), SshError> {
    let value = value.trim();
    if value.is_empty()
        || value.starts_with('-')
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        Err(SshError::Command(format!("invalid {field}")))
    } else {
        Ok(())
    }
}

fn safe_ssh_filename(value: &str) -> String {
    let value: String = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let value = value.trim_matches(['.', '_', '-']);
    if value.is_empty() {
        format!("host-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
    } else {
        value.into()
    }
}

fn quote_ssh_value(value: &str) -> String {
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_whitespace() || matches!(character, '"' | '\\'))
    {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.into()
    }
}

fn home_relative_ssh_path(path: &Path) -> String {
    let path = expand_home(&path.to_string_lossy());
    if let Some(home) = directories::BaseDirs::new()
        && let Ok(relative) = path.strip_prefix(home.home_dir())
    {
        return format!("~/{}", relative.to_string_lossy().replace('\\', "/"));
    }
    path.to_string_lossy().replace('\\', "/")
}

fn visit_ssh_config(
    config_path: &Path,
    seen: &mut HashSet<PathBuf>,
    entries: &mut Vec<SshHostEntry>,
) -> Result<(), SshError> {
    let identity = fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_owned());
    if !seen.insert(identity) || !config_path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(config_path).map_err(|source| SshError::Io {
        path: config_path.to_owned(),
        source,
    })?;
    for (line_index, raw) in content.lines().enumerate() {
        let words = split_ssh_words(raw);
        let Some(keyword) = words.first() else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("include") {
            for pattern in &words[1..] {
                let pattern = resolve_include_pattern(config_path, pattern);
                let pattern_text = pattern.to_string_lossy().into_owned();
                let matches = glob(&pattern_text).map_err(|error| SshError::IncludePattern {
                    pattern: pattern_text.clone(),
                    message: error.to_string(),
                })?;
                for included in matches.flatten() {
                    if included.is_file() {
                        visit_ssh_config(&included, seen, entries)?;
                    }
                }
            }
        } else if keyword.eq_ignore_ascii_case("host") {
            for host in &words[1..] {
                if !host.contains(['*', '?', '!']) {
                    entries.push(SshHostEntry {
                        host: host.to_owned(),
                        path: config_path.to_owned(),
                        line: line_index + 1,
                        raw: raw.to_owned(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn resolve_include_pattern(config_path: &Path, pattern: &str) -> PathBuf {
    let expanded = expand_home(pattern);
    if expanded.is_absolute() {
        expanded
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(expanded)
    }
}

fn split_ssh_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut characters = line.chars().peekable();
    while let Some(ch) = characters.next() {
        if ch == '\\' {
            if characters
                .peek()
                .is_some_and(|next| next.is_whitespace() || matches!(next, '\'' | '"' | '#'))
            {
                current.push(characters.next().expect("peeked character exists"));
            } else {
                current.push(ch);
            }
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch == '#' && current.is_empty() {
            break;
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn expand_home(value: &str) -> PathBuf {
    if (value == "~" || value.starts_with("~/") || value.starts_with("~\\"))
        && let Some(home) = directories::BaseDirs::new()
    {
        return if value == "~" {
            home.home_dir().to_owned()
        } else {
            home.home_dir().join(&value[2..])
        };
    }
    PathBuf::from(value)
}

pub fn scan_host_keys(
    keyscan: &Path,
    host: &str,
    port: &str,
    timeout: Duration,
) -> Result<Vec<String>, SshError> {
    validate_host_alias(host)?;
    let port = validate_port(port)?;
    let mut command = Command::new(keyscan);
    command
        .args(["-T", "8", "-p", &port, "-t", "rsa,ecdsa,ed25519"])
        .arg(host)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let mut child = command.spawn().map_err(|source| SshError::Io {
        path: keyscan.to_owned(),
        source,
    })?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        if let Some(mut stdout) = stdout {
            stdout.read_to_string(&mut output)?;
        }
        Ok::<_, std::io::Error>(output)
    });
    let wait_result = child.wait_timeout(timeout);
    let status = match wait_result {
        Ok(status) => status,
        Err(source) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(SshError::Io {
                path: keyscan.to_owned(),
                source,
            });
        }
    };
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return Err(SshError::Command("ssh-keyscan timed out".into()));
    }
    let output = reader
        .join()
        .map_err(|_| SshError::Command("ssh-keyscan output reader panicked".into()))?
        .map_err(|source| SshError::Io {
            path: keyscan.to_owned(),
            source,
        })?;
    Ok(normalize_host_key_output(host, &port, &output))
}

pub fn normalize_host_key_output(host: &str, port: &str, output: &str) -> Vec<String> {
    let marker = known_hosts_marker(host, normalize_port(port));
    let mut seen = HashSet::new();
    let mut keys = Vec::new();
    for line in output.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 3
            || !(parts[1].starts_with("ssh-")
                || parts[1].starts_with("ecdsa-")
                || parts[1].starts_with("sk-"))
        {
            continue;
        }
        let normalized = format!("{marker} {}", parts[1..].join(" "));
        if seen.insert(normalized.clone()) {
            keys.push(normalized);
        }
    }
    keys
}

fn normalize_port(port: &str) -> &str {
    if port.trim().is_empty() { "22" } else { port }
}

fn validate_port(port: &str) -> Result<String, SshError> {
    let port = normalize_port(port);
    match port.parse::<u16>() {
        Ok(value) if value > 0 => Ok(value.to_string()),
        _ => Err(SshError::InvalidPort(port.to_owned())),
    }
}

pub struct KnownHostsManager<'a> {
    paths: &'a AppPaths,
    lock_timeout: Duration,
}

impl<'a> KnownHostsManager<'a> {
    pub fn new(paths: &'a AppPaths) -> Self {
        Self {
            paths,
            lock_timeout: Duration::from_secs(30),
        }
    }

    pub fn pin_first_seen(
        &self,
        keyscan: &Path,
        host: &str,
        port: &str,
    ) -> Result<Option<PathBuf>, SshError> {
        self.pin_first_seen_with(host, port, || {
            scan_host_keys(keyscan, host, port, Duration::from_secs(12))
        })
    }

    pub fn pin_first_seen_with<F>(
        &self,
        host: &str,
        port: &str,
        scan: F,
    ) -> Result<Option<PathBuf>, SshError>
    where
        F: FnOnce() -> Result<Vec<String>, SshError>,
    {
        validate_host_alias(host)?;
        let port = validate_port(port)?;
        let marker = known_hosts_marker(host, &port);
        if self.managed_file_contains(&marker)? {
            return Ok(readable_file(&self.paths.known_hosts()));
        }

        let scanned = scan()?;
        if scanned.is_empty() {
            return Ok(None);
        }

        let _lock = FileLock::acquire(&self.paths.known_hosts_lock(), self.lock_timeout)?;
        if self.managed_file_contains(&marker)? {
            return Ok(readable_file(&self.paths.known_hosts()));
        }
        let path = self.paths.known_hosts();
        let mut content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(SshError::Io {
                    path: path.clone(),
                    source,
                });
            }
        };
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        for line in scanned {
            content.push_str(&line);
            content.push('\n');
        }
        atomic_write(&path, content.as_bytes())?;
        Ok(readable_file(&path))
    }

    fn managed_file_contains(&self, marker: &str) -> Result<bool, SshError> {
        let path = self.paths.known_hosts();
        match fs::read_to_string(&path) {
            Ok(content) => Ok(content
                .lines()
                .any(|line| known_hosts_line_matches(line, marker))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(SshError::Io { path, source }),
        }
    }
}

pub fn select_readable_known_hosts(
    managed: Option<&Path>,
    resolved: &ResolvedSshConfig,
    default: &Path,
) -> Option<PathBuf> {
    managed
        .and_then(readable_file)
        .or_else(|| {
            resolved
                .all("userknownhostsfile")
                .iter()
                .flat_map(|value| split_ssh_words(value))
                .map(|value| expand_home(&value))
                .find_map(|path| readable_file(&path))
        })
        .or_else(|| readable_file(default))
}

pub fn select_known_hosts_for_marker(
    managed: Option<&Path>,
    resolved: Option<&ResolvedSshConfig>,
    default: &Path,
    marker: &str,
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = managed.and_then(readable_file) {
        candidates.push(path);
    }
    if let Some(config) = resolved {
        candidates.extend(
            config
                .all("userknownhostsfile")
                .iter()
                .flat_map(|value| split_ssh_words(value))
                .map(|value| expand_home(&value))
                .filter_map(|path| readable_file(&path)),
        );
    }
    if let Some(path) = readable_file(default) {
        candidates.push(path);
    }

    candidates
        .iter()
        .find(|path| known_hosts_file_contains_marker(path, marker))
        .cloned()
        .or_else(|| {
            candidates
                .into_iter()
                .find(|path| known_hosts_file_contains_hashed_marker(path, marker))
        })
}

fn known_hosts_file_contains_marker(path: &Path, marker: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|content| {
        content
            .lines()
            .any(|line| known_hosts_line_matches(line, marker))
    })
}

fn known_hosts_file_contains_hashed_marker(path: &Path, marker: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|content| {
        content.lines().any(|line| {
            let mut parts = line.split_whitespace();
            let first = parts.next();
            let hosts = if first.is_some_and(|value| value.starts_with('@')) {
                parts.next()
            } else {
                first
            };
            hosts.is_some_and(|hosts| {
                hosts
                    .split(',')
                    .any(|host| hashed_host_matches(host, marker))
            })
        })
    })
}

fn hashed_host_matches(value: &str, marker: &str) -> bool {
    let Some(value) = value.strip_prefix("|1|") else {
        return false;
    };
    let Some((encoded_salt, encoded_tag)) = value.split_once('|') else {
        return false;
    };
    let engine = base64::engine::general_purpose::STANDARD;
    let Ok(salt) = engine.decode(encoded_salt) else {
        return false;
    };
    let Ok(tag) = engine.decode(encoded_tag) else {
        return false;
    };
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &salt);
    hmac::verify(&key, marker.as_bytes(), &tag).is_ok()
}

pub fn readable_file(path: &Path) -> Option<PathBuf> {
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    File::open(path).ok().map(|_| path.to_owned())
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use tempfile::tempdir;

    use super::*;

    #[derive(Default)]
    struct FakePermissions {
        paths: RefCell<Vec<(PathBuf, bool)>>,
    }

    impl SshPermissionControl for FakePermissions {
        fn restrict_private_path(&self, path: &Path, directory: bool) -> Result<(), String> {
            self.paths.borrow_mut().push((path.to_owned(), directory));
            Ok(())
        }
    }

    fn paths(root: &Path) -> AppPaths {
        AppPaths {
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            data_dir: root.join("data"),
        }
    }

    #[test]
    fn discovers_literal_hosts_through_includes_and_cycles() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("config");
        let includes = temp.path().join("config.d");
        fs::create_dir_all(&includes).unwrap();
        fs::write(
            &root,
            "Include config.d/*.conf\nHost direct *.wild !excluded\n",
        )
        .unwrap();
        fs::write(
            includes.join("cluster.conf"),
            "Include ../config\nHost cluster other\n",
        )
        .unwrap();

        let entries = list_ssh_config_hosts(&root).unwrap();
        let hosts: Vec<_> = entries.iter().map(|entry| entry.host.as_str()).collect();

        assert_eq!(hosts, ["cluster", "other", "direct"]);
        assert_eq!(entries[0].line, 2);
    }

    #[test]
    fn parses_resolved_config_and_detects_proxy_requirements() {
        let config = ResolvedSshConfig::parse(
            "hostname c1.example\nport 12022\nidentityfile ~/.ssh/id_ed25519\nproxyjump bastion\n",
        );
        assert_eq!(config.first("HostName", ""), "c1.example");
        assert_eq!(config.all("identityfile"), ["~/.ssh/id_ed25519"]);
        assert!(config.needs_openssh_transport());
        assert_eq!(
            choose_transport(RequestedTransport::Auto, &config, true),
            SshTransport::Openssh
        );
    }

    #[test]
    fn ssh_word_parser_preserves_windows_paths() {
        assert_eq!(
            split_ssh_words(r#"C:\Users\Agent\.ssh\known_hosts "D:\key files\id""#),
            [r"C:\Users\Agent\.ssh\known_hosts", r"D:\key files\id"]
        );
    }

    #[test]
    fn auto_transport_uses_native_on_windows_without_proxy() {
        let config = ResolvedSshConfig::parse("hostname c1.example\nproxycommand none\n");
        assert_eq!(
            choose_transport(RequestedTransport::Auto, &config, true),
            SshTransport::Native
        );
        assert_eq!(
            choose_transport(RequestedTransport::Auto, &config, false),
            SshTransport::Openssh
        );
    }

    #[test]
    fn normalizes_keyscan_hosts_and_deduplicates_keys() {
        let output = "# banner\n[c1.example]:12022 ssh-ed25519 AAAA\nc1.example ssh-rsa BBBB\nc1.example ssh-rsa BBBB\nbad line\n";
        assert_eq!(
            normalize_host_key_output("c1.example", "12022", output),
            [
                "[c1.example]:12022 ssh-ed25519 AAAA",
                "[c1.example]:12022 ssh-rsa BBBB"
            ]
        );
    }

    #[test]
    fn host_marker_supports_ports_and_known_hosts_directives() {
        assert_eq!(known_hosts_marker("example.com", "22"), "example.com");
        assert_eq!(
            known_hosts_marker("example.com", "12022"),
            "[example.com]:12022"
        );
        assert!(known_hosts_line_matches(
            "@cert-authority example.com ssh-ed25519 AAAA",
            "example.com"
        ));
    }

    #[test]
    fn keeps_first_seen_keys_without_rescanning() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(
            paths.known_hosts(),
            "[c1.example]:12022 ssh-ed25519 AAAAPINNED\n",
        )
        .unwrap();
        let called = Cell::new(false);

        let selected = KnownHostsManager::new(&paths)
            .pin_first_seen_with("c1.example", "12022", || {
                called.set(true);
                Ok(vec!["replacement".into()])
            })
            .unwrap();

        assert_eq!(selected, Some(paths.known_hosts()));
        assert!(!called.get());
        assert!(
            fs::read_to_string(paths.known_hosts())
                .unwrap()
                .contains("AAAAPINNED")
        );
    }

    #[test]
    fn writes_first_seen_keys_privately() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let selected = KnownHostsManager::new(&paths)
            .pin_first_seen_with("c1.example", "12022", || {
                Ok(vec!["[c1.example]:12022 ssh-ed25519 AAAAFIRST".into()])
            })
            .unwrap();

        assert_eq!(selected, Some(paths.known_hosts()));
        assert_eq!(
            fs::read_to_string(paths.known_hosts()).unwrap(),
            "[c1.example]:12022 ssh-ed25519 AAAAFIRST\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                paths.known_hosts().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn managed_ssh_profile_is_idempotent_and_copies_private_key() {
        let temp = tempdir().unwrap();
        let ssh_dir = temp.path().join(".ssh");
        let source = temp.path().join("source key");
        fs::write(&source, "PRIVATE KEY").unwrap();
        let permissions = FakePermissions::default();
        let mut server = crate::ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            host_alias: "alpha".into(),
            host: "alpha.example".into(),
            user: "alice".into(),
            port: "2202".into(),
            key_file: source.display().to_string(),
            ssh_config_managed: true,
            copy_key_to_ssh_dir: true,
            ..crate::ServerConfig::default()
        };

        prepare_managed_ssh_server_at(&mut server, &permissions, &ssh_dir).unwrap();
        prepare_managed_ssh_server_at(&mut server, &permissions, &ssh_dir).unwrap();

        let config = fs::read_to_string(ssh_dir.join("config")).unwrap();
        assert_eq!(
            config
                .lines()
                .filter(|line| line.starts_with("Include "))
                .count(),
            1
        );
        let managed = PathBuf::from(&server.managed_ssh_config_path);
        let content = fs::read_to_string(&managed).unwrap();
        assert!(content.contains("Host alpha"));
        assert!(content.contains("HostName alpha.example"));
        assert!(content.contains("Port 2202"));
        assert!(content.contains("IdentitiesOnly yes"));
        assert!(Path::new(&server.key_file).starts_with(&ssh_dir));
        assert_eq!(fs::read(&server.key_file).unwrap(), b"PRIVATE KEY");
        assert!(
            permissions
                .paths
                .borrow()
                .iter()
                .any(|(path, directory)| { path == &managed && !directory })
        );
    }

    #[test]
    fn never_selects_a_directory_as_known_hosts() {
        let temp = tempdir().unwrap();
        let managed = temp.path().join("managed");
        let fallback = temp.path().join("fallback");
        fs::create_dir(&managed).unwrap();
        fs::write(&fallback, "host ssh-ed25519 AAAA\n").unwrap();
        let config =
            ResolvedSshConfig::parse(&format!("userknownhostsfile {}\n", managed.display()));

        assert_eq!(
            select_readable_known_hosts(Some(&managed), &config, &fallback),
            Some(fallback)
        );
    }

    #[test]
    fn unrelated_managed_keys_do_not_hide_a_matching_default_key() {
        let temp = tempdir().unwrap();
        let managed = temp.path().join("managed");
        let default = temp.path().join("default");
        fs::write(&managed, "[other.example]:2200 ssh-ed25519 AAAAOTHER\n").unwrap();
        fs::write(&default, "[target.example]:2200 ssh-ed25519 AAAATARGET\n").unwrap();

        assert_eq!(
            select_known_hosts_for_marker(Some(&managed), None, &default, "[target.example]:2200"),
            Some(default)
        );
    }

    #[test]
    fn hashed_hosts_remain_eligible_when_no_plaintext_marker_matches() {
        let temp = tempdir().unwrap();
        let managed = temp.path().join("managed");
        fs::write(
            &managed,
            "|1|c2FsdA==|mF58uSfHH9jfpQnmp1eRRf3z0VY= ssh-ed25519 AAAAHASHED\n",
        )
        .unwrap();

        assert_eq!(
            select_known_hosts_for_marker(
                Some(&managed),
                None,
                &temp.path().join("missing"),
                "[target.example]:2200"
            ),
            Some(managed)
        );
    }

    #[test]
    fn hashed_hosts_do_not_match_a_different_marker() {
        assert!(hashed_host_matches(
            "|1|c2FsdA==|mF58uSfHH9jfpQnmp1eRRf3z0VY=",
            "[target.example]:2200"
        ));
        assert!(!hashed_host_matches(
            "|1|c2FsdA==|mF58uSfHH9jfpQnmp1eRRf3z0VY=",
            "[other.example]:2200"
        ));
    }

    #[test]
    fn rejects_option_like_host_aliases() {
        assert!(validate_host_alias("-oProxyCommand=bad").is_err());
        assert!(validate_host_alias("cluster.example").is_ok());
    }
}
