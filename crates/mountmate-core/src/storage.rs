use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use fs2::FileExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::paths::AppPaths;
use crate::{ServerConfig, Settings};

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
    if let Some(existing) = servers.iter_mut().find(|existing| existing.id == server.id) {
        *existing = server;
    } else {
        servers.push(server);
    }
    write_private_json(&paths.servers_file(), &servers)?;
    Ok(servers)
}

pub fn remove_server(paths: &AppPaths, server_id: &str) -> Result<Vec<ServerConfig>, StorageError> {
    let _lock = FileLock::acquire(&paths.servers_lock(), Duration::from_secs(10))?;
    let mut servers = load_servers(paths)?;
    servers.retain(|server| server.id != server_id);
    write_private_json(&paths.servers_file(), &servers)?;
    Ok(servers)
}

pub fn save_settings(paths: &AppPaths, settings: &Settings) -> Result<(), StorageError> {
    let _lock = FileLock::acquire(&paths.settings_lock(), Duration::from_secs(10))?;
    write_private_json(&paths.settings_file(), settings)
}

pub fn load_settings(paths: &AppPaths) -> Result<Settings, StorageError> {
    if !paths.settings_file().exists() {
        return Ok(Settings::default());
    }
    let mut settings: Settings = read_json(&paths.settings_file())?;
    if settings.cache_root.as_os_str().is_empty() {
        settings.cache_root = paths.cache_dir.clone();
    }
    Ok(settings.migrate())
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
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
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
                Err(_) if started.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(_) => return Err(StorageError::LockTimeout(path.to_owned())),
            }
        }
    }
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
                ..ServerConfig::default()
            }],
        )
        .unwrap();
        let servers = upsert_server(
            &paths,
            ServerConfig {
                id: "beta".into(),
                name: "Beta".into(),
                ..ServerConfig::default()
            },
        )
        .unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(load_servers(&paths).unwrap(), servers);
        assert_eq!(remove_server(&paths, "alpha").unwrap().len(), 1);
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
}
