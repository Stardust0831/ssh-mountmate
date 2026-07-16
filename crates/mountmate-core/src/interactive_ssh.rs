use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::ServerConfig;
use crate::paths::AppPaths;
use crate::plink_binary::resolve_plink;
use crate::rclone_binary::{RcloneBinaryError, find_system_executable};
use crate::ssh::resolve_ssh_config;

#[derive(Debug, Error)]
pub enum InteractiveSshError {
    #[error(
        "interactive SSH on Windows supports direct manual connections only; SSH-config profiles and proxy translation are not supported"
    )]
    UnsupportedWindowsSshConfig,
    #[error(
        "interactive shared SSH cannot bypass ProxyJump or ProxyCommand from the SSH config; use OpenSSH transport"
    )]
    UnsupportedWindowsSshProxy,
    #[error("verified Plink is missing from this Windows package")]
    PlinkMissing,
    #[error("OpenSSH was not found")]
    OpenSshMissing,
    #[error("interactive SSH shared session is not ready; complete login in the app terminal")]
    SessionMissing,
    #[error("interactive SSH readiness check timed out after {timeout:?}")]
    ReadinessTimeout { timeout: Duration },
    #[error("could not start interactive SSH: {0}")]
    Process(String),
    #[error(transparent)]
    Binary(#[from] RcloneBinaryError),
}

#[derive(Debug, Clone)]
pub struct InteractiveSshSession {
    connector: Vec<String>,
    check_program: PathBuf,
    check_arguments: Vec<String>,
    login: InteractiveSshLoginCommand,
    #[cfg(unix)]
    control_dir: PathBuf,
    #[cfg(unix)]
    control_socket: PathBuf,
}

/// Immutable command specification for the app-owned interactive SSH session.
///
/// The program is kept as a `PathBuf` and arguments as `OsString`s so callers
/// can pass the exact verified command to a PTY without lossy path conversion
/// or shell quoting. Creating or running the command remains the caller's
/// responsibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveSshLoginCommand {
    program: PathBuf,
    arguments: Vec<OsString>,
}

impl InteractiveSshLoginCommand {
    fn new(program: PathBuf, arguments: Vec<OsString>) -> Self {
        Self { program, arguments }
    }

    /// Return the verified interactive login executable path.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Return the exact argument vector for the login executable.
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnixPathMetadata {
    is_symlink: bool,
    is_directory: bool,
    is_socket: bool,
    uid: u32,
    mode: u32,
}

#[cfg(unix)]
trait UnixMetadataProvider {
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<UnixPathMetadata>;
    fn current_uid(&self) -> u32;
}

#[cfg(unix)]
struct SystemUnixMetadata;

#[cfg(unix)]
impl UnixMetadataProvider for SystemUnixMetadata {
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<UnixPathMetadata> {
        let metadata = fs::symlink_metadata(path)?;
        let file_type = metadata.file_type();
        Ok(UnixPathMetadata {
            is_symlink: file_type.is_symlink(),
            is_directory: file_type.is_dir(),
            is_socket: file_type.is_socket(),
            uid: metadata.uid(),
            mode: metadata.mode(),
        })
    }

    fn current_uid(&self) -> u32 {
        rustix::process::geteuid().as_raw()
    }
}

impl InteractiveSshSession {
    const DEFAULT_READINESS_TIMEOUT: Duration = Duration::from_secs(2);

    pub fn for_server(
        paths: &AppPaths,
        app_root: &Path,
        server: &ServerConfig,
    ) -> Result<Self, InteractiveSshError> {
        if cfg!(windows) {
            return Self::windows(paths, app_root, server);
        }
        Self::openssh(paths, server)
    }

    pub fn connector_arguments(&self) -> &[String] {
        &self.connector
    }

    /// Return the immutable command specification for the app-owned PTY.
    pub fn login_command(&self) -> &InteractiveSshLoginCommand {
        &self.login
    }

    /// Return connector arguments only after revalidating the control paths.
    /// Existing callers can retain the infallible accessor while code that is
    /// about to spawn a connector can use this checked form.
    pub fn verified_connector_arguments(&self) -> Result<&[String], InteractiveSshError> {
        #[cfg(unix)]
        self.validate_control_paths()?;
        Ok(&self.connector)
    }

    #[cfg(unix)]
    fn validate_control_paths(&self) -> Result<(), InteractiveSshError> {
        validate_control_directory(&self.control_dir)?;
        validate_optional_control_socket(&self.control_socket)?;
        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        self.check_ready(Self::DEFAULT_READINESS_TIMEOUT)
            .unwrap_or(false)
    }

    /// Check whether the app-owned SSH session is ready, with a hard deadline.
    ///
    /// The check is intended to run from a blocking worker (for example,
    /// `spawn_blocking`) because it polls a child process. A successful exit
    /// returns `Ok(true)`, a normal nonzero exit returns `Ok(false)`, and
    /// spawning, polling, termination, or timeout failures return an error.
    /// On Unix, control-path validation runs before spawning and a normal
    /// not-ready result attempts safe stale-socket cleanup. Process-management
    /// errors leave the socket untouched because they do not prove it is stale.
    pub fn check_ready(&self, timeout: Duration) -> Result<bool, InteractiveSshError> {
        #[cfg(unix)]
        self.validate_control_paths()?;

        let mut command = Command::new(&self.check_program);
        command
            .args(&self.check_arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_readiness_command(&mut command);
        let result = run_readiness_command(&mut command, timeout);
        if matches!(result, Ok(false)) {
            self.cleanup_failed_readiness();
        }
        result
    }

    /// Return the default bounded readiness timeout used by [`Self::is_ready`].
    pub const fn default_readiness_timeout() -> Duration {
        Self::DEFAULT_READINESS_TIMEOUT
    }

    #[cfg(unix)]
    fn cleanup_failed_readiness(&self) {
        // A verified socket that no longer answers is stale. Never remove an
        // object unless it passes the same identity checks used above.
        let _ = cleanup_control_socket(&self.control_socket);
    }

    #[cfg(not(unix))]
    fn cleanup_failed_readiness(&self) {}

    /// Remove this server's stale OpenSSH control socket when it is safe to do
    /// so. Invalid, replaced, or foreign-owned paths are left untouched.
    pub fn cleanup(&self) -> Result<(), InteractiveSshError> {
        #[cfg(unix)]
        {
            validate_control_directory(&self.control_dir)?;
            cleanup_control_socket(&self.control_socket)?;
        }
        Ok(())
    }

    fn openssh(paths: &AppPaths, server: &ServerConfig) -> Result<Self, InteractiveSshError> {
        let ssh = find_system_executable("ssh").ok_or(InteractiveSshError::OpenSshMissing)?;
        let id_hash = format!("{:x}", Sha256::digest(server.id.as_bytes()));
        let control_dir = control_directory(paths, &id_hash);
        #[cfg(unix)]
        ensure_control_directory(&control_dir)?;
        #[cfg(not(unix))]
        fs::create_dir_all(&control_dir)
            .map_err(|error| InteractiveSshError::Process(error.to_string()))?;
        let control = control_dir.join(format!("{}.sock", &id_hash[..16]));
        #[cfg(unix)]
        validate_optional_control_socket(&control)?;
        let target = openssh_target_arguments(server);
        let mut connector = vec![
            ssh.display().to_string(),
            "-S".into(),
            control.display().to_string(),
            "-o".into(),
            "ControlMaster=no".into(),
            "-o".into(),
            "BatchMode=yes".into(),
        ];
        connector.extend(target.clone());
        let mut check_arguments = vec![
            "-S".into(),
            control.display().to_string(),
            "-O".into(),
            "check".into(),
        ];
        check_arguments.extend(target.clone());
        let login = InteractiveSshLoginCommand::new(
            ssh.clone(),
            openssh_login_arguments(&control, &target),
        );
        Ok(Self {
            connector,
            check_program: ssh,
            check_arguments,
            login,
            #[cfg(unix)]
            control_dir,
            #[cfg(unix)]
            control_socket: control,
        })
    }

    fn windows(
        paths: &AppPaths,
        app_root: &Path,
        server: &ServerConfig,
    ) -> Result<Self, InteractiveSshError> {
        reject_windows_proxy_config(server)?;
        if !windows_direct_connection_supported(server) {
            return Err(InteractiveSshError::UnsupportedWindowsSshConfig);
        }
        let plink = resolve_plink(paths, app_root)?.ok_or(InteractiveSshError::PlinkMissing)?;
        let target = plink_target_arguments(server);
        let mut connector = vec![
            plink.path.display().to_string(),
            "-batch".into(),
            "-ssh".into(),
            "-share".into(),
        ];
        connector.extend(target.clone());
        let mut check_arguments = vec!["-batch".into(), "-ssh".into(), "-shareexists".into()];
        check_arguments.extend(target.clone());
        Ok(Self {
            connector,
            check_program: plink.path.clone(),
            check_arguments,
            login: InteractiveSshLoginCommand::new(plink.path, plink_login_arguments(&target)),
            #[cfg(unix)]
            control_dir: PathBuf::new(),
            #[cfg(unix)]
            control_socket: PathBuf::new(),
        })
    }
}

fn reject_windows_proxy_config(server: &ServerConfig) -> Result<(), InteractiveSshError> {
    if (!matches!(server.source.as_str(), "ssh_config" | "ssh_config_batch")
        && !server.ssh_config_managed)
        || server.host_alias.trim().is_empty()
    {
        return Ok(());
    }
    let ssh = find_system_executable(if cfg!(windows) { "ssh.exe" } else { "ssh" })
        .ok_or(InteractiveSshError::OpenSshMissing)?;
    let config =
        (!server.ssh_config_path.trim().is_empty()).then(|| Path::new(&server.ssh_config_path));
    let resolved = resolve_ssh_config(&ssh, &server.host_alias, config)
        .map_err(|error| InteractiveSshError::Process(error.to_string()))?;
    if !windows_resolved_config_supported(&resolved) {
        Err(InteractiveSshError::UnsupportedWindowsSshProxy)
    } else {
        Ok(())
    }
}

fn windows_resolved_config_supported(resolved: &crate::ssh::ResolvedSshConfig) -> bool {
    !resolved.needs_openssh_transport()
}

fn run_readiness_command(
    command: &mut Command,
    timeout: Duration,
) -> Result<bool, InteractiveSshError> {
    let mut child = command.spawn().map_err(|error| {
        InteractiveSshError::Process(format!("could not spawn SSH readiness check: {error}"))
    })?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => {
                let now = Instant::now();
                if now >= deadline {
                    terminate_readiness_child(&mut child)?;
                    return Err(InteractiveSshError::ReadinessTimeout { timeout });
                }
                thread::sleep(
                    deadline
                        .saturating_duration_since(now)
                        .min(Duration::from_millis(10)),
                );
            }
            Err(error) => {
                terminate_readiness_child(&mut child)?;
                return Err(InteractiveSshError::Process(format!(
                    "could not poll SSH readiness check: {error}"
                )));
            }
        }
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(windows)]
const fn readiness_creation_flags() -> u32 {
    CREATE_NO_WINDOW
}

#[cfg(all(not(windows), test))]
const fn readiness_creation_flags() -> u32 {
    0
}

#[cfg(windows)]
fn configure_readiness_command(command: &mut Command) {
    command.creation_flags(readiness_creation_flags());
}

#[cfg(not(windows))]
fn configure_readiness_command(_command: &mut Command) {}

fn terminate_readiness_child(child: &mut Child) -> Result<(), InteractiveSshError> {
    if let Err(error) = child.kill() {
        return match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) | Err(_) => Err(InteractiveSshError::Process(format!(
                "could not terminate SSH readiness check: {error}"
            ))),
        };
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                return Err(InteractiveSshError::Process(
                    "SSH readiness check did not exit after termination".into(),
                ));
            }
            Err(error) => {
                return Err(InteractiveSshError::Process(format!(
                    "could not reap SSH readiness check: {error}"
                )));
            }
        }
    }
}

fn control_directory(paths: &AppPaths, id_hash: &str) -> PathBuf {
    let preferred = paths.state_dir.join("ssh-control");
    if preferred.as_os_str().to_string_lossy().len() + id_hash.len().min(16) + 6 <= 96 {
        preferred
    } else {
        let state_hash = format!(
            "{:x}",
            Sha256::digest(paths.state_dir.as_os_str().to_string_lossy().as_bytes())
        );
        std::env::temp_dir().join(format!("ssh-mountmate-{}", &state_hash[..16]))
    }
}

#[cfg(unix)]
fn ensure_control_directory(path: &Path) -> Result<(), InteractiveSshError> {
    // Refuse to traverse an attacker-controlled symlink while creating the
    // directory. The final directory is checked again after creation because
    // the path can be replaced concurrently.
    validate_existing_path_components(path.parent().unwrap_or_else(|| Path::new(".")))?;
    let existed = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(invalid_control_path(path, "is a symbolic link"));
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(control_path_io_error(path, error)),
    };
    if !existed {
        fs::create_dir_all(path).map_err(|error| control_path_io_error(path, error))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| control_path_io_error(path, error))?;
    }
    validate_control_directory(path)
}

#[cfg(unix)]
fn validate_existing_path_components(path: &Path) -> Result<(), InteractiveSshError> {
    let mut current = path.to_owned();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    return Err(invalid_control_path(
                        &current,
                        "a parent is a symbolic link",
                    ));
                }
                if !file_type.is_dir() {
                    return Err(invalid_control_path(
                        &current,
                        "a parent is not a directory",
                    ));
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(parent) = current.parent() else {
                    break;
                };
                if parent == current {
                    break;
                }
                current = parent.to_owned();
            }
            Err(error) => return Err(control_path_io_error(&current, error)),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn validate_control_directory(path: &Path) -> Result<(), InteractiveSshError> {
    validate_existing_path_components(path.parent().unwrap_or_else(|| Path::new(".")))?;
    validate_control_directory_with(&SystemUnixMetadata, path)
        .map_err(|reason| invalid_control_path(path, reason))
}

#[cfg(unix)]
fn validate_control_directory_with(
    metadata: &dyn UnixMetadataProvider,
    path: &Path,
) -> Result<(), &'static str> {
    let observed = metadata
        .symlink_metadata(path)
        .map_err(|_| "could not inspect")?;
    let uid = metadata.current_uid();
    if observed.is_symlink {
        return Err("is a symbolic link");
    }
    if !observed.is_directory {
        return Err("is not a directory");
    }
    if observed.uid != uid {
        return Err("is not owned by the current user");
    }
    if observed.mode & 0o7777 != 0o700 {
        return Err("does not have owner-only permissions");
    }
    Ok(())
}

#[cfg(unix)]
fn validate_optional_control_socket(path: &Path) -> Result<bool, InteractiveSshError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            validate_control_socket(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(control_path_io_error(path, error)),
    }
}

#[cfg(unix)]
fn validate_control_socket(path: &Path) -> Result<(), InteractiveSshError> {
    validate_control_socket_with(&SystemUnixMetadata, path)
        .map_err(|reason| invalid_control_path(path, reason))
}

#[cfg(unix)]
fn validate_control_socket_with(
    metadata: &dyn UnixMetadataProvider,
    path: &Path,
) -> Result<(), &'static str> {
    let observed = metadata
        .symlink_metadata(path)
        .map_err(|_| "could not inspect")?;
    let uid = metadata.current_uid();
    if observed.is_symlink {
        return Err("is a symbolic link");
    }
    if !observed.is_socket {
        return Err("is not a Unix socket");
    }
    if observed.uid != uid {
        return Err("is not owned by the current user");
    }
    if observed.mode & 0o077 != 0 {
        return Err("has group or world permissions");
    }
    Ok(())
}

#[cfg(unix)]
fn cleanup_control_socket(path: &Path) -> Result<(), InteractiveSshError> {
    match validate_optional_control_socket(path)? {
        true => fs::remove_file(path).map_err(|error| control_path_io_error(path, error)),
        false => Ok(()),
    }
}

#[cfg(unix)]
fn invalid_control_path(path: &Path, reason: &str) -> InteractiveSshError {
    InteractiveSshError::Process(format!(
        "unsafe OpenSSH control path {}: {reason}",
        path.display()
    ))
}

#[cfg(unix)]
fn control_path_io_error(path: &Path, error: std::io::Error) -> InteractiveSshError {
    InteractiveSshError::Process(format!(
        "could not inspect OpenSSH control path {}: {error}",
        path.display()
    ))
}

fn openssh_target_arguments(server: &ServerConfig) -> Vec<String> {
    if (server.source == "ssh_config" || server.ssh_config_managed) && !server.host_alias.is_empty()
    {
        let mut arguments = Vec::new();
        if server.source == "ssh_config" && !server.ssh_config_path.trim().is_empty() {
            arguments.extend(["-F".into(), server.ssh_config_path.clone()]);
        }
        arguments.push(server.host_alias.clone());
        return arguments;
    }
    let mut arguments = vec![
        "-l".into(),
        server.user.clone(),
        "-p".into(),
        server.port.clone(),
    ];
    if !server.key_file.is_empty() {
        arguments.extend([
            "-i".into(),
            server.key_file.clone(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
        ]);
    }
    arguments.push(server.host.clone());
    arguments
}

fn plink_target_arguments(server: &ServerConfig) -> Vec<String> {
    let mut arguments = vec![
        "-P".into(),
        server.port.clone(),
        "-l".into(),
        server.user.clone(),
    ];
    if plink_private_key_supported(&server.key_file) {
        arguments.extend(["-i".into(), server.key_file.clone()]);
    }
    arguments.push(server.host.clone());
    arguments
}

fn plink_private_key_supported(key_file: &str) -> bool {
    if key_file.trim().is_empty() {
        return false;
    }
    File::open(key_file)
        .ok()
        .and_then(|file| BufReader::new(file).lines().next()?.ok())
        .is_some_and(|header| header.starts_with("PuTTY-User-Key-File-"))
}

fn plink_login_arguments(target: &[String]) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("-ssh"),
        OsString::from("-share"),
        OsString::from("-N"),
    ];
    arguments.extend(target.iter().map(OsString::from));
    arguments
}

fn windows_direct_connection_supported(server: &ServerConfig) -> bool {
    if !server.ssh_config_managed
        && !matches!(server.source.as_str(), "ssh_config" | "ssh_config_batch")
    {
        return true;
    }

    // Imported OpenSSH profiles are already resolved into the self-contained
    // HostName/User/Port fields on ServerConfig. Plink cannot consume the
    // OpenSSH config language itself, but it can safely use that resolved
    // direct target. A missing key is intentional: interactive login prompts
    // for the password or other auth challenge in the app-owned terminal.
    !server.host_alias.trim().is_empty()
        && !server.host.trim().is_empty()
        && !server.user.trim().is_empty()
}

fn openssh_login_arguments(control: &Path, target: &[String]) -> Vec<OsString> {
    let mut arguments = vec![
        "-M".into(),
        "-S".into(),
        control.as_os_str().to_owned(),
        "-o".into(),
        "BatchMode=no".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-N".into(),
    ];
    arguments.extend(target.iter().map(OsString::from));
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    struct FakeUnixMetadata {
        observed: UnixPathMetadata,
        uid: u32,
    }

    #[cfg(unix)]
    impl UnixMetadataProvider for FakeUnixMetadata {
        fn symlink_metadata(&self, _path: &Path) -> std::io::Result<UnixPathMetadata> {
            Ok(self.observed)
        }

        fn current_uid(&self) -> u32 {
            self.uid
        }
    }

    fn server() -> ServerConfig {
        ServerConfig {
            id: "alpha".into(),
            host: "host.example".into(),
            user: "alice".into(),
            port: "2202".into(),
            key_file: "/keys/id with space".into(),
            ..ServerConfig::default()
        }
    }

    #[test]
    fn openssh_connector_uses_exact_control_socket_and_noninteractive_mode() {
        let arguments = openssh_target_arguments(&server());
        assert_eq!(
            arguments,
            vec![
                "-l",
                "alice",
                "-p",
                "2202",
                "-i",
                "/keys/id with space",
                "-o",
                "IdentitiesOnly=yes",
                "host.example",
            ]
        );
    }

    #[test]
    fn imported_openssh_profile_keeps_its_config_and_alias() {
        let imported = ServerConfig {
            source: "ssh_config".into(),
            host_alias: "cluster".into(),
            ssh_config_path: "/config/custom ssh".into(),
            ..server()
        };
        assert_eq!(
            openssh_target_arguments(&imported),
            vec!["-F", "/config/custom ssh", "cluster"]
        );
    }

    #[test]
    fn plink_connector_is_direct_and_never_contains_a_secret() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("id.ppk");
        fs::write(&key, "PuTTY-User-Key-File-3: ssh-ed25519\n").unwrap();
        let configured = ServerConfig {
            key_file: key.display().to_string(),
            ..server()
        };
        assert_eq!(
            plink_target_arguments(&configured),
            vec![
                "-P",
                "2202",
                "-l",
                "alice",
                "-i",
                key.to_str().unwrap(),
                "host.example",
            ]
        );
    }

    #[test]
    fn plink_omits_openssh_private_keys_and_leaves_authentication_interactive() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("id_ed25519");
        fs::write(&key, "-----BEGIN OPENSSH PRIVATE KEY-----\n").unwrap();
        let configured = ServerConfig {
            key_file: key.display().to_string(),
            ..server()
        };

        assert_eq!(
            plink_target_arguments(&configured),
            vec!["-P", "2202", "-l", "alice", "host.example"]
        );
    }

    #[test]
    fn windows_interactive_sharing_accepts_resolved_direct_config_profiles() {
        assert!(windows_direct_connection_supported(&server()));
        for source in ["ssh_config", "ssh_config_batch"] {
            let configured = ServerConfig {
                host_alias: "cluster".into(),
                source: source.into(),
                ..server()
            };
            assert!(windows_direct_connection_supported(&configured));
        }
        let managed = ServerConfig {
            host_alias: "managed".into(),
            ssh_config_managed: true,
            ..server()
        };
        assert!(windows_direct_connection_supported(&managed));

        for invalid in [
            ServerConfig {
                source: "ssh_config".into(),
                host_alias: "cluster".into(),
                host: String::new(),
                ..server()
            },
            ServerConfig {
                source: "ssh_config".into(),
                host_alias: String::new(),
                ..server()
            },
        ] {
            assert!(!windows_direct_connection_supported(&invalid));
        }
    }

    #[test]
    fn windows_config_profile_overlays_resolved_target_without_a_key() {
        let configured = ServerConfig {
            source: "ssh_config".into(),
            host_alias: "cluster".into(),
            key_file: String::new(),
            ..server()
        };
        assert!(windows_direct_connection_supported(&configured));
        assert_eq!(
            plink_target_arguments(&configured),
            vec!["-P", "2202", "-l", "alice", "host.example"]
        );
    }

    #[test]
    fn windows_interactive_config_rejects_proxy_semantics() {
        assert!(windows_resolved_config_supported(
            &crate::ssh::ResolvedSshConfig::parse("hostname direct.example\nuser alice\n")
        ));
        for proxy in ["proxyjump gateway", "proxycommand ssh gateway -W %h:%p"] {
            assert!(!windows_resolved_config_supported(
                &crate::ssh::ResolvedSshConfig::parse(proxy)
            ));
        }
    }

    #[test]
    fn openssh_login_explicitly_allows_interactive_authentication() {
        let target = vec![
            "-F".into(),
            "/config/with-batch-mode".into(),
            "cluster".into(),
        ];
        let arguments = openssh_login_arguments(Path::new("/state/control.sock"), &target);

        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == [OsString::from("-o"), OsString::from("BatchMode=no")])
        );
        assert!(!arguments.iter().any(|argument| argument == "BatchMode=yes"));
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.to_string_lossy().starts_with("ControlPersist="))
        );
        assert_eq!(
            arguments.last().and_then(|argument| argument.to_str()),
            Some("cluster")
        );
    }

    #[test]
    fn login_command_exposes_exact_openssh_program_and_argv() {
        let command = InteractiveSshLoginCommand::new(
            PathBuf::from("/usr/bin/ssh"),
            openssh_login_arguments(
                Path::new("/state/control with space.sock"),
                &["-l".into(), "alice".into(), "host.example".into()],
            ),
        );

        assert_eq!(command.program(), Path::new("/usr/bin/ssh"));
        assert_eq!(
            command.arguments(),
            &[
                OsString::from("-M"),
                OsString::from("-S"),
                OsString::from("/state/control with space.sock"),
                OsString::from("-o"),
                OsString::from("BatchMode=no"),
                OsString::from("-o"),
                OsString::from("ServerAliveInterval=30"),
                OsString::from("-N"),
                OsString::from("-l"),
                OsString::from("alice"),
                OsString::from("host.example"),
            ]
        );
    }

    #[test]
    fn login_command_exposes_exact_plink_program_and_argv() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("id with space.ppk");
        fs::write(&key, "PuTTY-User-Key-File-3: ssh-ed25519\n").unwrap();
        let configured = ServerConfig {
            key_file: key.display().to_string(),
            ..server()
        };
        let target = plink_target_arguments(&configured);
        let command = InteractiveSshLoginCommand::new(
            PathBuf::from("C:/MountMate/bin/plink.exe"),
            plink_login_arguments(&target),
        );

        assert_eq!(command.program(), Path::new("C:/MountMate/bin/plink.exe"));
        assert_eq!(
            command.arguments(),
            &[
                OsString::from("-ssh"),
                OsString::from("-share"),
                OsString::from("-N"),
                OsString::from("-P"),
                OsString::from("2202"),
                OsString::from("-l"),
                OsString::from("alice"),
                OsString::from("-i"),
                key.into_os_string(),
                OsString::from("host.example"),
            ]
        );
    }

    #[test]
    fn long_state_paths_use_a_short_stable_control_directory() {
        let paths = AppPaths {
            config_dir: PathBuf::from("config"),
            cache_dir: PathBuf::from("cache"),
            state_dir: PathBuf::from("/very-long").join("segment".repeat(30)),
            data_dir: PathBuf::from("data"),
        };
        let first = control_directory(&paths, "0123456789abcdef");
        let second = control_directory(&paths, "0123456789abcdef");
        let state_hash = format!(
            "{:x}",
            Sha256::digest(paths.state_dir.as_os_str().to_string_lossy().as_bytes())
        );
        assert_eq!(first, second);
        assert_eq!(
            first,
            std::env::temp_dir().join(format!("ssh-mountmate-{}", &state_hash[..16]))
        );
    }

    #[cfg(unix)]
    #[test]
    fn control_directory_rejects_a_malicious_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("outside");
        fs::create_dir(&target).unwrap();
        let control = temp.path().join("control");
        symlink(&target, &control).unwrap();

        assert!(ensure_control_directory(&control).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn control_directory_creation_is_owner_only() {
        let temp = tempfile::tempdir().unwrap();
        let control = temp.path().join("control");

        ensure_control_directory(&control).unwrap();
        let metadata = fs::symlink_metadata(&control).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        assert!(validate_control_directory(&control).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn control_directory_rejects_wrong_owner_via_metadata_provider() {
        let metadata = FakeUnixMetadata {
            observed: UnixPathMetadata {
                is_symlink: false,
                is_directory: true,
                is_socket: false,
                uid: 2000,
                mode: 0o700,
            },
            uid: 1000,
        };

        assert!(validate_control_directory_with(&metadata, Path::new("control")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn control_directory_rejects_wrong_type_and_permissive_mode() {
        let wrong_type = FakeUnixMetadata {
            observed: UnixPathMetadata {
                is_symlink: false,
                is_directory: false,
                is_socket: false,
                uid: 1000,
                mode: 0o700,
            },
            uid: 1000,
        };
        assert!(validate_control_directory_with(&wrong_type, Path::new("control")).is_err());

        let permissive = FakeUnixMetadata {
            observed: UnixPathMetadata {
                is_symlink: false,
                is_directory: true,
                is_socket: false,
                uid: 1000,
                mode: 0o755,
            },
            uid: 1000,
        };
        assert!(validate_control_directory_with(&permissive, Path::new("control")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn control_socket_rejects_symlink_wrong_type_and_permissive_mode() {
        let symlink = FakeUnixMetadata {
            observed: UnixPathMetadata {
                is_symlink: true,
                is_directory: false,
                is_socket: false,
                uid: 1000,
                mode: 0o600,
            },
            uid: 1000,
        };
        assert!(validate_control_socket_with(&symlink, Path::new("socket")).is_err());

        let regular_file = FakeUnixMetadata {
            observed: UnixPathMetadata {
                is_symlink: false,
                is_directory: false,
                is_socket: false,
                uid: 1000,
                mode: 0o600,
            },
            uid: 1000,
        };
        assert!(validate_control_socket_with(&regular_file, Path::new("socket")).is_err());

        let permissive = FakeUnixMetadata {
            observed: UnixPathMetadata {
                is_symlink: false,
                is_directory: false,
                is_socket: true,
                uid: 1000,
                mode: 0o606,
            },
            uid: 1000,
        };
        assert!(validate_control_socket_with(&permissive, Path::new("socket")).is_err());

        let wrong_owner = FakeUnixMetadata {
            observed: UnixPathMetadata {
                is_symlink: false,
                is_directory: false,
                is_socket: true,
                uid: 2000,
                mode: 0o600,
            },
            uid: 1000,
        };
        assert!(validate_control_socket_with(&wrong_owner, Path::new("socket")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn control_socket_rejects_a_malicious_symlink_on_disk() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("outside.sock");
        let listener = UnixListener::bind(&target).unwrap();
        let socket = temp.path().join("control.sock");
        symlink(&target, &socket).unwrap();

        assert!(validate_control_socket(&socket).is_err());
        drop(listener);
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_removes_only_a_verified_owned_stale_socket() {
        use std::os::unix::net::UnixListener;

        let temp = tempfile::tempdir().unwrap();
        let control = temp.path().join("control");
        fs::create_dir(&control).unwrap();
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = control.join("stale.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        drop(listener);

        assert!(validate_control_socket(&socket).is_ok());
        cleanup_control_socket(&socket).unwrap();
        assert!(!socket.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_leaves_an_unverified_object_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let object = temp.path().join("stale.sock");
        fs::write(&object, b"not a socket").unwrap();

        assert!(cleanup_control_socket(&object).is_err());
        assert!(object.exists());
    }

    #[cfg(unix)]
    #[test]
    fn readiness_check_reports_success_for_zero_exit() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        assert!(matches!(
            run_readiness_command(&mut command, Duration::from_secs(1)),
            Ok(true)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn readiness_check_reports_false_for_normal_nonzero_exit() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 7"]);
        assert!(matches!(
            run_readiness_command(&mut command, Duration::from_secs(1)),
            Ok(false)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn readiness_check_terminates_a_hung_child_at_the_deadline() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "while :; do :; done"]);
        let timeout = Duration::from_millis(200);

        let started = Instant::now();
        let result = run_readiness_command(&mut command, timeout);
        let elapsed = started.elapsed();

        assert!(matches!(
            result,
            Err(InteractiveSshError::ReadinessTimeout { timeout: observed })
                if observed == timeout
        ));
        assert!(
            elapsed < Duration::from_secs(1),
            "readiness probe exceeded its bounded deadline: {elapsed:?}"
        );
    }

    #[test]
    fn readiness_creation_flags_are_platform_specific() {
        #[cfg(windows)]
        assert_eq!(readiness_creation_flags(), CREATE_NO_WINDOW);
        #[cfg(not(windows))]
        assert_eq!(readiness_creation_flags(), 0);
    }

    #[cfg(not(windows))]
    #[test]
    fn readiness_command_configuration_is_a_noop_without_windows_creation_flags() {
        let mut command = Command::new("true");
        configure_readiness_command(&mut command);
        assert_eq!(command.get_program(), std::ffi::OsStr::new("true"));
    }
}
