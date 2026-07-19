use std::collections::HashSet;
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
    #[error("invalid connection preference update: {0}")]
    InvalidPreferenceUpdate(String),
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

/// Removes an exact set of existing connection IDs under one lock and one
/// atomic write. Invalid selections leave the file untouched.
pub fn remove_servers(
    paths: &AppPaths,
    server_ids: &[String],
) -> Result<Vec<ServerConfig>, StorageError> {
    let _lock = FileLock::acquire(&paths.servers_lock(), Duration::from_secs(10))?;
    let mut servers = load_servers(paths)?;
    let mut selected = HashSet::with_capacity(server_ids.len());
    for id in server_ids {
        if !selected.insert(id.clone()) {
            return Err(StorageError::InvalidPreferenceUpdate(format!(
                "duplicate connection ID: {id}"
            )));
        }
        if !servers.iter().any(|server| server.id == *id) {
            return Err(StorageError::MissingConnection(id.clone()));
        }
    }
    servers.retain(|server| !selected.contains(&server.id));
    write_private_json(&paths.servers_file(), &servers)?;
    Ok(servers)
}

/// Reorders all saved connections according to an exact permutation of their
/// IDs. Unknown, missing, or duplicate IDs are rejected before any write.
pub fn reorder_servers(
    paths: &AppPaths,
    ordered_ids: &[String],
) -> Result<Vec<ServerConfig>, StorageError> {
    let _lock = FileLock::acquire(&paths.servers_lock(), Duration::from_secs(10))?;
    let servers = load_servers(paths)?;
    if ordered_ids.len() != servers.len() {
        return Err(StorageError::InvalidPreferenceUpdate(
            "reorder must include every connection exactly once".into(),
        ));
    }
    let by_id = servers
        .iter()
        .map(|server| (server.id.as_str(), server))
        .collect::<std::collections::HashMap<_, _>>();
    let mut seen = HashSet::with_capacity(ordered_ids.len());
    let mut reordered = Vec::with_capacity(servers.len());
    for id in ordered_ids {
        if !seen.insert(id) {
            return Err(StorageError::InvalidPreferenceUpdate(format!(
                "duplicate connection ID: {id}"
            )));
        }
        let Some(server) = by_id.get(id.as_str()) else {
            return Err(StorageError::MissingConnection(id.clone()));
        };
        reordered.push((*server).clone());
    }
    write_private_json(&paths.servers_file(), &reordered)?;
    Ok(reordered)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerPreferenceUpdate {
    pub id: String,
    pub tags: Option<Vec<String>>,
    pub auto_mount_at_login: Option<bool>,
}

/// Applies tag and optional startup preference updates atomically while
/// preserving every other server field.
pub fn update_server_preferences_batch(
    paths: &AppPaths,
    updates: &[ServerPreferenceUpdate],
) -> Result<Vec<ServerConfig>, StorageError> {
    let _lock = FileLock::acquire(&paths.servers_lock(), Duration::from_secs(10))?;
    let mut servers = load_servers(paths)?;
    let mut seen = HashSet::with_capacity(updates.len());
    for update in updates {
        if !seen.insert(update.id.clone()) {
            return Err(StorageError::InvalidPreferenceUpdate(format!(
                "duplicate connection ID: {}",
                update.id
            )));
        }
        if !servers.iter().any(|server| server.id == update.id) {
            return Err(StorageError::MissingConnection(update.id.clone()));
        }
    }
    for update in updates {
        if update
            .tags
            .as_ref()
            .is_some_and(|tags| tags.iter().any(|tag| tag.chars().any(char::is_control)))
        {
            return Err(StorageError::InvalidPreferenceUpdate(format!(
                "tags for connection {} must not contain control characters",
                update.id
            )));
        }
        if let Some(tags) = &update.tags {
            let mut normalized_tags = tags.clone();
            crate::model::normalize_tags(&mut normalized_tags, "");
            if normalized_tags.len() > crate::model::MAX_CONNECTION_TAGS {
                return Err(StorageError::InvalidPreferenceUpdate(format!(
                    "connection {} may have at most {} tags",
                    update.id,
                    crate::model::MAX_CONNECTION_TAGS
                )));
            }
            if normalized_tags
                .iter()
                .any(|tag| tag.chars().count() > crate::model::MAX_TAG_CHARS)
            {
                return Err(StorageError::InvalidPreferenceUpdate(format!(
                    "tags for connection {} must be at most {} Unicode characters each",
                    update.id,
                    crate::model::MAX_TAG_CHARS
                )));
            }
        }
        let server = servers
            .iter_mut()
            .find(|server| server.id == update.id)
            .expect("validated connection ID");
        if let Some(mut tags) = update.tags.clone() {
            crate::model::normalize_tags(&mut tags, "");
            server.tags = tags;
            server.folder = server.tags.first().cloned().unwrap_or_default();
        }
        if let Some(auto_mount_at_login) = update.auto_mount_at_login {
            server.auto_mount_at_login = auto_mount_at_login;
        }
    }
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
        if folder.chars().any(char::is_control) {
            return Err(StorageError::InvalidPreferenceUpdate(format!(
                "folder for connection {id} must not contain control characters"
            )));
        }
        let server = servers
            .iter_mut()
            .find(|server| server.id == *id)
            .ok_or_else(|| StorageError::MissingConnection(id.clone()))?;
        let folder = folder.trim();
        if folder != server.folder {
            server.tags.clear();
            crate::model::normalize_tags(&mut server.tags, folder);
            server.folder = server.tags.first().cloned().unwrap_or_default();
        }
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
    pub attempted_backup_path: Option<PathBuf>,
    pub backup_error: Option<String>,
    pub backup_error_kind: Option<std::io::ErrorKind>,
    pub persistence_error: Option<String>,
    pub persistence_error_kind: Option<std::io::ErrorKind>,
    pub cleanup_error: Option<String>,
    pub failure_stage: Option<SettingsRecoveryStage>,
    pub source_was_present: bool,
    pub original_restored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRecoveryStage {
    AcquireLock,
    Backup,
    PersistDefaults,
    RestoreOriginal,
}

pub fn load_settings_recovering(paths: &AppPaths) -> RecoveredSettings {
    match load_settings(paths) {
        Ok(settings) => RecoveredSettings {
            settings,
            load_error: None,
            backup_path: None,
            attempted_backup_path: None,
            backup_error: None,
            backup_error_kind: None,
            persistence_error: None,
            persistence_error_kind: None,
            cleanup_error: None,
            failure_stage: None,
            source_was_present: false,
            original_restored: false,
        },
        Err(error) => {
            let settings = Settings {
                cache_root: paths.cache_dir.clone(),
                ..Settings::default()
            };
            let settings = settings.migrate();
            let recovery = recover_settings_file(paths, &settings);
            if let Some(settings) = recovery.reloaded_settings {
                return RecoveredSettings {
                    settings,
                    load_error: None,
                    backup_path: None,
                    attempted_backup_path: None,
                    backup_error: None,
                    backup_error_kind: None,
                    persistence_error: None,
                    persistence_error_kind: None,
                    cleanup_error: None,
                    failure_stage: None,
                    source_was_present: false,
                    original_restored: false,
                };
            }
            RecoveredSettings {
                settings,
                load_error: Some(error.to_string()),
                backup_path: recovery.backup_path,
                attempted_backup_path: recovery.attempted_backup_path,
                backup_error: recovery.backup_error,
                backup_error_kind: recovery.backup_error_kind,
                persistence_error: recovery.persistence_error,
                persistence_error_kind: recovery.persistence_error_kind,
                cleanup_error: recovery.cleanup_error,
                failure_stage: recovery.failure_stage,
                source_was_present: recovery.source_was_present,
                original_restored: recovery.original_restored,
            }
        }
    }
}

struct SettingsFileRecovery {
    reloaded_settings: Option<Settings>,
    backup_path: Option<PathBuf>,
    attempted_backup_path: Option<PathBuf>,
    backup_error: Option<String>,
    backup_error_kind: Option<std::io::ErrorKind>,
    persistence_error: Option<String>,
    persistence_error_kind: Option<std::io::ErrorKind>,
    cleanup_error: Option<String>,
    failure_stage: Option<SettingsRecoveryStage>,
    source_was_present: bool,
    original_restored: bool,
}

fn recover_settings_file(paths: &AppPaths, settings: &Settings) -> SettingsFileRecovery {
    let settings_file = paths.settings_file();
    let backup = settings_recovery_backup_path(&settings_file);
    let _lock = match FileLock::acquire(&paths.settings_lock(), Duration::from_secs(10)) {
        Ok(lock) => lock,
        Err(error) => {
            return SettingsFileRecovery {
                reloaded_settings: None,
                backup_path: None,
                attempted_backup_path: Some(backup),
                backup_error: None,
                backup_error_kind: None,
                persistence_error: Some(error.to_string()),
                persistence_error_kind: storage_error_kind(&error),
                cleanup_error: None,
                failure_stage: Some(SettingsRecoveryStage::AcquireLock),
                source_was_present: false,
                original_restored: false,
            };
        }
    };
    if let Ok(settings) = load_settings(paths) {
        return SettingsFileRecovery {
            reloaded_settings: Some(settings),
            backup_path: None,
            attempted_backup_path: None,
            backup_error: None,
            backup_error_kind: None,
            persistence_error: None,
            persistence_error_kind: None,
            cleanup_error: None,
            failure_stage: None,
            source_was_present: false,
            original_restored: false,
        };
    }

    let moved = match fs::symlink_metadata(&settings_file) {
        Ok(_) => match fs::rename(&settings_file, &backup) {
            Ok(()) => true,
            Err(error) => {
                return SettingsFileRecovery {
                    reloaded_settings: None,
                    backup_path: None,
                    attempted_backup_path: Some(backup),
                    backup_error: Some(error.to_string()),
                    backup_error_kind: Some(error.kind()),
                    persistence_error: None,
                    persistence_error_kind: None,
                    cleanup_error: None,
                    failure_stage: Some(SettingsRecoveryStage::Backup),
                    source_was_present: true,
                    original_restored: false,
                };
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return SettingsFileRecovery {
                reloaded_settings: None,
                backup_path: None,
                attempted_backup_path: Some(backup),
                backup_error: Some(error.to_string()),
                backup_error_kind: Some(error.kind()),
                persistence_error: None,
                persistence_error_kind: None,
                cleanup_error: None,
                failure_stage: Some(SettingsRecoveryStage::Backup),
                source_was_present: false,
                original_restored: false,
            };
        }
    };

    if let Err(error) = write_private_json(&settings_file, settings) {
        let mut backup_error = None;
        let mut backup_error_kind = None;
        let mut backup_path = None;
        let mut cleanup_error = None;
        let mut original_restored = false;
        if moved {
            if let Err(error) = fs::remove_file(&settings_file)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                cleanup_error = Some(error.to_string());
            }
            if let Err(restore_error) = fs::rename(&backup, &settings_file) {
                backup_error = Some(format!(
                    "could not restore original settings: {restore_error}"
                ));
                backup_error_kind = Some(restore_error.kind());
                backup_path = Some(backup.clone());
            } else {
                original_restored = true;
            }
        }
        let failure_stage = if backup_error.is_some() {
            SettingsRecoveryStage::RestoreOriginal
        } else {
            SettingsRecoveryStage::PersistDefaults
        };
        return SettingsFileRecovery {
            reloaded_settings: None,
            backup_path,
            attempted_backup_path: Some(backup),
            backup_error,
            backup_error_kind,
            persistence_error: Some(error.to_string()),
            persistence_error_kind: storage_error_kind(&error),
            cleanup_error,
            failure_stage: Some(failure_stage),
            source_was_present: moved,
            original_restored,
        };
    }

    SettingsFileRecovery {
        reloaded_settings: None,
        backup_path: moved.then_some(backup.clone()),
        attempted_backup_path: Some(backup),
        backup_error: None,
        backup_error_kind: None,
        persistence_error: None,
        persistence_error_kind: None,
        cleanup_error: None,
        failure_stage: None,
        source_was_present: moved,
        original_restored: false,
    }
}

fn storage_error_kind(error: &StorageError) -> Option<std::io::ErrorKind> {
    match error {
        StorageError::Io { source, .. } => Some(source.kind()),
        _ => None,
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
        assert!(recovered.backup_error_kind.is_none());
        assert!(recovered.persistence_error.is_none());
        assert!(recovered.persistence_error_kind.is_none());
        let backup = recovered.backup_path.as_ref().unwrap();
        assert_eq!(recovered.attempted_backup_path.as_ref(), Some(backup));
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
        let attempted = recovered.attempted_backup_path.as_ref().unwrap();
        assert!(
            attempted
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("settings.json.invalid.")
        );
        assert!(recovered.persistence_error.is_some());
        assert_eq!(
            recovered.failure_stage,
            Some(SettingsRecoveryStage::AcquireLock)
        );
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

        let mut multi_tag = updated;
        multi_tag[0].tags.push("Shared".into());
        save_servers(&paths, &multi_tag).unwrap();
        let startup_only =
            update_server_preferences(&paths, &[("alpha".into(), "Work".into(), false)]).unwrap();
        assert_eq!(startup_only[0].tags, vec!["Work", "Shared"]);
        assert_eq!(startup_only[0].folder, "Work");
        assert!(!startup_only[0].auto_mount_at_login);
    }

    #[test]
    fn legacy_folder_loads_as_a_tag_and_empty_batch_tags_clear_compatibility_folder() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(
            paths.servers_file(),
            br#"[{"id":"alpha","folder":" Work "}]"#,
        )
        .unwrap();
        let loaded = load_servers(&paths).unwrap();
        assert_eq!(loaded[0].tags, vec!["Work"]);
        let updated = update_server_preferences_batch(
            &paths,
            &[ServerPreferenceUpdate {
                id: "alpha".into(),
                tags: Some(Vec::new()),
                auto_mount_at_login: None,
            }],
        )
        .unwrap();
        assert!(updated[0].tags.is_empty());
        assert!(updated[0].folder.is_empty());
    }

    #[test]
    fn reorder_requires_exact_permutation_and_preserves_saved_order() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let servers = vec![
            ServerConfig {
                id: "alpha".into(),
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "beta".into(),
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "gamma".into(),
                ..ServerConfig::default()
            },
        ];
        save_servers(&paths, &servers).unwrap();
        let reordered =
            reorder_servers(&paths, &["gamma".into(), "alpha".into(), "beta".into()]).unwrap();
        assert_eq!(
            reordered
                .iter()
                .map(|server| server.id.as_str())
                .collect::<Vec<_>>(),
            vec!["gamma", "alpha", "beta"]
        );
        assert_eq!(load_servers(&paths).unwrap(), reordered);
        let before = fs::read(paths.servers_file()).unwrap();
        assert!(reorder_servers(&paths, &["gamma".into(), "gamma".into(), "beta".into()]).is_err());
        assert_eq!(fs::read(paths.servers_file()).unwrap(), before);
        assert!(
            reorder_servers(&paths, &["gamma".into(), "alpha".into(), "unknown".into()]).is_err()
        );
        assert_eq!(fs::read(paths.servers_file()).unwrap(), before);
    }

    #[test]
    fn batch_preferences_only_change_tags_and_requested_startup_flag() {
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
            user: "alice".into(),
            password_credential: "credential-ref".into(),
            auto_mount_at_login: true,
            ..ServerConfig::default()
        };
        save_servers(&paths, std::slice::from_ref(&alpha)).unwrap();
        let updated = update_server_preferences_batch(
            &paths,
            &[ServerPreferenceUpdate {
                id: "alpha".into(),
                tags: Some(vec![" Work ".into(), "研究".into()]),
                auto_mount_at_login: None,
            }],
        )
        .unwrap();
        assert_eq!(updated[0].tags, vec!["Work", "研究"]);
        assert_eq!(updated[0].folder, "Work");
        assert!(updated[0].auto_mount_at_login);
        assert_eq!(updated[0].host, alpha.host);
        assert_eq!(updated[0].password_credential, alpha.password_credential);

        let startup_only = update_server_preferences_batch(
            &paths,
            &[ServerPreferenceUpdate {
                id: "alpha".into(),
                tags: None,
                auto_mount_at_login: Some(false),
            }],
        )
        .unwrap();
        assert_eq!(startup_only[0].tags, vec!["Work", "研究"]);
        assert_eq!(startup_only[0].folder, "Work");
        assert!(!startup_only[0].auto_mount_at_login);
    }

    #[test]
    fn batch_preferences_reject_tag_limits() {
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
                ..ServerConfig::default()
            }],
        )
        .unwrap();
        let too_many = (0..=crate::model::MAX_CONNECTION_TAGS)
            .map(|i| format!("tag-{i}"))
            .collect();
        assert!(
            update_server_preferences_batch(
                &paths,
                &[ServerPreferenceUpdate {
                    id: "alpha".into(),
                    tags: Some(too_many),
                    auto_mount_at_login: None
                }]
            )
            .is_err()
        );
        let too_long = vec!["界".repeat(crate::model::MAX_TAG_CHARS + 1)];
        assert!(
            update_server_preferences_batch(
                &paths,
                &[ServerPreferenceUpdate {
                    id: "alpha".into(),
                    tags: Some(too_long),
                    auto_mount_at_login: None
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn batch_remove_requires_existing_unique_ids_and_preserves_order() {
        let temp = tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        save_servers(
            &paths,
            &[
                ServerConfig {
                    id: "alpha".into(),
                    ..ServerConfig::default()
                },
                ServerConfig {
                    id: "beta".into(),
                    ..ServerConfig::default()
                },
                ServerConfig {
                    id: "gamma".into(),
                    ..ServerConfig::default()
                },
            ],
        )
        .unwrap();
        let before = fs::read(paths.servers_file()).unwrap();
        assert!(remove_servers(&paths, &["beta".into(), "unknown".into()]).is_err());
        assert_eq!(fs::read(paths.servers_file()).unwrap(), before);
        let remaining = remove_servers(&paths, &["beta".into()]).unwrap();
        assert_eq!(
            remaining
                .iter()
                .map(|server| server.id.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "gamma"]
        );
        let before_duplicate = fs::read(paths.servers_file()).unwrap();
        assert!(remove_servers(&paths, &["alpha".into(), "alpha".into()]).is_err());
        assert_eq!(fs::read(paths.servers_file()).unwrap(), before_duplicate);
    }
}
