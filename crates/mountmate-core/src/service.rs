use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use thiserror::Error;

use crate::connection::{SshImportPlan, plan_ssh_imports};
use crate::mountpoint::{HOME_MOUNTPOINT_VALUE, MountpointAllocator, SystemMountpointProbe};
use crate::paths::AppPaths;
use crate::process::MountStatus;
use crate::rc::{HttpRcClient, RcError};
use crate::rclone::{RcloneConfigError, RcloneRemote, write_rclone_remote};
use crate::rclone_binary::{RcloneBinaryError, resolve_rclone};
use crate::runtime::{
    HttpRcControl, MountRequest, MountRuntime, RuntimeError, SystemMountpointControl,
    SystemProcessControl,
};
use crate::ssh::{
    KnownHostsManager, RequestedTransport, ResolvedSshConfig, SshError, SshTransport,
    choose_transport, list_ssh_config_hosts, readable_file, resolve_ssh_config,
    select_readable_known_hosts,
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
    #[error("rclone obscure failed: {0}")]
    Obscure(String),
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
            .and_then(|path| path.parent().map(Path::to_owned))
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
        self.ensure_remote(server)?;

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

        self.with_runtime(|runtime| {
            loop {
                let mountpoint =
                    allocator
                        .resolve(server)
                        .map_err(|error| RuntimeError::InvalidMountpoint {
                            path: PathBuf::from(&server.mountpoint),
                            message: error.to_string(),
                        })?;
                match runtime.mount(MountRequest {
                    server,
                    settings,
                    rclone: &rclone.path,
                    mountpoint: &mountpoint,
                    cache_dir: &cache_dir,
                }) {
                    Err(RuntimeError::MountpointReserved(_)) if retry_automatic => continue,
                    result => return result.map_err(ServiceError::from),
                }
            }
        })
    }

    pub fn unmount(&self, server_id: &str) -> Result<(), ServiceError> {
        self.with_runtime(|runtime| runtime.unmount(server_id).map_err(ServiceError::from))
    }

    pub fn status(&self, server_id: &str) -> Result<MountStatus, ServiceError> {
        self.with_runtime(|runtime| runtime.status(server_id).map_err(ServiceError::from))
    }

    pub fn transfer_snapshot(&self, server_id: &str) -> Result<TransferSnapshot, ServiceError> {
        let state: MountState = read_json(&self.paths.state_file(server_id))?;
        Ok(HttpRcClient::new(&state.rc_addr, Duration::from_millis(750))?.transfer_snapshot()?)
    }

    pub fn obscure_secret(&self, secret: &str) -> Result<String, ServiceError> {
        if secret.is_empty() {
            return Err(ServiceError::Obscure("secret is empty".into()));
        }
        let rclone = resolve_rclone(&self.paths, &self.app_root, None)?
            .ok_or(ServiceError::RcloneMissing)?;
        let mut command = Command::new(&rclone.path);
        command.args(["obscure", secret]);
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

    fn ensure_remote(&self, server: &ServerConfig) -> Result<(), ServiceError> {
        let resolved = if server.mode == "ssh_config"
            && server.connection_method != ConnectionMethod::Openssh
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
        let known_hosts = if server.connection_method == ConnectionMethod::Openssh {
            None
        } else {
            self.known_hosts_for(server, resolved.as_ref())?
        };
        let remote = RcloneRemote::for_server(
            server,
            resolved.as_ref(),
            known_hosts.as_deref(),
            cfg!(windows),
        )?;
        write_rclone_remote(&self.paths, &remote)?;
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
            Ok(None) => Ok(fallback_known_hosts(&self.paths, resolved, &default)),
            Err(error @ (SshError::InvalidHost(_) | SshError::InvalidPort(_))) => {
                Err(ServiceError::Ssh(error))
            }
            Err(error) => fallback_known_hosts(&self.paths, resolved, &default)
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
) -> Option<PathBuf> {
    resolved.map_or_else(
        || readable_file(&paths.known_hosts()).or_else(|| readable_file(default)),
        |config| select_readable_known_hosts(Some(&paths.known_hosts()), config, default),
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
    use super::*;

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
}
