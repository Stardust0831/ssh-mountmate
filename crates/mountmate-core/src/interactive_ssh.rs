use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::ServerConfig;
use crate::paths::AppPaths;
use crate::plink_binary::resolve_plink;
use crate::rclone_binary::{RcloneBinaryError, find_system_executable};
use crate::storage::atomic_write;

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

#[derive(Debug, Error)]
pub enum InteractiveSshError {
    #[error(
        "interactive SSH on Windows supports direct manual connections only; SSH-config profiles and proxy translation are not supported"
    )]
    UnsupportedWindowsSshConfig,
    #[error("verified Plink is missing from this Windows package")]
    PlinkMissing,
    #[error("OpenSSH was not found")]
    OpenSshMissing,
    #[error("no supported terminal application was found")]
    TerminalMissing,
    #[error(
        "interactive login is required; a login window was opened. Complete authentication, keep that window open, then mount again"
    )]
    LoginStarted,
    #[error("interactive login is required; start the shared SSH login and mount again")]
    SessionMissing,
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
    login: LoginCommand,
    #[cfg(unix)]
    control_dir: PathBuf,
    #[cfg(unix)]
    control_socket: PathBuf,
}

#[derive(Debug, Clone)]
enum LoginCommand {
    Windows {
        program: PathBuf,
        arguments: Vec<String>,
    },
    Macos {
        script: PathBuf,
        arguments: Vec<String>,
    },
    Unix {
        program: PathBuf,
        arguments: Vec<String>,
        script: PathBuf,
        login_arguments: Vec<String>,
    },
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
        #[cfg(unix)]
        if self.validate_control_paths().is_err() {
            return false;
        }
        let ready = Command::new(&self.check_program)
            .args(&self.check_arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        #[cfg(unix)]
        if !ready {
            // A verified socket that no longer answers is stale. Never remove
            // an object unless it passes the same identity checks used above.
            let _ = cleanup_control_socket(&self.control_socket);
        }
        ready
    }

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

    pub fn start_login(&self) -> Result<(), InteractiveSshError> {
        #[cfg(unix)]
        self.validate_control_paths()?;
        match &self.login {
            LoginCommand::Windows { program, arguments } => {
                let mut command = Command::new(program);
                command.args(arguments);
                #[cfg(windows)]
                command.creation_flags(CREATE_NEW_CONSOLE);
                command
                    .spawn()
                    .map(|_| ())
                    .map_err(|error| InteractiveSshError::Process(error.to_string()))
            }
            LoginCommand::Macos { script, arguments } => {
                write_login_script(script, arguments)?;
                Command::new("open")
                    .arg(script)
                    .spawn()
                    .map(|_| ())
                    .map_err(|error| InteractiveSshError::Process(error.to_string()))
            }
            LoginCommand::Unix {
                program,
                arguments,
                script,
                login_arguments,
            } => {
                write_login_script(script, login_arguments)?;
                Command::new(program)
                    .args(arguments)
                    .spawn()
                    .map(|_| ())
                    .map_err(|error| InteractiveSshError::Process(error.to_string()))
            }
        }
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
        let login_arguments = openssh_login_arguments(&ssh, &control, &target);
        let script = control_dir.join(if cfg!(target_os = "macos") {
            format!("{}.command", &id_hash[..16])
        } else {
            format!("{}.sh", &id_hash[..16])
        });
        let login = if cfg!(target_os = "macos") {
            LoginCommand::Macos {
                script,
                arguments: login_arguments,
            }
        } else {
            let terminal_arguments = vec![script.display().to_string()];
            let (terminal, arguments) = terminal_command(&terminal_arguments)?;
            LoginCommand::Unix {
                program: terminal,
                arguments,
                script,
                login_arguments,
            }
        };
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
        let mut login_arguments = vec!["-ssh".into(), "-share".into(), "-N".into()];
        login_arguments.extend(target);
        Ok(Self {
            connector,
            check_program: plink.path.clone(),
            check_arguments,
            login: LoginCommand::Windows {
                program: plink.path,
                arguments: login_arguments,
            },
            #[cfg(unix)]
            control_dir: PathBuf::new(),
            #[cfg(unix)]
            control_socket: PathBuf::new(),
        })
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
    if !server.key_file.is_empty() {
        arguments.extend(["-i".into(), server.key_file.clone()]);
    }
    arguments.push(server.host.clone());
    arguments
}

fn windows_direct_connection_supported(server: &ServerConfig) -> bool {
    !matches!(server.source.as_str(), "ssh_config" | "ssh_config_batch")
}

fn openssh_login_arguments(ssh: &Path, control: &Path, target: &[String]) -> Vec<String> {
    let mut arguments = vec![
        ssh.display().to_string(),
        "-M".into(),
        "-S".into(),
        control.display().to_string(),
        "-o".into(),
        "BatchMode=no".into(),
        "-o".into(),
        "ControlPersist=10m".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-N".into(),
    ];
    arguments.extend_from_slice(target);
    arguments
}

fn terminal_command(login: &[String]) -> Result<(PathBuf, Vec<String>), InteractiveSshError> {
    for (name, separator) in [
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xterm", "-e"),
    ] {
        if let Some(program) = find_system_executable(name) {
            let arguments = terminal_launch_arguments(&program, separator, login);
            return Ok((program, arguments));
        }
    }
    Err(InteractiveSshError::TerminalMissing)
}

fn terminal_launch_arguments(program: &Path, separator: &str, login: &[String]) -> Vec<String> {
    let resolved = fs::canonicalize(program).unwrap_or_else(|_| program.to_owned());
    let terminal_name = resolved
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut arguments = Vec::new();
    // Debian alternatives can resolve to lxterm without its legacy bitmap font installed.
    if matches!(terminal_name.as_str(), "xterm" | "lxterm") {
        arguments.extend(["-fa".into(), "Monospace".into()]);
    }
    arguments.push(separator.into());
    arguments.extend_from_slice(login);
    arguments
}

fn write_login_script(path: &Path, arguments: &[String]) -> Result<(), InteractiveSshError> {
    let command = arguments
        .iter()
        .map(|argument| format!("'{}'", argument.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' 'SSH MountMate interactive login. Keep this window open while mounts use the shared session.'\n{command}\nstatus=$?\nif [ \"$status\" -ne 0 ] && [ -t 0 ]; then\n  printf '%s\\n' 'Interactive SSH login failed. Review the message above, then press Enter to close.'\n  read -r _\nfi\nprintf '%s\\n' 'Shared SSH session ended. You may close this window.'\nexit $status\n"
    );
    atomic_write(path, script.as_bytes())
        .map_err(|error| InteractiveSshError::Process(error.to_string()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| InteractiveSshError::Process(error.to_string()))?;
    Ok(())
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
        assert_eq!(
            plink_target_arguments(&server()),
            vec![
                "-P",
                "2202",
                "-l",
                "alice",
                "-i",
                "/keys/id with space",
                "host.example",
            ]
        );
    }

    #[test]
    fn windows_interactive_sharing_accepts_manual_and_rejects_ssh_config_sources() {
        assert!(windows_direct_connection_supported(&server()));
        for source in ["ssh_config", "ssh_config_batch"] {
            let configured = ServerConfig {
                source: source.into(),
                ..server()
            };
            assert!(!windows_direct_connection_supported(&configured));
        }
    }

    #[test]
    fn openssh_login_explicitly_allows_interactive_authentication() {
        let target = vec![
            "-F".into(),
            "/config/with-batch-mode".into(),
            "cluster".into(),
        ];
        let arguments = openssh_login_arguments(
            Path::new("/usr/bin/ssh"),
            Path::new("/state/control.sock"),
            &target,
        );

        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-o", "BatchMode=no"])
        );
        assert!(!arguments.iter().any(|argument| argument == "BatchMode=yes"));
        assert_eq!(arguments.last().map(String::as_str), Some("cluster"));
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
    fn xterm_alias_uses_a_scalable_font_before_the_login_command() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let lxterm = temp.path().join("lxterm");
        fs::write(&lxterm, "#!/bin/sh\n").unwrap();
        let alias = temp.path().join("x-terminal-emulator");
        symlink(&lxterm, &alias).unwrap();

        assert_eq!(
            terminal_launch_arguments(&alias, "-e", &["/state/login.sh".into()]),
            vec!["-fa", "Monospace", "-e", "/state/login.sh"]
        );
    }

    #[test]
    fn non_xterm_launch_keeps_the_terminal_specific_separator() {
        assert_eq!(
            terminal_launch_arguments(
                Path::new("/usr/bin/gnome-terminal"),
                "--",
                &["/state/login.sh".into()],
            ),
            vec!["--", "/state/login.sh"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn login_script_quotes_arguments_and_keeps_failed_authentication_visible() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("login.sh");
        write_login_script(
            &script,
            &[
                "/usr/bin/ssh".into(),
                "host with space".into(),
                "value'with-quote".into(),
            ],
        )
        .unwrap();
        let content = fs::read_to_string(&script).unwrap();

        assert!(content.contains("'/usr/bin/ssh' 'host with space'"));
        assert!(content.contains("'value'\\''with-quote'"));
        assert!(content.contains("Interactive SSH login failed"));
        assert!(content.contains("read -r _"));
        assert_eq!(
            fs::metadata(script).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
