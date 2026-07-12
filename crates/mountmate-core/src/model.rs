use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SETTINGS_SCHEMA_VERSION: u32 = 8;

fn default_port() -> String {
    "22".into()
}

fn default_auth() -> AuthMethod {
    AuthMethod::Key
}

fn default_connection_method() -> ConnectionMethod {
    ConnectionMethod::Native
}

fn default_cache_mode() -> String {
    "full".into()
}

fn default_cache_age() -> String {
    "30m".into()
}

fn default_write_back() -> String {
    "0s".into()
}

fn default_dir_cache_time() -> String {
    "5m".into()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    #[default]
    Key,
    Password,
}

impl AuthMethod {
    pub const ALL: [Self; 2] = [Self::Key, Self::Password];
}

impl fmt::Display for AuthMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Key => "Private key",
            Self::Password => "Password",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMethod {
    #[default]
    Native,
    Openssh,
}

impl ConnectionMethod {
    pub const ALL: [Self; 2] = [Self::Native, Self::Openssh];
}

impl fmt::Display for ConnectionMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Native => "Native SFTP",
            Self::Openssh => "OpenSSH",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub host_alias: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub user: String,
    #[serde(default = "default_port")]
    pub port: String,
    #[serde(default = "default_auth")]
    pub auth: AuthMethod,
    #[serde(default)]
    pub key_file: String,
    #[serde(default)]
    pub password_obscured: String,
    #[serde(default)]
    pub key_pass_obscured: String,
    #[serde(default = "default_connection_method")]
    pub connection_method: ConnectionMethod,
    #[serde(default)]
    pub remote_path: String,
    #[serde(default)]
    pub mountpoint: String,
    #[serde(default)]
    pub cache_mode: String,
    #[serde(default)]
    pub network_mode: bool,
    #[serde(default)]
    pub ssh_config_managed: bool,
    #[serde(default)]
    pub copy_key_to_ssh_dir: bool,
    #[serde(default)]
    pub managed_ssh_config_path: String,
}

fn default_mode() -> String {
    "manual".into()
}

fn default_source() -> String {
    "manual".into()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            mode: default_mode(),
            source: default_source(),
            host_alias: String::new(),
            host: String::new(),
            user: String::new(),
            port: default_port(),
            auth: default_auth(),
            key_file: String::new(),
            password_obscured: String::new(),
            key_pass_obscured: String::new(),
            connection_method: default_connection_method(),
            remote_path: String::new(),
            mountpoint: String::new(),
            cache_mode: String::new(),
            network_mode: false,
            ssh_config_managed: false,
            copy_key_to_ssh_dir: false,
            managed_ssh_config_path: String::new(),
        }
    }
}

impl ServerConfig {
    pub fn normalize(&mut self) {
        let id_source = if self.id.trim().is_empty() {
            self.display_name()
        } else {
            &self.id
        };
        self.id = sanitize_id(id_source);
        if self.name.trim().is_empty() {
            self.name = self.display_name().to_owned();
        }
        self.port = normalize_port(&self.port).unwrap_or_else(|| "22".into());
    }

    pub fn display_name(&self) -> &str {
        [&self.name, &self.host_alias, &self.host, &self.id]
            .into_iter()
            .find(|value| !value.trim().is_empty())
            .map_or("Server", String::as_str)
    }

    pub fn remote_name(&self) -> &str {
        if self.mode == "ssh_config" && !self.host_alias.is_empty() {
            &self.host_alias
        } else {
            &self.id
        }
    }

    pub fn remote_spec(&self) -> String {
        let path = self.remote_path.trim();
        if path.is_empty() {
            format!("{}:", self.remote_name())
        } else {
            format!("{}:{}", self.remote_name(), path.trim_start_matches('/'))
        }
    }

    pub fn effective_cache_mode<'a>(&'a self, settings: &'a Settings) -> &'a str {
        if self.cache_mode.is_empty() {
            &settings.vfs_cache_mode
        } else {
            &self.cache_mode
        }
    }
}

pub fn normalize_port(value: &str) -> Option<String> {
    let port = value.trim().parse::<u16>().ok()?;
    (port > 0).then(|| port.to_string())
}

pub fn sanitize_id(value: &str) -> String {
    let cleaned: String = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(['.', '_', '-']);
    if cleaned.is_empty() {
        format!("server-{}", &Uuid::new_v4().simple().to_string()[..8])
    } else {
        cleaned.to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "schema_version")]
    pub settings_schema_version: u32,
    #[serde(default)]
    pub cache_root: PathBuf,
    #[serde(default = "default_cache_mode")]
    pub vfs_cache_mode: String,
    #[serde(default)]
    pub vfs_cache_max_size: String,
    #[serde(default = "default_cache_age")]
    pub vfs_cache_max_age: String,
    #[serde(default)]
    pub vfs_cache_min_free_space: String,
    #[serde(default = "default_write_back")]
    pub vfs_write_back: String,
    #[serde(default = "default_dir_cache_time")]
    pub dir_cache_time: String,
    #[serde(default)]
    pub buffer_size: String,
    #[serde(default)]
    pub startup_all: bool,
    #[serde(default = "default_true")]
    pub auto_show_transfers: bool,
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
    #[serde(default = "default_language")]
    pub language: String,
}

fn schema_version() -> u32 {
    SETTINGS_SCHEMA_VERSION
}

fn default_language() -> String {
    "auto".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_schema_version: SETTINGS_SCHEMA_VERSION,
            cache_root: PathBuf::new(),
            vfs_cache_mode: default_cache_mode(),
            vfs_cache_max_size: String::new(),
            vfs_cache_max_age: default_cache_age(),
            vfs_cache_min_free_space: String::new(),
            vfs_write_back: default_write_back(),
            dir_cache_time: default_dir_cache_time(),
            buffer_size: String::new(),
            startup_all: false,
            auto_show_transfers: true,
            auto_check_updates: true,
            language: default_language(),
        }
    }
}

impl Settings {
    pub fn migrate(mut self) -> Self {
        let version = self.settings_schema_version;
        if version < 2
            && self.vfs_cache_mode == "writes"
            && self.vfs_cache_max_size.is_empty()
            && self.vfs_cache_max_age.is_empty()
            && self.vfs_cache_min_free_space.is_empty()
            && self.vfs_write_back.is_empty()
            && self.dir_cache_time.is_empty()
        {
            self.apply_recommended_cache_defaults(true);
        } else if version < 3 && self.vfs_cache_mode == "off"
            || version < 5 && self.vfs_cache_mode.is_empty()
        {
            self.apply_recommended_cache_defaults(false);
        } else if version < 4
            && self.vfs_cache_mode == "minimal"
            && self.vfs_cache_max_size == "10G"
            && self.vfs_cache_min_free_space == "10G"
            && self.vfs_write_back == "0s"
            && self.dir_cache_time == "30s"
        {
            self.apply_recommended_cache_defaults(true);
        } else if version < 6
            && self.vfs_cache_mode == "writes"
            && self.vfs_cache_max_age.is_empty()
            && self.vfs_write_back.is_empty()
            && self.dir_cache_time.is_empty()
        {
            self.apply_recommended_cache_defaults(false);
        }
        self.settings_schema_version = SETTINGS_SCHEMA_VERSION;
        self
    }

    fn apply_recommended_cache_defaults(&mut self, clear_limits: bool) {
        self.vfs_cache_mode = default_cache_mode();
        self.vfs_cache_max_age = default_cache_age();
        self.vfs_write_back = default_write_back();
        self.dir_cache_time = default_dir_cache_time();
        if clear_limits {
            self.vfs_cache_max_size.clear();
            self.vfs_cache_min_free_space.clear();
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MountPhase {
    Starting,
    #[default]
    Mounted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountState {
    pub pid: u32,
    pub server_id: String,
    pub remote: String,
    pub mountpoint: PathBuf,
    pub log: PathBuf,
    pub rc_addr: String,
    #[serde(default)]
    pub phase: MountPhase,
    #[serde(default)]
    pub process_started_at: Option<u64>,
    #[serde(default)]
    pub rclone: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_cache_defaults_migrate_without_overwriting_custom_limits() {
        let settings = Settings {
            settings_schema_version: 5,
            vfs_cache_mode: "writes".into(),
            vfs_cache_max_size: "20G".into(),
            vfs_cache_max_age: String::new(),
            vfs_write_back: String::new(),
            dir_cache_time: String::new(),
            ..Settings::default()
        }
        .migrate();
        assert_eq!(settings.vfs_cache_mode, "full");
        assert_eq!(settings.vfs_cache_max_size, "20G");
        assert_eq!(settings.vfs_cache_max_age, "30m");
        assert_eq!(settings.vfs_write_back, "0s");
        assert_eq!(settings.dir_cache_time, "5m");
    }

    #[test]
    fn server_deserializes_existing_python_shape() {
        let mut server: ServerConfig = serde_json::from_str(
            r#"{"id":"SAI-user","name":"SAI user","host":"example.com","user":"user","port":"12022","auth":"key","connection_method":"native"}"#,
        )
        .unwrap();
        server.normalize();
        assert_eq!(server.remote_spec(), "SAI-user:");
        assert_eq!(server.port, "12022");
    }

    #[test]
    fn invalid_ports_do_not_survive_normalization() {
        assert_eq!(normalize_port("65535"), Some("65535".into()));
        assert_eq!(normalize_port("0"), None);
        assert_eq!(normalize_port("65536"), None);
        assert_eq!(normalize_port("22x"), None);
    }

    #[test]
    fn legacy_mount_state_deserializes_with_runtime_defaults() {
        let state: MountState = serde_json::from_str(
            r#"{"pid":42,"server_id":"alpha","remote":"alpha:","mountpoint":"R:","log":"alpha.log","rc_addr":"127.0.0.1:5572"}"#,
        )
        .unwrap();

        assert_eq!(state.phase, MountPhase::Mounted);
        assert_eq!(state.process_started_at, None);
        assert!(state.rclone.as_os_str().is_empty());
    }
}
