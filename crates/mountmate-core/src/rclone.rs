use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use configparser::ini::{Ini, WriteOptions};
use thiserror::Error;

use crate::model::{AuthMethod, ConnectionMethod};
use crate::paths::AppPaths;
use crate::ssh::{ResolvedSshConfig, readable_file, validate_host_alias};
use crate::storage::{FileLock, StorageError, atomic_write};
use crate::{MountBackend, ServerConfig, Settings};

#[derive(Debug, Error)]
pub enum RcloneConfigError {
    #[error("invalid rclone remote name: {0}")]
    InvalidRemoteName(String),
    #[error("invalid value for {field}")]
    InvalidValue { field: &'static str },
    #[error("SSH config resolution is required for an SSH config connection")]
    MissingResolvedSshConfig,
    #[error("interactive SSH requires a verified shared-session connector")]
    MissingInteractiveConnector,
    #[error("invalid rclone config at {path}: {message}")]
    InvalidConfig { path: PathBuf, message: String },
    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcloneRemote {
    pub name: String,
    pub options: Vec<(String, String)>,
}

impl RcloneRemote {
    pub fn for_server(
        server: &ServerConfig,
        resolved: Option<&ResolvedSshConfig>,
        known_hosts: Option<&Path>,
        windows: bool,
    ) -> Result<Self, RcloneConfigError> {
        Self::for_server_with_external_ssh(server, resolved, known_hosts, windows, None)
    }

    pub fn for_server_with_external_ssh(
        server: &ServerConfig,
        resolved: Option<&ResolvedSshConfig>,
        known_hosts: Option<&Path>,
        windows: bool,
        external_ssh_arguments: Option<&[String]>,
    ) -> Result<Self, RcloneConfigError> {
        let name = server.remote_name().to_owned();
        validate_remote_name(&name)?;
        let mut options = vec![
            ("type".into(), "sftp".into()),
            ("shell_type".into(), "unix".into()),
            ("disable_hashcheck".into(), "true".into()),
        ];
        if server.connection_method != ConnectionMethod::Native {
            let arguments = match server.connection_method {
                ConnectionMethod::Interactive => external_ssh_arguments
                    .filter(|arguments| !arguments.is_empty())
                    .ok_or(RcloneConfigError::MissingInteractiveConnector)?,
                ConnectionMethod::Openssh => {
                    if let Some(arguments) = external_ssh_arguments {
                        arguments
                    } else {
                        let ssh = openssh_command(server, windows)?;
                        options.push(("ssh".into(), ssh));
                        return Ok(Self { name, options });
                    }
                }
                ConnectionMethod::Native => unreachable!(),
            };
            for argument in arguments {
                validate_scalar(argument, "external SSH argument")?;
            }
            let ssh = arguments
                .iter()
                .map(|argument| quote_command_argument(argument, windows))
                .collect::<Vec<_>>()
                .join(" ");
            options.push(("ssh".into(), ssh));
            return Ok(Self { name, options });
        }

        let ssh_config_connection = server.mode == "ssh_config";
        let resolved = if ssh_config_connection {
            Some(resolved.ok_or(RcloneConfigError::MissingResolvedSshConfig)?)
        } else {
            None
        };
        let host = resolved
            .map(|config| config.first("hostname", &server.host))
            .unwrap_or(&server.host);
        let default_user = default_username();
        let user = resolved
            .map(|config| config.first("user", default_user.as_str()))
            .unwrap_or(&server.user);
        let port = resolved
            .map(|config| config.first("port", &server.port))
            .unwrap_or(&server.port);
        validate_scalar(host, "host")?;
        validate_scalar(user, "user")?;
        validate_port(port)?;
        options.extend([
            ("host".into(), host.to_owned()),
            ("user".into(), user.to_owned()),
            ("port".into(), port.to_owned()),
        ]);

        if ssh_config_connection {
            if let Some(key_file) =
                resolved.and_then(|config| config.first_existing_path("identityfile"))
            {
                options.push(("key_file".into(), key_file.display().to_string()));
                if !server.key_pass_obscured.is_empty() {
                    validate_scalar(&server.key_pass_obscured, "key passphrase")?;
                    options.push(("key_file_pass".into(), server.key_pass_obscured.clone()));
                }
            } else {
                options.push(("key_use_agent".into(), "true".into()));
            }
        } else {
            match server.auth {
                AuthMethod::Password => {
                    validate_scalar(&server.password_obscured, "password")?;
                    options.push(("pass".into(), server.password_obscured.clone()));
                }
                AuthMethod::Key if !server.key_file.is_empty() => {
                    validate_scalar(&server.key_file, "key file")?;
                    options.push(("key_file".into(), server.key_file.clone()));
                    if !server.key_pass_obscured.is_empty() {
                        validate_scalar(&server.key_pass_obscured, "key passphrase")?;
                        options.push(("key_file_pass".into(), server.key_pass_obscured.clone()));
                    }
                }
                AuthMethod::Key => {
                    options.push(("key_use_agent".into(), "true".into()));
                }
            }
        }
        if let Some(path) = known_hosts.and_then(readable_file) {
            options.push(("known_hosts_file".into(), path.display().to_string()));
        }
        Ok(Self { name, options })
    }

    pub fn wrap_external_ssh(
        &mut self,
        proxy: &Path,
        windows: bool,
    ) -> Result<(), RcloneConfigError> {
        let Some((_, ssh)) = self.options.iter_mut().find(|(key, _)| key == "ssh") else {
            return Ok(());
        };
        let proxy = proxy.display().to_string();
        validate_scalar(&proxy, "SSH connector proxy")?;
        *ssh = format!(
            "{} --run-ssh-connector {}",
            quote_command_argument(&proxy, windows),
            ssh
        );
        Ok(())
    }
}

pub fn write_rclone_remote(
    paths: &AppPaths,
    remote: &RcloneRemote,
) -> Result<(), RcloneConfigError> {
    validate_remote_name(&remote.name)?;
    let _lock = FileLock::acquire(&paths.rclone_config_lock(), Duration::from_secs(180))?;
    let path = paths.rclone_config();
    let mut defaults = Ini::new_cs().defaults();
    defaults.enable_inline_comments = false;
    let mut config = Ini::new_from_defaults(defaults);
    if path.exists() {
        let content = fs::read_to_string(&path).map_err(|source| StorageError::Io {
            path: path.clone(),
            source,
        })?;
        config
            .read(content)
            .map_err(|message| RcloneConfigError::InvalidConfig {
                path: path.clone(),
                message,
            })?;
    }
    config.remove_section(&remote.name);
    for (key, value) in &remote.options {
        validate_ini_name(key, "rclone option")?;
        validate_scalar(value, "rclone option value")?;
        config.set(&remote.name, key, Some(value.clone()));
    }
    let write_options = WriteOptions::new_with_params(true, 4, 1);
    atomic_write(&path, config.pretty_writes(&write_options).as_bytes())?;
    Ok(())
}

pub fn clear_rclone_remote_secrets(
    paths: &AppPaths,
    remote_name: &str,
) -> Result<(), RcloneConfigError> {
    validate_remote_name(remote_name)?;
    let _lock = FileLock::acquire(&paths.rclone_config_lock(), Duration::from_secs(180))?;
    let path = paths.rclone_config();
    if !path.exists() {
        return Ok(());
    }
    let mut defaults = Ini::new_cs().defaults();
    defaults.enable_inline_comments = false;
    let mut config = Ini::new_from_defaults(defaults);
    let content = fs::read_to_string(&path).map_err(|source| StorageError::Io {
        path: path.clone(),
        source,
    })?;
    config
        .read(content)
        .map_err(|message| RcloneConfigError::InvalidConfig {
            path: path.clone(),
            message,
        })?;
    config.remove_key(remote_name, "pass");
    config.remove_key(remote_name, "key_file_pass");
    let write_options = WriteOptions::new_with_params(true, 4, 1);
    atomic_write(&path, config.pretty_writes(&write_options).as_bytes())?;
    Ok(())
}

fn validate_remote_name(value: &str) -> Result<(), RcloneConfigError> {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        Ok(())
    } else {
        Err(RcloneConfigError::InvalidRemoteName(value.to_owned()))
    }
}

fn validate_ini_name(value: &str, field: &'static str) -> Result<(), RcloneConfigError> {
    if !value.is_empty() && !value.contains(['\r', '\n', '[', ']', '=', ':']) {
        Ok(())
    } else {
        Err(RcloneConfigError::InvalidValue { field })
    }
}

fn validate_scalar(value: &str, field: &'static str) -> Result<(), RcloneConfigError> {
    if !value.is_empty() && !value.contains(['\r', '\n', '\0']) {
        Ok(())
    } else {
        Err(RcloneConfigError::InvalidValue { field })
    }
}

fn validate_port(value: &str) -> Result<(), RcloneConfigError> {
    match value.parse::<u16>() {
        Ok(port) if port > 0 => Ok(()),
        _ => Err(RcloneConfigError::InvalidValue { field: "port" }),
    }
}

fn default_username() -> String {
    env::var("USERNAME")
        .or_else(|_| env::var("USER"))
        .unwrap_or_default()
}

fn openssh_command(server: &ServerConfig, windows: bool) -> Result<String, RcloneConfigError> {
    let mut arguments = vec!["ssh".to_owned(), "-o".into(), "BatchMode=yes".into()];
    if (server.source == "ssh_config" || server.ssh_config_managed) && !server.host_alias.is_empty()
    {
        if server.source == "ssh_config" && !server.ssh_config_path.trim().is_empty() {
            validate_scalar(&server.ssh_config_path, "SSH config path")?;
            arguments.extend(["-F".into(), server.ssh_config_path.clone()]);
        }
        validate_host_alias(&server.host_alias)
            .map_err(|_| RcloneConfigError::InvalidValue { field: "SSH host" })?;
        arguments.push(server.host_alias.clone());
    } else {
        validate_scalar(&server.host, "host")?;
        validate_port(&server.port)?;
        if !server.user.is_empty() {
            validate_scalar(&server.user, "user")?;
            arguments.extend(["-l".into(), server.user.clone()]);
        }
        arguments.extend(["-p".into(), server.port.clone()]);
        if !server.key_file.is_empty() {
            validate_scalar(&server.key_file, "key file")?;
            arguments.extend([
                "-i".into(),
                server.key_file.clone(),
                "-o".into(),
                "IdentitiesOnly=yes".into(),
            ]);
        }
        arguments.push(server.host.clone());
    }
    Ok(arguments
        .iter()
        .map(|argument| quote_command_argument(argument, windows))
        .collect::<Vec<_>>()
        .join(" "))
}

fn quote_command_argument(value: &str, windows: bool) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "._-/:=~".contains(ch))
    {
        return value.to_owned();
    }
    if windows {
        let mut quoted = String::from("\"");
        let mut backslashes = 0;
        for ch in value.chars() {
            if ch == '\\' {
                backslashes += 1;
            } else if ch == '"' {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            } else {
                quoted.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
        quoted.push_str(&"\\".repeat(backslashes * 2));
        quoted.push('"');
        quoted
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[derive(Debug, Clone)]
pub struct MountCommand<'a> {
    pub rclone: &'a Path,
    pub config: &'a Path,
    pub server: &'a ServerConfig,
    pub settings: &'a Settings,
    pub remote: &'a str,
    pub mountpoint: &'a Path,
    pub cache_dir: &'a Path,
    pub log_path: &'a Path,
    pub rc_addr: &'a str,
    pub rc_user: &'a str,
    pub rc_pass: &'a str,
    pub platform: MountPlatform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountPlatform {
    Windows,
    Macos,
    Linux,
    Other,
}

impl MountPlatform {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    pub const fn effective_backend(self, selected: MountBackend) -> MountBackend {
        if matches!(self, Self::Macos) && matches!(selected, MountBackend::Nfs) {
            MountBackend::Nfs
        } else {
            MountBackend::Fuse
        }
    }
}

impl MountCommand<'_> {
    pub fn build(&self) -> Vec<String> {
        let cache_mode = self.server.effective_cache_mode(self.settings);
        let backend = self
            .platform
            .effective_backend(self.settings.macos_mount_backend);
        let mut command = vec![
            self.rclone.display().to_string(),
            "--config".into(),
            self.config.display().to_string(),
            "--rc".into(),
            "--rc-addr".into(),
            self.rc_addr.into(),
            "--rc-user".into(),
            self.rc_user.into(),
            "--rc-pass".into(),
            self.rc_pass.into(),
            match backend {
                MountBackend::Fuse => "mount".into(),
                MountBackend::Nfs => "nfsmount".into(),
            },
            self.remote.into(),
            self.mountpoint.display().to_string(),
            "--vfs-fast-fingerprint".into(),
            "--links".into(),
            "--cache-dir".into(),
            self.cache_dir.display().to_string(),
            "--log-file".into(),
            self.log_path.display().to_string(),
            "--volname".into(),
            self.server.display_name().into(),
            "--vfs-cache-mode".into(),
            cache_mode.into(),
            "--transfers".into(),
            self.settings.vfs_upload_transfers.to_string(),
        ];
        if backend == MountBackend::Nfs {
            command.extend(["--addr".into(), "127.0.0.1:0".into()]);
        }
        push_option(
            &mut command,
            "--vfs-cache-max-size",
            &self.settings.vfs_cache_max_size,
        );
        push_option(
            &mut command,
            "--vfs-cache-max-age",
            &self.settings.vfs_cache_max_age,
        );
        push_option(
            &mut command,
            "--vfs-cache-min-free-space",
            &self.settings.vfs_cache_min_free_space,
        );
        if matches!(cache_mode, "writes" | "full") {
            push_option(
                &mut command,
                "--vfs-write-back",
                &self.settings.vfs_write_back,
            );
        }
        push_option(
            &mut command,
            "--dir-cache-time",
            &self.settings.dir_cache_time,
        );
        push_option(&mut command, "--buffer-size", &self.settings.buffer_size);
        if self.platform == MountPlatform::Windows
            && self.server.network_mode
            && is_windows_drive(self.mountpoint)
        {
            command.push("--network-mode".into());
        }
        command
    }
}

fn push_option(command: &mut Vec<String>, option: &str, value: &str) {
    if !value.is_empty() {
        command.extend([option.into(), value.into()]);
    }
}

pub fn is_windows_drive(path: &Path) -> bool {
    let value = path.as_os_str().to_string_lossy();
    value.len() == 2 && value.as_bytes()[0].is_ascii_alphabetic() && value.ends_with(':')
}

pub fn normalize_explorer_refresh_path(value: &str, windows: bool) -> String {
    if !windows {
        return value.to_owned();
    }
    let mut value = value.to_owned();
    if value.starts_with('"') {
        value.remove(0);
    }
    if value.ends_with('"') {
        value.pop();
        value.push('\\');
    }
    value
}

pub fn normalize_refresh_relative_path(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let normalized = normalized.trim_matches('/');
    if matches!(normalized, "" | "." | "\"") {
        String::new()
    } else {
        normalized
            .split('/')
            .filter(|component| !component.is_empty() && *component != ".")
            .collect::<Vec<_>>()
            .join("/")
    }
}

pub fn cache_directory(settings: &Settings, server: &ServerConfig) -> PathBuf {
    settings.cache_root.join(server.remote_name())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn app_paths(root: &Path) -> AppPaths {
        AppPaths {
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            data_dir: root.join("data"),
        }
    }

    fn command(cache_mode: &str) -> Vec<String> {
        let server = ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            cache_mode: cache_mode.into(),
            ..ServerConfig::default()
        };
        MountCommand {
            rclone: Path::new("rclone"),
            config: Path::new("rclone.conf"),
            server: &server,
            settings: &Settings::default(),
            remote: "alpha:",
            mountpoint: Path::new("R:"),
            cache_dir: Path::new("cache"),
            log_path: Path::new("alpha.log"),
            rc_addr: "127.0.0.1:1234",
            rc_user: "mountmate",
            rc_pass: "secret",
            platform: MountPlatform::Windows,
        }
        .build()
    }

    #[test]
    fn mount_command_keeps_reliability_flags_and_defaults() {
        let command = command("");
        assert!(command.contains(&"--links".into()));
        assert!(!command.contains(&"--rc-no-auth".into()));
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--rc-user", "mountmate"])
        );
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--rc-pass", "secret"])
        );
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--vfs-cache-mode", "full"])
        );
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--vfs-cache-max-age", "30m"])
        );
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--vfs-write-back", "5s"])
        );
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--dir-cache-time", "5m"])
        );
        assert!(command.windows(2).any(|item| item == ["--transfers", "4"]));
    }

    #[test]
    fn mount_command_uses_configured_upload_transfer_limit() {
        let server = ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            ..ServerConfig::default()
        };
        let settings = Settings {
            vfs_upload_transfers: 12,
            ..Settings::default()
        };
        for platform in [MountPlatform::Linux, MountPlatform::Windows] {
            let command = MountCommand {
                rclone: Path::new("rclone"),
                config: Path::new("rclone.conf"),
                server: &server,
                settings: &settings,
                remote: "alpha:",
                mountpoint: Path::new("R:"),
                cache_dir: Path::new("cache"),
                log_path: Path::new("alpha.log"),
                rc_addr: "127.0.0.1:1234",
                rc_user: "mountmate",
                rc_pass: "secret",
                platform,
            }
            .build();
            assert_eq!(
                command
                    .windows(2)
                    .filter(|item| item[0] == "--transfers")
                    .count(),
                1
            );
            assert!(command.windows(2).any(|item| item == ["--transfers", "12"]));
        }
    }

    #[test]
    fn write_back_is_not_passed_for_non_write_cache_modes() {
        let command = command("off");
        assert!(!command.contains(&"--vfs-write-back".into()));
    }

    #[test]
    fn mount_backend_command_is_platform_scoped() {
        let server = ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            ..ServerConfig::default()
        };
        for (platform, selected, expected) in [
            (MountPlatform::Macos, MountBackend::Fuse, "mount"),
            (MountPlatform::Macos, MountBackend::Nfs, "nfsmount"),
            (MountPlatform::Windows, MountBackend::Nfs, "mount"),
            (MountPlatform::Linux, MountBackend::Nfs, "mount"),
        ] {
            let settings = Settings {
                macos_mount_backend: selected,
                ..Settings::default()
            };
            let command = MountCommand {
                rclone: Path::new("rclone"),
                config: Path::new("rclone.conf"),
                server: &server,
                settings: &settings,
                remote: "alpha:",
                mountpoint: Path::new("/mnt/alpha"),
                cache_dir: Path::new("cache"),
                log_path: Path::new("alpha.log"),
                rc_addr: "127.0.0.1:1234",
                rc_user: "mountmate",
                rc_pass: "secret",
                platform,
            }
            .build();
            assert_eq!(command[10], expected);
        }
    }

    #[test]
    fn macos_nfs_keeps_rc_cache_writeback_and_log_arguments_on_loopback() {
        let server = ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            ..ServerConfig::default()
        };
        let settings = Settings {
            macos_mount_backend: MountBackend::Nfs,
            ..Settings::default()
        };
        let command = MountCommand {
            rclone: Path::new("rclone"),
            config: Path::new("rclone.conf"),
            server: &server,
            settings: &settings,
            remote: "alpha:",
            mountpoint: Path::new("/mnt/alpha"),
            cache_dir: Path::new("cache"),
            log_path: Path::new("alpha.log"),
            rc_addr: "127.0.0.1:1234",
            rc_user: "mountmate",
            rc_pass: "secret",
            platform: MountPlatform::Macos,
        }
        .build();
        for expected in [
            ["--rc-addr", "127.0.0.1:1234"],
            ["--cache-dir", "cache"],
            ["--log-file", "alpha.log"],
            ["--vfs-cache-mode", "full"],
            ["--vfs-write-back", "5s"],
            ["--dir-cache-time", "5m"],
            ["--addr", "127.0.0.1:0"],
        ] {
            assert!(command.windows(2).any(|pair| pair == expected));
        }
        assert!(command.contains(&"--links".into()));
        assert!(command.contains(&"--volname".into()));
        assert!(!command.iter().any(|value| value == "0.0.0.0"));
    }

    #[test]
    fn explorer_drive_root_repairs_trailing_quote() {
        assert_eq!(normalize_explorer_refresh_path("Y:\"", true), "Y:\\");
        assert_eq!(
            normalize_explorer_refresh_path("\"C:\\Mount Folder\"", true),
            "C:\\Mount Folder\\"
        );
        assert_eq!(normalize_refresh_relative_path("\""), "");
        assert_eq!(
            normalize_refresh_relative_path("Folder\\Child\\."),
            "Folder/Child"
        );
    }

    #[test]
    fn native_remote_keeps_obscured_password_and_readable_known_hosts() {
        let temp = tempdir().unwrap();
        let known_hosts = temp.path().join("known_hosts");
        fs::write(&known_hosts, "host ssh-ed25519 AAAA\n").unwrap();
        let server = ServerConfig {
            id: "alpha".into(),
            host: "alpha.example".into(),
            user: "researcher".into(),
            port: "12022".into(),
            auth: AuthMethod::Password,
            password_obscured: "obscured-value".into(),
            ..ServerConfig::default()
        };

        let remote = RcloneRemote::for_server(&server, None, Some(&known_hosts), false).unwrap();

        assert!(
            remote
                .options
                .contains(&("pass".into(), "obscured-value".into()))
        );
        assert!(
            remote
                .options
                .contains(&("known_hosts_file".into(), known_hosts.display().to_string()))
        );
        assert!(!remote.options.iter().any(|(key, _)| key == "key_file"));
    }

    #[test]
    fn unreadable_known_hosts_is_never_written_to_remote() {
        let temp = tempdir().unwrap();
        let not_a_file = temp.path().join("known_hosts");
        fs::create_dir(&not_a_file).unwrap();
        let server = ServerConfig {
            id: "alpha".into(),
            host: "alpha.example".into(),
            user: "researcher".into(),
            ..ServerConfig::default()
        };

        let remote = RcloneRemote::for_server(&server, None, Some(&not_a_file), false).unwrap();

        assert!(
            !remote
                .options
                .iter()
                .any(|(key, _)| key == "known_hosts_file")
        );
    }

    #[test]
    fn ssh_config_remote_uses_resolved_values_and_existing_identity() {
        let temp = tempdir().unwrap();
        let identity = temp.path().join("id key");
        fs::write(&identity, "PRIVATE KEY").unwrap();
        let resolved = ResolvedSshConfig::parse(&format!(
            "hostname c1.example\nuser researcher\nport 12022\nidentityfile \"{}\"\n",
            identity.display()
        ));
        let server = ServerConfig {
            id: "internal-id".into(),
            mode: "ssh_config".into(),
            host_alias: "cluster".into(),
            ..ServerConfig::default()
        };

        let remote = RcloneRemote::for_server(&server, Some(&resolved), None, false).unwrap();

        assert_eq!(remote.name, "cluster");
        assert!(
            remote
                .options
                .contains(&("host".into(), "c1.example".into()))
        );
        assert!(
            remote
                .options
                .contains(&("key_file".into(), identity.display().to_string()))
        );
        assert!(!remote.options.iter().any(|(key, _)| key == "key_use_agent"));
    }

    #[test]
    fn ssh_config_remote_keeps_key_passphrase_after_server_json_round_trip() {
        let temp = tempdir().unwrap();
        let identity = temp.path().join("id key");
        fs::write(&identity, "PRIVATE KEY").unwrap();
        let resolved = ResolvedSshConfig::parse(&format!(
            "hostname c1.example\nuser researcher\nport 12022\nidentityfile \"{}\"\n",
            identity.display()
        ));
        let server = ServerConfig {
            id: "internal-id".into(),
            mode: "ssh_config".into(),
            host_alias: "cluster".into(),
            key_pass_obscured: "obscured-key-passphrase".into(),
            ..ServerConfig::default()
        };
        let server: ServerConfig =
            serde_json::from_value(serde_json::to_value(server).unwrap()).unwrap();

        let remote = RcloneRemote::for_server(&server, Some(&resolved), None, false).unwrap();

        assert!(
            remote
                .options
                .contains(&("key_file_pass".into(), "obscured-key-passphrase".into()))
        );
    }

    #[test]
    fn openssh_remote_uses_quoted_command_without_saved_secrets() {
        let server = ServerConfig {
            id: "alpha".into(),
            host: "alpha.example".into(),
            user: "researcher".into(),
            port: "22".into(),
            key_file: "/home/user/key file".into(),
            password_obscured: "must-not-appear".into(),
            connection_method: ConnectionMethod::Openssh,
            ..ServerConfig::default()
        };

        let remote = RcloneRemote::for_server(&server, None, None, false).unwrap();
        let command = remote.options.iter().find(|(key, _)| key == "ssh").unwrap();

        assert!(command.1.contains("'/home/user/key file'"));
        assert!(!remote.options.iter().any(|(key, _)| key == "pass"));
        assert!(!command.1.contains("must-not-appear"));
    }

    #[test]
    fn imported_openssh_profile_keeps_its_custom_config_file() {
        let server = ServerConfig {
            id: "cluster".into(),
            source: "ssh_config".into(),
            host_alias: "cluster".into(),
            connection_method: ConnectionMethod::Openssh,
            ssh_config_path: "/home/user/ssh configs/research".into(),
            ..ServerConfig::default()
        };

        let remote = RcloneRemote::for_server(&server, None, None, false).unwrap();
        let command = &remote
            .options
            .iter()
            .find(|(key, _)| key == "ssh")
            .unwrap()
            .1;
        assert!(command.contains("-F '/home/user/ssh configs/research'"));
        assert!(command.ends_with("cluster"));
    }

    #[test]
    fn interactive_remote_uses_only_the_verified_shared_connector() {
        let server = ServerConfig {
            id: "interactive".into(),
            connection_method: ConnectionMethod::Interactive,
            password_obscured: "must-not-appear".into(),
            key_pass_obscured: "must-not-appear".into(),
            ..ServerConfig::default()
        };
        let connector = vec![
            "C:/Program Files/SSH MountMate/plink.exe".into(),
            "-batch".into(),
            "-share".into(),
            "host.example".into(),
        ];
        let remote =
            RcloneRemote::for_server_with_external_ssh(&server, None, None, true, Some(&connector))
                .unwrap();
        let ssh = remote
            .options
            .iter()
            .find(|(key, _)| key == "ssh")
            .map(|(_, value)| value.as_str())
            .unwrap();
        assert!(ssh.contains("\"C:/Program Files/SSH MountMate/plink.exe\""));
        assert!(ssh.contains("-batch -share host.example"));
        assert!(
            !remote
                .options
                .iter()
                .any(|(key, _)| { matches!(key.as_str(), "pass" | "key_file" | "key_file_pass") })
        );
        assert!(!format!("{remote:?}").contains("must-not-appear"));
    }

    #[test]
    fn interactive_ssh_config_remote_does_not_collide_with_openssh_remote() {
        let openssh = ServerConfig {
            id: "openssh-profile".into(),
            mode: "ssh_config".into(),
            source: "ssh_config".into(),
            host_alias: "cluster-login".into(),
            connection_method: ConnectionMethod::Openssh,
            ..ServerConfig::default()
        };
        let interactive = ServerConfig {
            id: "interactive-profile".into(),
            mode: "ssh_config".into(),
            source: "ssh_config".into(),
            host_alias: "cluster-login".into(),
            connection_method: ConnectionMethod::Interactive,
            ..ServerConfig::default()
        };
        let connector = vec!["ssh".into(), "-o".into(), "ControlMaster=no".into()];

        let openssh_remote = RcloneRemote::for_server(&openssh, None, None, false).unwrap();
        let interactive_remote = RcloneRemote::for_server_with_external_ssh(
            &interactive,
            None,
            None,
            false,
            Some(&connector),
        )
        .unwrap();

        assert_eq!(openssh_remote.name, "cluster-login");
        assert_eq!(interactive_remote.name, "interactive-profile");
        assert_ne!(openssh_remote.name, interactive_remote.name);
    }

    #[test]
    fn interactive_remote_never_falls_back_to_a_normal_ssh_process() {
        let server = ServerConfig {
            id: "interactive".into(),
            connection_method: ConnectionMethod::Interactive,
            ..ServerConfig::default()
        };

        assert!(matches!(
            RcloneRemote::for_server(&server, None, None, false),
            Err(RcloneConfigError::MissingInteractiveConnector)
        ));
    }

    #[test]
    fn interactive_remote_rejects_config_line_injection_in_connector_arguments() {
        let server = ServerConfig {
            id: "interactive".into(),
            connection_method: ConnectionMethod::Interactive,
            ..ServerConfig::default()
        };
        let connector = vec!["ssh".into(), "host\npass = leaked".into()];

        assert!(matches!(
            RcloneRemote::for_server_with_external_ssh(
                &server,
                None,
                None,
                false,
                Some(&connector)
            ),
            Err(RcloneConfigError::InvalidValue {
                field: "external SSH argument"
            })
        ));
    }

    #[test]
    fn windows_external_ssh_is_wrapped_by_the_app_connector_proxy() {
        let mut remote = RcloneRemote {
            name: "alpha".into(),
            options: vec![(
                "ssh".into(),
                "\"C:\\Program Files\\PuTTY\\plink.exe\" -batch host".into(),
            )],
        };
        remote
            .wrap_external_ssh(Path::new("C:\\Apps\\SSH MountMate.exe"), true)
            .unwrap();
        assert_eq!(
            remote.options[0].1,
            "\"C:\\Apps\\SSH MountMate.exe\" --run-ssh-connector \"C:\\Program Files\\PuTTY\\plink.exe\" -batch host"
        );
    }

    #[test]
    fn config_update_preserves_other_secrets_and_removes_stale_options() {
        let temp = tempdir().unwrap();
        let paths = app_paths(temp.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(
            paths.rclone_config(),
            "[other]\npass = preserved-secret\n\n[alpha]\nknown_hosts_file = stale\npass = stale\n",
        )
        .unwrap();
        let remote = RcloneRemote {
            name: "alpha".into(),
            options: vec![
                ("type".into(), "sftp".into()),
                ("ssh".into(), "ssh -o BatchMode=yes alpha".into()),
            ],
        };

        write_rclone_remote(&paths, &remote).unwrap();

        let content = fs::read_to_string(paths.rclone_config()).unwrap();
        let mut parsed = Ini::new_cs();
        parsed.read(content).unwrap();
        assert_eq!(parsed.get("other", "pass"), Some("preserved-secret".into()));
        assert_eq!(parsed.get("alpha", "known_hosts_file"), None);
        assert_eq!(parsed.get("alpha", "pass"), None);
        assert_eq!(
            parsed.get("alpha", "ssh"),
            Some("ssh -o BatchMode=yes alpha".into())
        );
    }

    #[test]
    fn credential_cleanup_removes_only_persisted_remote_secrets() {
        let temp = tempdir().unwrap();
        let paths = app_paths(temp.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(
            paths.rclone_config(),
            "[alpha]\ntype = sftp\nhost = example.test\npass = obscured\nkey_file_pass = obscured-key\n\n[other]\ntype = sftp\npass = keep-me\n",
        )
        .unwrap();
        clear_rclone_remote_secrets(&paths, "alpha").unwrap();
        let content = fs::read_to_string(paths.rclone_config()).unwrap();
        let mut config = Ini::new_cs();
        config.read(content).unwrap();
        assert_eq!(config.get("alpha", "host").as_deref(), Some("example.test"));
        assert_eq!(config.get("alpha", "pass"), None);
        assert_eq!(config.get("alpha", "key_file_pass"), None);
        assert_eq!(config.get("other", "pass").as_deref(), Some("keep-me"));
    }
}
