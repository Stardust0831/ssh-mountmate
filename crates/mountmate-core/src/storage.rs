use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use fs2::FileExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::paths::AppPaths;
use crate::{ServerConfig, Settings};

static ATOMIC_WRITE_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("timed out waiting for lock: {0}")]
    LockTimeout(PathBuf),
    #[error("a connection for the same target already exists: {0}")]
    DuplicateConnection(String),
    #[error("connection does not exist: {0}")]
    MissingConnection(String),
}

pub fn load_servers(paths: &AppPaths) -> Result<Vec<ServerConfig>, StorageError> {
    if !paths.servers_file().exists() {
        return Ok(Vec::new());
    }
    let mut servers: Vec<ServerConfig> = read_json(&paths.servers_file())?;
    for server in &mut servers {
        server.normalize();
    }
    Ok(servers)
}

pub fn save_servers(paths: &AppPaths, servers: &[ServerConfig]) -> Result<(), StorageError> {
    let _lock = FileLock::acquire(&paths.servers_lock(), Duration::from_secs(10))?;
    write_private_json(&paths.servers_file(), servers)
}

pub fn upsert_server(
    paths: &AppPaths,
    server: ServerConfig,
) -> Result<Vec<ServerConfig>, StorageError> {
    let _lock = FileLock::acquire(&paths.servers_lock(), Duration::from_secs(10))?;
    let mut servers = load_servers(paths)?;
    reject_duplicate_target(&servers, &server)?;
    if let Some(existing) = servers.iter_mut().find(|existing| existing.id == server.id) {
        *existing = server;
    } else {
        servers.push(server);
    }
    write_private_json(&paths.servers_file(), &servers)?;
    Ok(servers)
}

pub fn upsert_servers(
    paths: &AppPaths,
    updates: Vec<ServerConfig>,
) -> Result<Vec<ServerConfig>, StorageError> {
    let _lock = FileLock::acquire(&paths.servers_lock(), Duration::from_secs(10))?;
    let mut servers = load_servers(paths)?;
    for server in updates {
        reject_duplicate_target(&servers, &server)?;
        if let Some(existing) = servers.iter_mut().find(|existing| existing.id == server.id) {
            *existing = server;
        } else {
            servers.push(server);
        }
    }
    write_private_json(&paths.servers_file(), &servers)?;
    Ok(servers)
}

fn reject_duplicate_target(
    servers: &[ServerConfig],
    candidate: &ServerConfig,
) -> Result<(), StorageError> {
    if let Some(duplicate) = servers.iter().find(|server| {
        server.id != candidate.id && crate::connection::same_connection_target(server, candidate)
    }) {
        Err(StorageError::DuplicateConnection(
            duplicate.display_name().into(),
        ))
    } else {
        Ok(())
    }
}

pub fn remove_server(paths: &AppPaths, server_id: &str) -> Result<Vec<ServerConfig>, StorageError> {
    let _lock = FileLock::acquire(&paths.servers_lock(), Duration::from_secs(10))?;
    let mut servers = load_servers(paths)?;
    servers.retain(|server| server.id != server_id);
    write_private_json(&paths.servers_file(), &servers)?;
    Ok(servers)
}

pub fn update_server_preferences(
    paths: &AppPaths,
    updates: &[(String, String, bool)],
) -> Result<Vec<ServerConfig>, StorageError> {
    let _lock = FileLock::acquire(&paths.servers_lock(), Duration::from_secs(10))?;
    let mut servers = load_servers(paths)?;
    for (id, folder, auto_mount_at_login) in updates {
        let server = servers
            .iter_mut()
            .find(|server| server.id == *id)
            .ok_or_else(|| StorageError::MissingConnection(id.clone()))?;
        server.folder.clone_from(folder);
        server.auto_mount_at_login = *auto_mount_at_login;
    }
    write_private_json(&paths.servers_file(), &servers)?;
    Ok(servers)
}

pub fn save_settings(paths: &AppPaths, settings: &Settings) -> Result<(), StorageError> {
    let _lock = FileLock::acquire(&paths.settings_lock(), Duration::from_secs(10))?;
    write_private_json(&paths.settings_file(), settings)
}

pub fn load_settings(paths: &AppPaths) -> Result<Settings, StorageError> {
    let mut settings: Settings = match read_json(&paths.settings_file()) {
        Ok(settings) => settings,
        Err(StorageError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound
                && settings_path_is_genuinely_absent(&paths.settings_file()) =>
        {
            Settings::default()
        }
        Err(error) => return Err(error),
    };
    if settings.cache_root.as_os_str().is_empty() {
        settings.cache_root = paths.cache_dir.clone();
    }
    Ok(settings.migrate())
}

fn settings_path_is_genuinely_absent(path: &Path) -> bool {
    if !fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return false;
    }
    let Some(parent) = path.parent() else {
        return true;
    };
    match fs::symlink_metadata(parent) {
        Ok(metadata) => metadata.is_dir(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

#[derive(Debug)]
pub struct RecoveredSettings {
    pub settings: Settings,
    pub load_error: Option<String>,
    pub backup_path: Option<PathBuf>,
    pub backup_error: Option<String>,
    pub persistence_error: Option<String>,
}

pub fn load_settings_recovering(paths: &AppPaths) -> RecoveredSettings {
    match load_settings(paths) {
        Ok(settings) => RecoveredSettings {
            settings,
            load_error: None,
            backup_path: None,
            backup_error: None,
            persistence_error: None,
        },
        Err(error) => {
            let mut settings = Settings::default();
            settings.cache_root = paths.cache_dir.clone();
            let settings = settings.migrate();
            let recovery = recover_settings_file(paths, &settings);
            if let Some(settings) = recovery.reloaded_settings {
                return RecoveredSettings {
                    settings,
                    load_error: None,
                    backup_path: None,
                    backup_error: None,
                    persistence_error: None,
                };
            }
            RecoveredSettings {
                settings,
                load_error: Some(error.to_string()),
                backup_path: recovery.backup_path,
                backup_error: recovery.backup_error,
                persistence_error: recovery.persistence_error,
            }
        }
    }
}

struct SettingsFileRecovery {
    reloaded_settings: Option<Settings>,
    backup_path: Option<PathBuf>,
    backup_error: Option<String>,
    persistence_error: Option<String>,
}

fn recover_settings_file(paths: &AppPaths, settings: &Settings) -> SettingsFileRecovery {
    let _lock = match FileLock::acquire(&paths.settings_lock(), Duration::from_secs(10)) {
        Ok(lock) => lock,
        Err(error) => {
            return SettingsFileRecovery {
                reloaded_settings: None,
                backup_path: None,
                backup_error: None,
                persistence_error: Some(error.to_string()),
            };
        }
    };
    if let Ok(settings) = load_settings(paths) {
        return SettingsFileRecovery {
            reloaded_settings: Some(settings),
            backup_path: None,
            backup_error: None,
            persistence_error: None,
        };
    }

    let settings_file = paths.settings_file();
    let backup = settings_recovery_backup_path(&settings_file);
    let moved = match fs::symlink_metadata(&settings_file) {
        Ok(_) => match fs::rename(&settings_file, &backup) {
            Ok(()) => true,
            Err(error) => {
                return SettingsFileRecovery {
                    reloaded_settings: None,
                    backup_path: None,
                    backup_error: Some(error.to_string()),
                    persistence_error: None,
                };
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return SettingsFileRecovery {
                reloaded_settings: None,
                backup_path: None,
                backup_error: Some(error.to_string()),
                persistence_error: None,
            };
        }
    };

    if let Err(error) = write_private_json(&settings_file, settings) {
        let mut backup_error = None;
        let mut backup_path = None;
        if moved {
            let _ = fs::remove_file(&settings_file);
            if let Err(restore_error) = fs::rename(&backup, &settings_file) {
                backup_error = Some(format!(
                    "could not restore original settings: {restore_error}"
                ));
                backup_path = Some(backup);
            }
        }
        return SettingsFileRecovery {
            reloaded_settings: None,
            backup_path,
            backup_error,
            persistence_error: Some(error.to_string()),
        };
    }

    SettingsFileRecovery {
        reloaded_settings: None,
        backup_path: moved.then_some(backup),
        backup_error: None,
        persistence_error: None,
    }
}

fn settings_recovery_backup_path(path: &Path) -> PathBuf {
    let nonce = ATOMIC_WRITE_NONCE.fetch_add(1, Ordering::Relaxed);
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(
        "{file_name}.invalid.{}.{}.{}.bak",
        std::process::id(),
        nonce,
        uuid::Uuid::new_v4().simple()
    ))
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StorageError> {
    let mut file = File::open(path).map_err(|source| StorageError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|source| StorageError::Io {
            path: path.to_owned(),
            source,
        })?;
    serde_json::from_str(&content).map_err(|source| StorageError::Json {
        path: path.to_owned(),
        source,
    })
}

pub fn write_private_json<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
) -> Result<(), StorageError> {
    let content = serde_json::to_vec_pretty(value).map_err(|source| StorageError::Json {
        path: path.to_owned(),
        source,
    })?;
    atomic_write(path, &content)
}

pub fn atomic_write(path: &Path, content: &[u8]) -> Result<(), StorageError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| StorageError::Io {
        path: parent.to_owned(),
        source,
    })?;
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        ATOMIC_WRITE_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        #[cfg(unix)]
        {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok::<_, std::io::Error>(())
    })();
    if let Err(source) = result {
        let _ = fs::remove_file(&temporary);
        return Err(StorageError::Io {
            path: path.to_owned(),
            source,
        });
    }
    Ok(())
}

pub struct FileLock {
    file: File,
}

impl FileLock {
    pub fn acquire(path: &Path, timeout: Duration) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| StorageError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|source| StorageError::Io {
                path: path.to_owned(),
                source,
            })?;
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if lock_is_contended(&error) && started.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(error) if lock_is_contended(&error) => {
                    return Err(StorageError::LockTimeout(path.to_owned()));
                }
                Err(source) => {
                    return Err(StorageError::Io {
                        path: path.to_owned(),
                        source,
                    });
                }
            }
        }
    }
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    let expected = fs2::lock_contended_error();
    error.kind() == expected.kind()
        && expected
            .raw_os_error()
            .is_none_or(|code| error.raw_os_error() == Some(code))
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn atomic_json_round_trip_preserves_existing_server_fields() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("servers.json");
        let servers = vec![ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            password_obscured: "obscured-secret".into(),
            ..ServerConfig::default()
        }];
        write_private_json(&path, &servers).unwrap();
        let loaded: Vec<ServerConfig> = read_json(&path).unwrap();
        assert_eq!(loaded, servers);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn server_restart_round_trip_preserves_dual_credential_representations() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let server = ServerConfig {
            id: "alpha".into(),
            name: "alpha".into(),
            key_pass_obscured: "obscured-key-passphrase".into(),
            key_pass_credential: "ssh-mountmate:alpha:key-passphrase".into(),
            ..ServerConfig::default()
        };

        save_servers(&paths, std::slice::from_ref(&server)).unwrap();

        assert_eq!(load_servers(&paths).unwrap(), vec![server]);
    }

    #[test]
    fn transactional_upsert_preserves_other_connections() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        save_servers(
            &paths,
            &[ServerConfig {
                id: "alpha".into(),
                name: "Alpha".into(),
                host: "alpha.example".into(),
                user: "alice".into(),
                ..ServerConfig::default()
            }],
        )
        .unwrap();
        let servers = upsert_server(
            &paths,
            ServerConfig {
                id: "beta".into(),
                name: "Beta".into(),
                host: "beta.example".into(),
                user: "alice".into(),
                ..ServerConfig::default()
            },
        )
        .unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(load_servers(&paths).unwrap(), servers);
        assert_eq!(remove_server(&paths, "alpha").unwrap().len(), 1);
    }

    #[test]
    fn transactional_upsert_rechecks_duplicate_targets_under_lock() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let alpha = ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            host: "host.example".into(),
            user: "alice".into(),
            port: "22".into(),
            ..ServerConfig::default()
        };
        save_servers(&paths, std::slice::from_ref(&alpha)).unwrap();
        let duplicate = ServerConfig {
            id: "beta".into(),
            name: "Beta".into(),
            ..alpha
        };
        assert!(matches!(
            upsert_server(&paths, duplicate),
            Err(StorageError::DuplicateConnection(name)) if name == "Alpha"
        ));
    }

    #[test]
    fn settings_save_uses_private_atomic_storage() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let settings = Settings {
            cache_root: temp.path().join("cache with spaces"),
            ..Settings::default()
        };
        save_settings(&paths, &settings).unwrap();
        assert_eq!(load_settings(&paths).unwrap(), settings);
    }

    #[test]
    fn missing_settings_use_app_cache_directory_and_migrate_defaults() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };

        let settings = load_settings(&paths).unwrap();

        assert_eq!(settings.cache_root, paths.cache_dir);
        assert_eq!(
            settings.settings_schema_version,
            crate::model::SETTINGS_SCHEMA_VERSION
        );
    }

    #[test]
    fn malformed_settings_are_backed_up_reset_and_remain_saveable() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(paths.settings_file(), b"{ truncated").unwrap();

        let recovered = load_settings_recovering(&paths);

        assert!(recovered.load_error.is_some());
        assert!(recovered.backup_error.is_none());
        assert!(recovered.persistence_error.is_none());
        let backup = recovered.backup_path.unwrap();
        assert_eq!(fs::read(backup).unwrap(), b"{ truncated");
        assert_eq!(recovered.settings.cache_root, paths.cache_dir);
        assert_eq!(load_settings(&paths).unwrap(), recovered.settings);

        let mut edited = recovered.settings;
        edited.language = "zh-CN".into();
        save_settings(&paths, &edited).unwrap();
        assert_eq!(load_settings(&paths).unwrap(), edited);
    }

    #[test]
    fn unrecoverable_settings_path_reports_in_memory_fallback() {
        let temp = tempdir().unwrap();
        let config_path = temp.path().join("config-is-a-file");
        fs::write(&config_path, b"not a directory").unwrap();
        let paths = AppPaths {
            config_dir: config_path,
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };

        let recovered = load_settings_recovering(&paths);

        assert!(recovered.load_error.is_some());
        assert!(recovered.backup_path.is_none());
        assert!(recovered.persistence_error.is_some());
        assert_eq!(recovered.settings.cache_root, paths.cache_dir);
    }

    #[cfg(unix)]
    #[test]
    fn broken_settings_symlink_is_not_treated_as_a_fresh_profile() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        fs::create_dir_all(&paths.config_dir).unwrap();
        symlink("missing-settings.json", paths.settings_file()).unwrap();

        assert!(matches!(
            load_settings(&paths),
            Err(StorageError::Io { .. })
        ));
    }

    #[test]
    fn preference_patch_preserves_connection_fields_order_and_unrelated_entries() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let alpha = ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            host: "alpha.example".into(),
            ..ServerConfig::default()
        };
        let beta = ServerConfig {
            id: "beta".into(),
            name: "Beta".into(),
            host: "beta.example".into(),
            ..ServerConfig::default()
        };
        save_servers(&paths, &[alpha.clone(), beta.clone()]).unwrap();

        let updated =
            update_server_preferences(&paths, &[("alpha".into(), "Work".into(), true)]).unwrap();

        assert_eq!(updated[0].id, "alpha");
        assert_eq!(updated[0].host, alpha.host);
        assert_eq!(updated[0].folder, "Work");
        assert!(updated[0].auto_mount_at_login);
        assert_eq!(updated[1], beta);
    }
}
