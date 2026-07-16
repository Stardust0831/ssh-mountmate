use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SETTINGS_SCHEMA_VERSION: u32 = 14;
pub const DEFAULT_VFS_UPLOAD_TRANSFERS: u16 = 4;
pub const MIN_VFS_UPLOAD_TRANSFERS: u16 = 1;
pub const MAX_VFS_UPLOAD_TRANSFERS: u16 = 32;

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
    "5s".into()
}

fn default_dir_cache_time() -> String {
    "5m".into()
}

fn default_vfs_upload_transfers() -> u16 {
    DEFAULT_VFS_UPLOAD_TRANSFERS
}

fn default_mount_backend() -> MountBackend {
    MountBackend::Fuse
}

fn default_credential_storage() -> CredentialStorage {
    CredentialStorage::Obscure
}

fn default_appearance_mode() -> AppearanceMode {
    AppearanceMode::System
}

fn default_accent_color() -> AccentColor {
    AccentColor::Blue
}

fn default_font_scale() -> FontScale {
    FontScale::Standard
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceMode {
    #[default]
    System,
    Light,
    Dark,
}

impl AppearanceMode {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccentColor {
    #[default]
    Blue,
    Green,
    Amber,
    Purple,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontScale {
    Small,
    #[default]
    Standard,
    Large,
    ExtraLarge,
}

impl FontScale {
    pub const ALL: [Self; 4] = [Self::Small, Self::Standard, Self::Large, Self::ExtraLarge];

    pub const fn factor(self) -> f32 {
        match self {
            Self::Small => 0.9,
            Self::Standard => 1.0,
            Self::Large => 1.15,
            Self::ExtraLarge => 1.3,
        }
    }
}

impl AccentColor {
    pub const ALL: [Self; 4] = [Self::Blue, Self::Green, Self::Amber, Self::Purple];
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
    Interactive,
}

impl ConnectionMethod {
    pub const ALL: [Self; 3] = [Self::Native, Self::Openssh, Self::Interactive];
}

impl fmt::Display for ConnectionMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Native => "Native SFTP",
            Self::Openssh => "OpenSSH",
            Self::Interactive => "Interactive shared SSH",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MountBackend {
    #[default]
    Fuse,
    Nfs,
}

impl MountBackend {
    pub const ALL: [Self; 2] = [Self::Fuse, Self::Nfs];
}

impl fmt::Display for MountBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Fuse => "FUSE",
            Self::Nfs => "rclone built-in NFS (Experimental)",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStorage {
    #[default]
    Obscure,
    System,
}

impl CredentialStorage {
    pub const ALL: [Self; 2] = [Self::Obscure, Self::System];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub folder: String,
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
    #[serde(default)]
    pub password_credential: String,
    #[serde(default)]
    pub key_pass_credential: String,
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
    #[serde(default)]
    pub ssh_config_path: String,
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
            folder: String::new(),
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
            password_credential: String::new(),
            key_pass_credential: String::new(),
            connection_method: default_connection_method(),
            remote_path: String::new(),
            mountpoint: String::new(),
            cache_mode: String::new(),
            network_mode: false,
            ssh_config_managed: false,
            copy_key_to_ssh_dir: false,
            managed_ssh_config_path: String::new(),
            ssh_config_path: String::new(),
        }
    }
}

impl ServerConfig {
    pub fn normalize(&mut self) {
        let display_name = self.display_name().to_owned();
        let id_source = if self.id.trim().is_empty() {
            [&self.name, &self.host_alias, &self.host]
                .into_iter()
                .find(|value| !value.trim().is_empty())
                .map_or("", String::as_str)
        } else {
            &self.id
        };
        self.id = sanitize_id(id_source);
        if self.name.trim().is_empty() {
            self.name = display_name;
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
        if self.connection_method != ConnectionMethod::Interactive
            && self.mode == "ssh_config"
            && !self.host_alias.is_empty()
        {
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
            format!("{}:{path}", self.remote_name())
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
    #[serde(default = "default_vfs_upload_transfers")]
    pub vfs_upload_transfers: u16,
    #[serde(default = "default_mount_backend")]
    pub macos_mount_backend: MountBackend,
    #[serde(default = "default_credential_storage")]
    pub credential_storage: CredentialStorage,
    #[serde(default)]
    pub startup_all: bool,
    #[serde(default = "default_true")]
    pub auto_show_transfers: bool,
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_appearance_mode")]
    pub appearance_mode: AppearanceMode,
    #[serde(default = "default_accent_color")]
    pub accent_color: AccentColor,
    #[serde(default = "default_font_scale")]
    pub font_scale: FontScale,
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
            vfs_upload_transfers: default_vfs_upload_transfers(),
            macos_mount_backend: default_mount_backend(),
            credential_storage: default_credential_storage(),
            startup_all: false,
            auto_show_transfers: true,
            auto_check_updates: true,
            language: default_language(),
            appearance_mode: default_appearance_mode(),
            accent_color: default_accent_color(),
            font_scale: default_font_scale(),
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
        if version < 9
            && self.vfs_cache_mode == "full"
            && self.vfs_cache_max_age == "30m"
            && self.vfs_write_back == "0s"
            && self.dir_cache_time == "5m"
        {
            self.vfs_write_back = default_write_back();
        }
        if !(MIN_VFS_UPLOAD_TRANSFERS..=MAX_VFS_UPLOAD_TRANSFERS)
            .contains(&self.vfs_upload_transfers)
        {
            self.vfs_upload_transfers = default_vfs_upload_transfers();
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
    pub rc_user: String,
    #[serde(default)]
    pub rc_pass: String,
    #[serde(default)]
    pub phase: MountPhase,
    #[serde(default)]
    pub process_started_at: Option<u64>,
    #[serde(default)]
    pub rclone: PathBuf,
    #[serde(default = "default_mount_backend")]
    pub mount_backend: MountBackend,
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
        assert_eq!(settings.vfs_write_back, "5s");
        assert_eq!(settings.dir_cache_time, "5m");
    }

    #[test]
    fn recommended_zero_delay_migrates_but_custom_profiles_are_preserved() {
        let recommended = Settings {
            settings_schema_version: 8,
            vfs_cache_mode: "full".into(),
            vfs_cache_max_age: "30m".into(),
            vfs_write_back: "0s".into(),
            dir_cache_time: "5m".into(),
            ..Settings::default()
        }
        .migrate();
        assert_eq!(recommended.vfs_write_back, "5s");

        let custom = Settings {
            settings_schema_version: 8,
            vfs_cache_mode: "full".into(),
            vfs_cache_max_age: "2h".into(),
            vfs_write_back: "0s".into(),
            dir_cache_time: "5m".into(),
            ..Settings::default()
        }
        .migrate();
        assert_eq!(custom.vfs_write_back, "0s");
    }

    #[test]
    fn upload_transfer_defaults_and_invalid_values_migrate_safely() {
        let missing: Settings = serde_json::from_str(r#"{"settings_schema_version":9}"#).unwrap();
        assert_eq!(
            missing.migrate().vfs_upload_transfers,
            DEFAULT_VFS_UPLOAD_TRANSFERS
        );

        for invalid in [0, MAX_VFS_UPLOAD_TRANSFERS + 1] {
            let settings = Settings {
                vfs_upload_transfers: invalid,
                ..Settings::default()
            }
            .migrate();
            assert_eq!(settings.vfs_upload_transfers, DEFAULT_VFS_UPLOAD_TRANSFERS);
        }

        let custom = Settings {
            vfs_upload_transfers: 12,
            ..Settings::default()
        }
        .migrate();
        assert_eq!(custom.vfs_upload_transfers, 12);
    }

    #[test]
    fn upload_transfer_limit_serializes_as_a_typed_number() {
        let settings = Settings {
            vfs_upload_transfers: 8,
            ..Settings::default()
        };
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["vfs_upload_transfers"], 8);
        assert_eq!(
            serde_json::from_value::<Settings>(json)
                .unwrap()
                .vfs_upload_transfers,
            8
        );
    }

    #[test]
    fn legacy_settings_and_state_default_to_fuse() {
        let settings: Settings = serde_json::from_str(r#"{"settings_schema_version":10}"#).unwrap();
        assert_eq!(settings.migrate().macos_mount_backend, MountBackend::Fuse);

        let state: MountState = serde_json::from_str(
            r#"{"pid":42,"server_id":"alpha","remote":"alpha:","mountpoint":"/tmp/mnt","log":"/tmp/alpha.log","rc_addr":"127.0.0.1:1234"}"#,
        )
        .unwrap();
        assert_eq!(state.mount_backend, MountBackend::Fuse);
    }

    #[test]
    fn mount_backend_serializes_as_a_typed_setting() {
        let settings = Settings {
            macos_mount_backend: MountBackend::Nfs,
            ..Settings::default()
        };
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["macos_mount_backend"], "nfs");
        assert_eq!(
            serde_json::from_value::<Settings>(json)
                .unwrap()
                .macos_mount_backend,
            MountBackend::Nfs
        );
    }

    #[test]
    fn credential_storage_is_opt_in_and_legacy_settings_remain_obscured() {
        let legacy: Settings = serde_json::from_str(r#"{"settings_schema_version":11}"#).unwrap();
        assert_eq!(
            legacy.migrate().credential_storage,
            CredentialStorage::Obscure
        );
        assert_eq!(
            Settings::default().credential_storage,
            CredentialStorage::Obscure
        );

        let settings = Settings {
            credential_storage: CredentialStorage::System,
            ..Settings::default()
        };
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["credential_storage"], "system");
    }

    #[test]
    fn appearance_settings_are_typed_persisted_and_migrated() {
        let legacy: Settings = serde_json::from_str(r#"{"settings_schema_version":12}"#).unwrap();
        let migrated = legacy.migrate();
        assert_eq!(migrated.settings_schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(migrated.appearance_mode, AppearanceMode::System);
        assert_eq!(migrated.accent_color, AccentColor::Blue);

        let settings = Settings {
            appearance_mode: AppearanceMode::Dark,
            accent_color: AccentColor::Purple,
            ..Settings::default()
        };
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["appearance_mode"], "dark");
        assert_eq!(json["accent_color"], "purple");
        assert_eq!(serde_json::from_value::<Settings>(json).unwrap(), settings);
    }

    #[test]
    fn settings_schema_13_defaults_to_standard_font_scale() {
        let legacy: Settings = serde_json::from_str(r#"{"settings_schema_version":13}"#).unwrap();
        let migrated = legacy.migrate();
        assert_eq!(migrated.settings_schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(migrated.font_scale, FontScale::Standard);
    }

    #[test]
    fn interactive_connection_method_is_typed_and_opt_in() {
        let legacy: ServerConfig = serde_json::from_str(r#"{"id":"legacy"}"#).unwrap();
        assert_eq!(legacy.connection_method, ConnectionMethod::Native);

        let interactive = ServerConfig {
            id: "interactive".into(),
            connection_method: ConnectionMethod::Interactive,
            ..ServerConfig::default()
        };
        let json = serde_json::to_value(&interactive).unwrap();
        assert_eq!(json["connection_method"], "interactive");
        assert_eq!(
            serde_json::from_value::<ServerConfig>(json)
                .unwrap()
                .connection_method,
            ConnectionMethod::Interactive
        );
    }

    #[test]
    fn openssh_ssh_config_remote_keeps_host_alias() {
        let server = ServerConfig {
            id: "openssh-profile".into(),
            mode: "ssh_config".into(),
            host_alias: "cluster-login".into(),
            connection_method: ConnectionMethod::Openssh,
            ..ServerConfig::default()
        };

        assert_eq!(server.remote_name(), "cluster-login");
    }

    #[test]
    fn legacy_server_defaults_to_no_folder() {
        let server: ServerConfig = serde_json::from_str(r#"{"id":"legacy"}"#).unwrap();
        assert!(server.folder.is_empty());
    }

    #[test]
    fn interactive_ssh_config_remote_uses_unique_server_id() {
        let server = ServerConfig {
            id: "interactive-profile".into(),
            mode: "ssh_config".into(),
            host_alias: "cluster-login".into(),
            connection_method: ConnectionMethod::Interactive,
            ..ServerConfig::default()
        };

        assert_eq!(server.remote_name(), "interactive-profile");
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
    fn remote_spec_preserves_relative_and_absolute_paths() {
        let mut server = ServerConfig {
            id: "alpha".into(),
            remote_path: "relative/path".into(),
            ..ServerConfig::default()
        };
        assert_eq!(server.remote_spec(), "alpha:relative/path");
        server.remote_path = "/absolute/path".into();
        assert_eq!(server.remote_spec(), "alpha:/absolute/path");
    }

    #[test]
    fn blank_servers_receive_unique_generated_ids() {
        let mut first = ServerConfig::default();
        let mut second = ServerConfig::default();
        first.normalize();
        second.normalize();
        assert_ne!(first.id, second.id);
        assert_eq!(first.name, "Server");
        assert_eq!(second.name, "Server");
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
        assert!(state.rc_user.is_empty());
        assert!(state.rc_pass.is_empty());
    }
}
