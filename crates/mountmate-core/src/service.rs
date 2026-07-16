use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use thiserror::Error;

use crate::capacity::{CapacityError, CapacityInfo, mounted_capacity};
use crate::connection::{SshImportPlan, plan_ssh_imports};
use crate::credential::{CredentialError, SystemCredentialStore, hydrate_server_from_system};
use crate::interactive_ssh::{InteractiveSshError, InteractiveSshSession};
use crate::mountpoint::{HOME_MOUNTPOINT_VALUE, MountpointAllocator, SystemMountpointProbe};
use crate::paths::AppPaths;
use crate::process::MountStatus;
use crate::rc::{HttpRcClient, RcError, RefreshResult};
use crate::rclone::{
    RcloneConfigError, RcloneRemote, clear_rclone_remote_secrets, normalize_explorer_refresh_path,
    normalize_refresh_relative_path, write_rclone_remote,
};
use crate::rclone_binary::{RcloneBinaryError, resolve_rclone};
use crate::runtime::{
    HttpRcControl, MountRequest, MountRuntime, RuntimeError, SystemMountpointControl,
    SystemProcessControl,
};
use crate::ssh::{
    KnownHostsManager, RequestedTransport, ResolvedSshConfig, SshError, SshTransport,
    choose_transport, known_hosts_marker, list_ssh_config_hosts, resolve_ssh_config,
    select_known_hosts_for_marker,
};
use crate::storage::{StorageError, read_json};
use crate::transfer::TransferSnapshot;
use crate::{ConnectionMethod, MountState, ServerConfig, Settings};

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("rclone was not found in the application bundle, managed directory, or PATH")]
    RcloneMissing,
    #[error(transparent)]
    RcloneBinary(#[from] RcloneBinaryError),
    #[error(transparent)]
    RcloneConfig(#[from] RcloneConfigError),
    #[error(transparent)]
    Rc(#[from] RcError),
    #[error(transparent)]
    Ssh(#[from] SshError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Capacity(#[from] CapacityError),
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error(transparent)]
    InteractiveSsh(#[from] InteractiveSshError),
    #[error("rclone obscure failed: {0}")]
    Obscure(String),
    #[error("the selected path is not inside an active SSH MountMate mount: {0}")]
    PathOutsideMount(String),
}

#[derive(Debug, Clone)]
pub struct MountService {
    paths: AppPaths,
    app_root: PathBuf,
}

impl MountService {
    pub fn new(paths: AppPaths, app_root: PathBuf) -> Self {
        Self { paths, app_root }
    }

    pub fn discover() -> Self {
        let app_root = std::env::current_exe()
            .ok()
            .map(|path| crate::rclone_binary::application_root(&path))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(AppPaths::discover(), app_root)
    }

    pub fn mount(
        &self,
        server: &ServerConfig,
        settings: &Settings,
    ) -> Result<MountState, ServiceError> {
        let rclone = resolve_rclone(&self.paths, &self.app_root, None)?
            .ok_or(ServiceError::RcloneMissing)?;
        let external_ssh = self.interactive_ssh_arguments(server)?;
        let prepared_server = self.prepare_server_credentials(server)?;
        self.ensure_remote(&prepared_server, external_ssh.as_deref())?;

        let home = directories::BaseDirs::new()
            .map(|directories| directories.home_dir().to_owned())
            .unwrap_or_else(|| PathBuf::from("."));
        let probe = SystemMountpointProbe;
        let mut allocator = MountpointAllocator::new(home, cfg!(windows), &probe);
        self.reserve_recorded_mountpoints(&mut allocator, &server.id);
        let retry_automatic = matches!(server.mountpoint.trim(), "" | HOME_MOUNTPOINT_VALUE)
            || server.mountpoint.eq_ignore_ascii_case("auto");
        let cache_dir = if settings.cache_root.as_os_str().is_empty() {
            self.paths.mount_cache_dir(server.remote_name())
        } else {
            expand_home_path(&settings.cache_root).join(server.remote_name())
        };

        let result = self.with_runtime(|runtime| {
            loop {
                let mountpoint = allocator.resolve(&prepared_server).map_err(|error| {
                    RuntimeError::InvalidMountpoint {
                        path: PathBuf::from(&server.mountpoint),
                        message: error.to_string(),
                    }
                })?;
                match runtime.mount(MountRequest {
                    server: &prepared_server,
                    settings,
                    rclone: &rclone.path,
                    mountpoint: &mountpoint,
                    cache_dir: &cache_dir,
                }) {
                    Err(RuntimeError::MountpointReserved(_)) if retry_automatic => continue,
                    result => return result.map_err(ServiceError::from),
                }
            }
        });
        self.finish_secret_use(server, &result)?;
        result
    }

    pub fn unmount(&self, server_id: &str) -> Result<(), ServiceError> {
        self.with_runtime(|runtime| runtime.unmount(server_id).map_err(ServiceError::from))
    }

    pub fn status(&self, server_id: &str) -> Result<MountStatus, ServiceError> {
        self.with_runtime(|runtime| runtime.status(server_id).map_err(ServiceError::from))
    }

    pub fn transfer_snapshot(&self, server_id: &str) -> Result<TransferSnapshot, ServiceError> {
        let state: MountState = read_json(&self.paths.state_file(server_id))?;
        Ok(HttpRcClient::with_credentials(
            &state.rc_addr,
            &state.rc_user,
            &state.rc_pass,
            Duration::from_millis(750),
        )?
        .transfer_snapshot()?)
    }

    pub fn capacity(&self, server: &ServerConfig) -> Result<Option<CapacityInfo>, ServiceError> {
        if self.status(&server.id)? != MountStatus::Mounted {
            return Ok(None);
        }
        let state: MountState = read_json(&self.paths.state_file(&server.id))?;
        let external_ssh = self.interactive_ssh_arguments(server)?;
        let prepared_server = self.prepare_server_credentials(server)?;
        self.ensure_remote(&prepared_server, external_ssh.as_deref())?;
        let result = mounted_capacity(&prepared_server, &state, &self.paths.rclone_config())
            .map_err(ServiceError::from);
        self.finish_secret_use(server, &result)?;
        result
    }

    pub fn refresh(
        &self,
        server_id: &str,
        relative_dir: &str,
        recursive: bool,
    ) -> Result<RefreshResult, ServiceError> {
        let state: MountState = read_json(&self.paths.state_file(server_id))?;
        Ok(HttpRcClient::with_credentials(
            &state.rc_addr,
            &state.rc_user,
            &state.rc_pass,
            Duration::from_secs(3),
        )?
        .refresh_remote(
            &state.remote,
            &normalize_refresh_relative_path(relative_dir),
            recursive,
        )?)
    }

    pub fn refresh_path(
        &self,
        servers: &[ServerConfig],
        local_path: &str,
    ) -> Result<RefreshResult, ServiceError> {
        for server in servers {
            let state_file = self.paths.state_file(&server.id);
            if !state_file.exists() {
                continue;
            }
            let state: MountState = match read_json(&state_file) {
                Ok(state) => state,
                Err(_) => continue,
            };
            let Some(relative_dir) = relative_refresh_dir(
                local_path,
                &state.mountpoint.to_string_lossy(),
                cfg!(windows),
            ) else {
                continue;
            };
            return Ok(HttpRcClient::with_credentials(
                &state.rc_addr,
                &state.rc_user,
                &state.rc_pass,
                Duration::from_secs(3),
            )?
            .refresh_remote(&state.remote, &relative_dir, false)?);
        }
        Err(ServiceError::PathOutsideMount(local_path.into()))
    }

    pub fn obscure_secret(&self, secret: &str) -> Result<String, ServiceError> {
        if secret.is_empty() {
            return Err(ServiceError::Obscure("secret is empty".into()));
        }
        let rclone = resolve_rclone(&self.paths, &self.app_root, None)?
            .ok_or(ServiceError::RcloneMissing)?;
        let mut command = Command::new(&rclone.path);
        command
            .args(["obscure", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command
            .spawn()
            .map_err(|error| ServiceError::Obscure(error.to_string()))?;
        child
            .stdin
            .take()
            .ok_or_else(|| ServiceError::Obscure("rclone stdin was unavailable".into()))?
            .write_all(secret.as_bytes())
            .map_err(|error| ServiceError::Obscure(error.to_string()))?;
        let output = child
            .wait_with_output()
            .map_err(|error| ServiceError::Obscure(error.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(ServiceError::Obscure(if stderr.is_empty() {
                format!("process exited with {}", output.status)
            } else {
                stderr
            }));
        }
        let obscured = String::from_utf8(output.stdout)
            .map_err(|error| ServiceError::Obscure(error.to_string()))?
            .trim()
            .to_owned();
        if obscured.is_empty() {
            Err(ServiceError::Obscure(
                "rclone returned an empty value".into(),
            ))
        } else {
            Ok(obscured)
        }
    }

    pub fn reveal_secret(&self, obscured: &str) -> Result<String, ServiceError> {
        if obscured.is_empty() {
            return Err(ServiceError::Obscure("obscured secret is empty".into()));
        }
        let rclone = resolve_rclone(&self.paths, &self.app_root, None)?
            .ok_or(ServiceError::RcloneMissing)?;
        let mut command = Command::new(&rclone.path);
        command
            .args(["reveal", "--"])
            .arg(obscured)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let output = command
            .output()
            .map_err(|error| ServiceError::Obscure(error.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(ServiceError::Obscure(if stderr.is_empty() {
                format!("process exited with {}", output.status)
            } else {
                stderr
            }));
        }
        let plaintext = String::from_utf8(output.stdout)
            .map_err(|error| ServiceError::Obscure(error.to_string()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if plaintext.is_empty() {
            Err(ServiceError::Obscure(
                "rclone returned an empty revealed value".into(),
            ))
        } else {
            Ok(plaintext)
        }
    }

    pub fn ssh_import_plan(
        &self,
        config_path: &Path,
        existing: &[ServerConfig],
        protected_ids: &std::collections::HashSet<String>,
    ) -> Result<SshImportPlan, ServiceError> {
        let entries = list_ssh_config_hosts(config_path)?;
        let mut seen = std::collections::HashSet::new();
        let imports = entries
            .into_iter()
            .filter(|entry| seen.insert(entry.host.to_ascii_lowercase()))
            .map(|entry| {
                let host_alias = entry.host;
                let server = resolve_ssh_config(Path::new("ssh"), &host_alias, Some(config_path))
                    .map_err(|error| error.to_string())
                    .and_then(|resolved| {
                        imported_ssh_server(&host_alias, config_path, &resolved, cfg!(windows))
                    });
                (host_alias, server)
            })
            .collect();
        Ok(plan_ssh_imports(imports, existing, protected_ids))
    }

    fn ensure_remote(
        &self,
        server: &ServerConfig,
        external_ssh_arguments: Option<&[String]>,
    ) -> Result<(), ServiceError> {
        let resolved = if server.mode == "ssh_config"
            && server.connection_method == ConnectionMethod::Native
        {
            let config_value = if !server.ssh_config_path.trim().is_empty() {
                &server.ssh_config_path
            } else {
                &server.managed_ssh_config_path
            };
            let config = (!config_value.trim().is_empty())
                .then(|| expand_home_path(Path::new(config_value)));
            Some(resolve_ssh_config(
                Path::new("ssh"),
                &server.host_alias,
                config.as_deref(),
            )?)
        } else {
            None
        };
        let known_hosts = if server.connection_method != ConnectionMethod::Native {
            None
        } else {
            self.known_hosts_for(server, resolved.as_ref())?
        };
        let remote = RcloneRemote::for_server_with_external_ssh(
            server,
            resolved.as_ref(),
            known_hosts.as_deref(),
            cfg!(windows),
            external_ssh_arguments,
        )?;
        write_rclone_remote(&self.paths, &remote)?;
        Ok(())
    }

    fn interactive_ssh_arguments(
        &self,
        server: &ServerConfig,
    ) -> Result<Option<Vec<String>>, ServiceError> {
        if server.connection_method != ConnectionMethod::Interactive {
            return Ok(None);
        }
        let session = InteractiveSshSession::for_server(&self.paths, &self.app_root, server)?;
        if !session.is_ready() {
            return Err(InteractiveSshError::SessionMissing.into());
        }
        Ok(Some(session.verified_connector_arguments()?.to_vec()))
    }

    fn hydrate_server_credentials(
        &self,
        server: &ServerConfig,
    ) -> Result<ServerConfig, ServiceError> {
        if server.password_credential.is_empty() && server.key_pass_credential.is_empty() {
            return Ok(server.clone());
        }
        hydrate_server_from_system(server, &SystemCredentialStore, |secret| {
            self.obscure_secret(secret)
                .map_err(|error| CredentialError::Obscure(error.to_string()))
        })
        .map_err(ServiceError::from)
    }

    fn prepare_server_credentials(
        &self,
        server: &ServerConfig,
    ) -> Result<ServerConfig, ServiceError> {
        if server.connection_method == ConnectionMethod::Native {
            self.hydrate_server_credentials(server)
        } else {
            Ok(server.clone())
        }
    }

    fn finish_secret_use<T>(
        &self,
        server: &ServerConfig,
        operation: &Result<T, ServiceError>,
    ) -> Result<(), ServiceError> {
        if server.password_credential.is_empty() && server.key_pass_credential.is_empty() {
            return Ok(());
        }
        if let Err(cleanup_error) = clear_rclone_remote_secrets(&self.paths, server.remote_name()) {
            if operation.is_ok() {
                let _ = self.unmount(&server.id);
            }
            return Err(cleanup_error.into());
        }
        Ok(())
    }

    fn known_hosts_for(
        &self,
        server: &ServerConfig,
        resolved: Option<&ResolvedSshConfig>,
    ) -> Result<Option<PathBuf>, ServiceError> {
        let host = resolved
            .map(|config| config.first("hostname", &server.host))
            .unwrap_or(&server.host);
        let port = resolved
            .map(|config| config.first("port", &server.port))
            .unwrap_or(&server.port);
        let default = directories::BaseDirs::new()
            .map(|directories| directories.home_dir().join(".ssh/known_hosts"))
            .unwrap_or_else(|| PathBuf::from(".ssh/known_hosts"));
        let manager = KnownHostsManager::new(&self.paths);
        match manager.pin_first_seen(Path::new("ssh-keyscan"), host, port) {
            Ok(Some(path)) => Ok(Some(path)),
            Ok(None) => Ok(fallback_known_hosts(
                &self.paths,
                resolved,
                &default,
                host,
                port,
            )),
            Err(error @ (SshError::InvalidHost(_) | SshError::InvalidPort(_))) => {
                Err(ServiceError::Ssh(error))
            }
            Err(error) => fallback_known_hosts(&self.paths, resolved, &default, host, port)
                .map(Some)
                .ok_or(ServiceError::Ssh(error)),
        }
    }

    fn reserve_recorded_mountpoints(
        &self,
        allocator: &mut MountpointAllocator<'_>,
        server_id: &str,
    ) {
        let Ok(entries) = fs::read_dir(&self.paths.state_dir) else {
            return;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if let Ok(state) = read_json::<MountState>(&path)
                && state.server_id != server_id
            {
                allocator.reserve(&state.mountpoint);
            }
        }
    }

    fn with_runtime<T>(&self, action: impl FnOnce(MountRuntime<'_>) -> T) -> T {
        let processes = SystemProcessControl;
        let rc = HttpRcControl::new(Duration::from_millis(250));
        let mountpoints = SystemMountpointControl;
        action(MountRuntime::new(
            &self.paths,
            &processes,
            &rc,
            &mountpoints,
        ))
    }
}

pub fn relative_refresh_dir(requested: &str, mountpoint: &str, windows: bool) -> Option<String> {
    let normalize = |value: &str| {
        normalize_explorer_refresh_path(value, windows)
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_owned()
    };
    let requested_normalized = normalize(requested);
    let mountpoint_normalized = normalize(mountpoint);
    if requested_normalized.is_empty() || mountpoint_normalized.is_empty() {
        return None;
    }
    let equal = if windows {
        requested_normalized.eq_ignore_ascii_case(&mountpoint_normalized)
    } else {
        requested_normalized == mountpoint_normalized
    };
    if equal {
        return Some(String::new());
    }
    let prefix = format!("{mountpoint_normalized}/");
    let relative = if windows {
        requested_normalized.get(prefix.len()..).filter(|_| {
            requested_normalized
                .get(..prefix.len())
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&prefix))
        })
    } else {
        requested_normalized.strip_prefix(&prefix)
    }?;
    Some(normalize_refresh_relative_path(relative))
}

fn imported_ssh_server(
    host_alias: &str,
    config_path: &Path,
    resolved: &ResolvedSshConfig,
    windows: bool,
) -> Result<ServerConfig, String> {
    let host = resolved.first("hostname", host_alias).trim();
    let user = resolved.first("user", "").trim();
    let port = crate::model::normalize_port(resolved.first("port", "22"))
        .ok_or_else(|| "invalid SSH port".to_owned())?;
    if host.is_empty() || user.is_empty() {
        return Err("missing HostName or User".into());
    }
    let connection_method = match choose_transport(RequestedTransport::Auto, resolved, windows) {
        SshTransport::Native => ConnectionMethod::Native,
        SshTransport::Openssh => ConnectionMethod::Openssh,
    };
    Ok(ServerConfig {
        name: host_alias.into(),
        mode: "ssh_config".into(),
        source: "ssh_config".into(),
        host_alias: host_alias.into(),
        host: host.into(),
        user: user.into(),
        port,
        auth: crate::AuthMethod::Key,
        key_file: resolved
            .first_existing_path("identityfile")
            .map_or_else(String::new, |path| path.display().to_string()),
        connection_method,
        ssh_config_path: config_path.display().to_string(),
        ..ServerConfig::default()
    })
}

fn fallback_known_hosts(
    paths: &AppPaths,
    resolved: Option<&ResolvedSshConfig>,
    default: &Path,
    host: &str,
    port: &str,
) -> Option<PathBuf> {
    select_known_hosts_for_marker(
        Some(&paths.known_hosts()),
        resolved,
        default,
        &known_hosts_marker(host, port),
    )
}

fn expand_home_path(path: &Path) -> PathBuf {
    let value = path.as_os_str().to_string_lossy();
    if (value == "~" || value.starts_with("~/") || value.starts_with("~\\"))
        && let Some(directories) = directories::BaseDirs::new()
    {
        return if value == "~" {
            directories.home_dir().to_owned()
        } else {
            directories.home_dir().join(&value[2..])
        };
    }
    path.to_owned()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    use tempfile::tempdir;

    use super::*;

    #[cfg(unix)]
    #[test]
    fn obscure_secret_is_delivered_over_stdin_not_argv() {
        let temp = tempdir().unwrap();
        let app_root = temp.path().join("app");
        let binary = app_root.join("bin/rclone");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(
            &binary,
            b"#!/bin/sh\nif [ \"$1\" = obscure ]; then\n  [ \"$2\" = - ]\n  IFS= read -r secret || true\n  [ \"$secret\" = 'top secret' ]\n  printf 'obscured-value\\n'\nelif [ \"$1\" = reveal ]; then\n  [ \"$2\" = -- ]\n  [ \"$3\" = obscured-value ]\n  printf 'top secret\\n'\nelse\n  exit 1\nfi\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let digest = crate::rclone_binary::file_sha256(&binary).unwrap();
        fs::write(binary.with_file_name("rclone.sha256"), digest).unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let service = MountService::new(paths, app_root);

        assert_eq!(
            service.obscure_secret("top secret").unwrap(),
            "obscured-value"
        );
        assert_eq!(
            service.reveal_secret("obscured-value").unwrap(),
            "top secret"
        );
    }

    #[test]
    fn resolved_ssh_host_becomes_a_self_contained_import_profile() {
        let resolved = ResolvedSshConfig::parse(
            "hostname login.example\nuser alice\nport 2202\nidentityfile /missing/key\n",
        );
        let server = imported_ssh_server(
            "cluster",
            Path::new("/tmp/custom ssh config"),
            &resolved,
            true,
        )
        .unwrap();
        assert_eq!(server.mode, "ssh_config");
        assert_eq!(server.host, "login.example");
        assert_eq!(server.user, "alice");
        assert_eq!(server.port, "2202");
        assert_eq!(server.connection_method, ConnectionMethod::Native);
        assert_eq!(server.ssh_config_path, "/tmp/custom ssh config");
        assert!(server.key_file.is_empty());
    }

    #[test]
    fn proxy_configuration_selects_openssh_transport() {
        let resolved = ResolvedSshConfig::parse(
            "hostname login.example\nuser alice\nport 22\nproxyjump gateway\n",
        );
        let server = imported_ssh_server("cluster", Path::new("config"), &resolved, true).unwrap();
        assert_eq!(server.connection_method, ConnectionMethod::Openssh);
    }

    #[test]
    fn refresh_path_resolves_root_and_nested_directories() {
        assert_eq!(
            relative_refresh_dir("/mnt/alpha", "/mnt/alpha", false),
            Some(String::new())
        );
        assert_eq!(
            relative_refresh_dir("/mnt/alpha/folder/child", "/mnt/alpha", false),
            Some("folder/child".into())
        );
        assert_eq!(
            relative_refresh_dir("/mnt/alphabet", "/mnt/alpha", false),
            None
        );
    }

    #[test]
    fn windows_refresh_path_repairs_quotes_and_compares_case_insensitively() {
        assert_eq!(
            relative_refresh_dir("Y:\"", "Y:", true),
            Some(String::new())
        );
        assert_eq!(
            relative_refresh_dir("y:\\Folder\\Child\\.", "Y:", true),
            Some("Folder/Child".into())
        );
        assert_eq!(relative_refresh_dir("Z:\\Folder", "Y:", true), None);
    }

    #[cfg(unix)]
    #[test]
    fn missing_interactive_session_does_not_start_a_login_process() {
        if crate::rclone_binary::find_system_executable("ssh").is_none() {
            return;
        }
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let service = MountService::new(paths.clone(), temp.path().join("app"));
        let server = ServerConfig {
            id: "missing-session".into(),
            host: "host.example".into(),
            user: "alice".into(),
            connection_method: ConnectionMethod::Interactive,
            ..ServerConfig::default()
        };

        let error = service.interactive_ssh_arguments(&server).unwrap_err();
        assert!(matches!(
            error,
            ServiceError::InteractiveSsh(InteractiveSshError::SessionMissing)
        ));
        let preferred_control_dir = paths.state_dir.join("ssh-control");
        if preferred_control_dir.is_dir() {
            assert_eq!(fs::read_dir(preferred_control_dir).unwrap().count(), 0);
        }
        assert!(!paths.config_dir.exists());
        assert!(!paths.cache_dir.exists());
        assert!(!paths.data_dir.exists());
    }
}
