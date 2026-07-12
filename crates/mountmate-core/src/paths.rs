use std::env;
use std::path::{Path, PathBuf};

use crate::LEGACY_APP_ID;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Self {
        let home = home_dir();
        #[cfg(target_os = "windows")]
        {
            let roaming = env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData/Roaming"));
            let local = env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData/Local"));
            return Self {
                config_dir: roaming.join(LEGACY_APP_ID),
                cache_dir: local.join(LEGACY_APP_ID).join("Cache"),
                state_dir: local.join(LEGACY_APP_ID).join("State"),
                data_dir: local.join("ssh-mountmate"),
            };
        }
        #[cfg(target_os = "macos")]
        {
            Self {
                config_dir: env_path("XDG_CONFIG_HOME", home.join(".config")).join(LEGACY_APP_ID),
                cache_dir: env_path("XDG_CACHE_HOME", home.join(".cache")).join(LEGACY_APP_ID),
                state_dir: env_path("XDG_STATE_HOME", home.join(".local/state"))
                    .join(LEGACY_APP_ID),
                data_dir: home.join("Library/Application Support/ssh-mountmate"),
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            Self {
                config_dir: env_path("XDG_CONFIG_HOME", home.join(".config")).join(LEGACY_APP_ID),
                cache_dir: env_path("XDG_CACHE_HOME", home.join(".cache")).join(LEGACY_APP_ID),
                state_dir: env_path("XDG_STATE_HOME", home.join(".local/state"))
                    .join(LEGACY_APP_ID),
                data_dir: env_path("XDG_DATA_HOME", home.join(".local/share"))
                    .join("ssh-mountmate"),
            }
        }
    }

    pub fn servers_file(&self) -> PathBuf {
        self.config_dir.join("servers.json")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn rclone_config(&self) -> PathBuf {
        self.config_dir.join("rclone.conf")
    }

    pub fn rclone_config_lock(&self) -> PathBuf {
        self.config_dir.join("rclone.conf.lock")
    }

    pub fn known_hosts(&self) -> PathBuf {
        self.config_dir.join("known_hosts")
    }

    pub fn known_hosts_lock(&self) -> PathBuf {
        self.config_dir.join("known_hosts.lock")
    }

    pub fn state_file(&self, server_id: &str) -> PathBuf {
        self.state_dir
            .join(format!("{}.json", path_component(server_id)))
    }

    pub fn mount_lock(&self, server_id: &str) -> PathBuf {
        self.state_dir
            .join(format!("{}.mount.lock", path_component(server_id)))
    }

    pub fn mount_log(&self, remote_name: &str) -> PathBuf {
        self.state_dir
            .join(format!("{}.log", path_component(remote_name)))
    }
}

#[cfg(not(target_os = "windows"))]
fn env_path(name: &str, fallback: PathBuf) -> PathBuf {
    env::var_os(name).map(PathBuf::from).unwrap_or(fallback)
}

fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_owned())
        .unwrap_or_else(|| Path::new(".").to_owned())
}

fn path_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let component = component.trim_matches(['.', '_', '-']);
    if component.is_empty() {
        "invalid".into()
    } else {
        component.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_paths_cannot_escape_the_state_directory() {
        let paths = AppPaths {
            config_dir: PathBuf::from("config"),
            cache_dir: PathBuf::from("cache"),
            state_dir: PathBuf::from("state"),
            data_dir: PathBuf::from("data"),
        };
        assert_eq!(
            paths.state_file("../../outside"),
            PathBuf::from("state/outside.json")
        );
        assert_eq!(
            paths.mount_log("host:22"),
            PathBuf::from("state/host_22.log")
        );
    }
}
