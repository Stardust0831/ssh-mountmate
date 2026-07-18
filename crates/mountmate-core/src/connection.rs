use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::model::{normalize_port, normalize_tags, sanitize_id};
use crate::mountpoint::HOME_MOUNTPOINT_VALUE;
use crate::{AuthMethod, ConnectionMethod, ServerConfig};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConnectionSource {
    #[default]
    Manual,
    SshConfig,
    SshConfigBatch,
    SaiCluster,
}

impl ConnectionSource {
    pub const ALL: [Self; 4] = [
        Self::Manual,
        Self::SshConfig,
        Self::SshConfigBatch,
        Self::SaiCluster,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::SshConfig => "ssh_config",
            Self::SshConfigBatch => "ssh_config_batch",
            Self::SaiCluster => "sai_cluster",
        }
    }

    fn from_server(server: &ServerConfig) -> Self {
        match server.source.as_str() {
            "ssh_config" => Self::SshConfig,
            "ssh_config_batch" => Self::SshConfigBatch,
            "sai_cluster" => Self::SaiCluster,
            _ => Self::Manual,
        }
    }
}

impl fmt::Display for ConnectionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Manual => "Manual",
            Self::SshConfig => "SSH config",
            Self::SshConfigBatch => "SSH config (batch)",
            Self::SaiCluster => "SAI cluster",
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionDraft {
    pub editing_id: Option<String>,
    pub source: ConnectionSource,
    pub name: String,
    pub folder: String,
    pub tags: Vec<String>,
    pub host_alias: String,
    pub host: String,
    pub user: String,
    pub port: String,
    pub auth: AuthMethod,
    pub key_file: String,
    pub password: String,
    pub key_passphrase: String,
    pub connection_method: ConnectionMethod,
    pub remote_path: String,
    pub mountpoint: String,
    pub auto_mount_at_login: bool,
    pub ssh_config_managed: bool,
    pub copy_key_to_ssh_dir: bool,
    pub ssh_config_path: String,
    existing: Option<ServerConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreservedSecretState {
    Absent,
    Obscured,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionRequirements {
    pub name: bool,
    pub host_alias: bool,
    pub host: bool,
    pub user: bool,
    pub port: bool,
    pub ssh_config_path: bool,
    pub password: bool,
    pub key_file: bool,
}

impl fmt::Debug for ConnectionDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionDraft")
            .field("editing_id", &self.editing_id)
            .field("source", &self.source)
            .field("name", &self.name)
            .field("host", &self.host)
            .field("user", &self.user)
            .field(
                "password",
                &(!self.password.is_empty()).then_some("<redacted>"),
            )
            .field(
                "key_passphrase",
                &(!self.key_passphrase.is_empty()).then_some("<redacted>"),
            )
            .finish_non_exhaustive()
    }
}

impl Default for ConnectionDraft {
    fn default() -> Self {
        Self {
            editing_id: None,
            source: ConnectionSource::Manual,
            name: String::new(),
            folder: String::new(),
            tags: Vec::new(),
            host_alias: String::new(),
            host: String::new(),
            user: String::new(),
            port: "22".into(),
            auth: AuthMethod::Key,
            key_file: String::new(),
            password: String::new(),
            key_passphrase: String::new(),
            connection_method: ConnectionMethod::Native,
            remote_path: String::new(),
            mountpoint: String::new(),
            auto_mount_at_login: false,
            ssh_config_managed: false,
            copy_key_to_ssh_dir: false,
            ssh_config_path: String::new(),
            existing: None,
        }
    }
}

impl ConnectionDraft {
    pub fn requirements(&self) -> ConnectionRequirements {
        let native = self.connection_method == ConnectionMethod::Native;
        let ssh_config_source = matches!(
            self.source,
            ConnectionSource::SshConfig | ConnectionSource::SshConfigBatch
        );
        ConnectionRequirements {
            name: true,
            host_alias: ssh_config_source,
            host: true,
            user: true,
            port: true,
            ssh_config_path: ssh_config_source,
            password: native
                && self.auth == AuthMethod::Password
                && !self.preserves_password_for_current_target(),
            key_file: native && self.auth == AuthMethod::Key && !ssh_config_source,
        }
    }

    fn preserves_password_for_current_target(&self) -> bool {
        let Some(existing) = self.existing.as_ref().filter(|existing| {
            !existing.password_obscured.is_empty() || !existing.password_credential.is_empty()
        }) else {
            return false;
        };
        let Ok(mountpoint) = normalize_mountpoint(&self.mountpoint) else {
            return false;
        };
        existing.source == self.source.as_str()
            && existing.auth == AuthMethod::Password
            && existing.connection_method == ConnectionMethod::Native
            && existing.host.trim().eq_ignore_ascii_case(self.host.trim())
            && existing.user.trim() == self.user.trim()
            && normalize_port(&existing.port) == normalize_port(&self.port)
            && normalize_remote_path(&existing.remote_path)
                == normalize_remote_path(&self.remote_path)
            && normalize_mountpoint(&existing.mountpoint).ok().as_deref()
                == Some(mountpoint.as_str())
    }

    pub fn from_server(server: &ServerConfig) -> Self {
        Self {
            editing_id: Some(server.id.clone()),
            source: ConnectionSource::from_server(server),
            name: server.name.clone(),
            folder: server.folder.clone(),
            tags: server.tags.clone(),
            host_alias: server.host_alias.clone(),
            host: server.host.clone(),
            user: server.user.clone(),
            port: server.port.clone(),
            auth: server.auth,
            key_file: server.key_file.clone(),
            password: String::new(),
            key_passphrase: String::new(),
            connection_method: server.connection_method,
            remote_path: server.remote_path.clone(),
            mountpoint: server.mountpoint.clone(),
            auto_mount_at_login: server.auto_mount_at_login,
            ssh_config_managed: server.ssh_config_managed,
            copy_key_to_ssh_dir: server.copy_key_to_ssh_dir,
            ssh_config_path: server.ssh_config_path.clone(),
            existing: Some(server.clone()),
        }
    }

    /// Explicitly forget a preserved secret while editing. Text inputs remain
    /// blank for both "keep" and "clear", so the UI must call this method for
    /// an intentional clear instead of inferring it from an empty input.
    pub fn clear_preserved_secret(&mut self, kind: crate::credential::CredentialKind) {
        let Some(existing) = &mut self.existing else {
            return;
        };
        match kind {
            crate::credential::CredentialKind::Password => {
                self.password.clear();
                existing.password_obscured.clear();
                existing.password_credential.clear();
            }
            crate::credential::CredentialKind::KeyPassphrase => {
                self.key_passphrase.clear();
                existing.key_pass_obscured.clear();
                existing.key_pass_credential.clear();
            }
        }
    }

    pub fn preserved_secret_state(
        &self,
        kind: crate::credential::CredentialKind,
    ) -> PreservedSecretState {
        let Some(existing) = &self.existing else {
            return PreservedSecretState::Absent;
        };
        let (obscured, credential) = match kind {
            crate::credential::CredentialKind::Password => {
                (&existing.password_obscured, &existing.password_credential)
            }
            crate::credential::CredentialKind::KeyPassphrase => {
                (&existing.key_pass_obscured, &existing.key_pass_credential)
            }
        };
        if !credential.is_empty() {
            PreservedSecretState::System
        } else if !obscured.is_empty() {
            PreservedSecretState::Obscured
        } else {
            PreservedSecretState::Absent
        }
    }

    pub fn apply_source_defaults(&mut self) {
        if self.source == ConnectionSource::SaiCluster {
            self.host = "c1.sai.ai-4s.com".into();
            self.port = "12022".into();
            self.auth = AuthMethod::Key;
            self.connection_method = ConnectionMethod::Native;
            self.ssh_config_managed = true;
            self.copy_key_to_ssh_dir = true;
            self.apply_sai_name();
        }
    }

    pub fn apply_sai_name(&mut self) {
        if self.source != ConnectionSource::SaiCluster || self.user.trim().is_empty() {
            return;
        }
        let name = format!("SAI-{}", self.user.trim());
        if self.name.trim().is_empty() || self.name == "SAI" || self.name.starts_with("SAI-") {
            self.name.clone_from(&name);
        }
        if self.host_alias.trim().is_empty()
            || self.host_alias == "SAI"
            || self.host_alias.starts_with("SAI-")
        {
            self.host_alias = name;
        }
    }

    pub fn apply_imported_server(&mut self, server: &ServerConfig) {
        self.source = ConnectionSource::SshConfig;
        self.name = server.name.clone();
        self.host_alias = server.host_alias.clone();
        self.host = server.host.clone();
        self.user = server.user.clone();
        self.port = server.port.clone();
        self.auth = server.auth;
        self.key_file = server.key_file.clone();
        self.connection_method = server.connection_method;
        self.ssh_config_path = server.ssh_config_path.clone();
    }

    pub fn validate(&self, servers: &[ServerConfig]) -> Result<ValidatedConnection, DraftError> {
        let requirements = self.requirements();
        let name = required_display_name(&self.name)?;
        let tags = validate_tags(&self.tags, &self.folder)?;
        let folder = tags.first().cloned().unwrap_or_default();
        let host = required_scalar(&self.host, "IP/Host")?;
        let user = required_scalar(&self.user, "User")?;
        let port = normalize_port(&self.port).ok_or(DraftError::InvalidPort)?;
        let connection_method = self.connection_method;
        let auth = if connection_method != ConnectionMethod::Native {
            AuthMethod::Key
        } else {
            self.auth
        };
        let key_file = self.key_file.trim().to_owned();
        if requirements.key_file {
            validate_private_key(&key_file)?;
        }
        if requirements.ssh_config_path && self.ssh_config_path.trim().is_empty() {
            return Err(DraftError::Required("SSH config file"));
        }
        let mountpoint = normalize_mountpoint(&self.mountpoint)?;
        let remote_path = normalize_remote_path(&self.remote_path);
        let mut host_alias = self.host_alias.trim().to_owned();
        let ssh_config_managed = self.ssh_config_managed
            && !matches!(
                self.source,
                ConnectionSource::SshConfig | ConnectionSource::SshConfigBatch
            );
        if ssh_config_managed && host_alias.is_empty() {
            host_alias = sanitize_id(&name);
        }
        if matches!(
            self.source,
            ConnectionSource::SshConfig | ConnectionSource::SshConfigBatch
        ) || ssh_config_managed
        {
            validate_host_alias(&host_alias)?;
        }

        let mut id = self
            .editing_id
            .clone()
            .unwrap_or_else(|| unique_id(&sanitize_id(&name), servers));
        if id.trim().is_empty() {
            id = unique_id(&sanitize_id(&name), servers);
        }
        let mut server = ServerConfig {
            id,
            name,
            folder,
            tags,
            mode: if matches!(
                self.source,
                ConnectionSource::SshConfig | ConnectionSource::SshConfigBatch
            ) {
                "ssh_config".into()
            } else {
                "manual".into()
            },
            source: self.source.as_str().into(),
            host_alias,
            host,
            user,
            port,
            auth,
            key_file,
            password_obscured: String::new(),
            key_pass_obscured: String::new(),
            password_credential: self
                .existing
                .as_ref()
                .map_or_else(String::new, |server| server.password_credential.clone()),
            key_pass_credential: self
                .existing
                .as_ref()
                .map_or_else(String::new, |server| server.key_pass_credential.clone()),
            connection_method,
            remote_path,
            mountpoint,
            auto_mount_at_login: self.auto_mount_at_login,
            cache_mode: self
                .existing
                .as_ref()
                .map_or_else(String::new, |server| server.cache_mode.clone()),
            network_mode: self
                .existing
                .as_ref()
                .is_some_and(|server| server.network_mode),
            ssh_config_managed,
            copy_key_to_ssh_dir: self.copy_key_to_ssh_dir
                && ssh_config_managed
                && auth == AuthMethod::Key,
            managed_ssh_config_path: self
                .existing
                .as_ref()
                .map_or_else(String::new, |server| server.managed_ssh_config_path.clone()),
            ssh_config_path: if matches!(
                self.source,
                ConnectionSource::SshConfig | ConnectionSource::SshConfigBatch
            ) {
                self.ssh_config_path.trim().into()
            } else {
                String::new()
            },
        };
        server.normalize();

        if let Some(duplicate) = servers.iter().find(|candidate| {
            self.editing_id.as_deref() != Some(candidate.id.as_str())
                && connection_fingerprint(candidate) == connection_fingerprint(&server)
        }) {
            return Err(DraftError::Duplicate(duplicate.display_name().into()));
        }

        let password = match auth {
            AuthMethod::Password if !self.password.is_empty() => {
                SecretAction::Obscure(self.password.clone())
            }
            AuthMethod::Password if !requirements.password => self
                .existing
                .as_ref()
                .filter(|existing| same_password_target(existing, &server))
                .map(|existing| SecretAction::Keep(existing.password_obscured.clone()))
                .ok_or(DraftError::PasswordRequired)?,
            AuthMethod::Password => return Err(DraftError::PasswordRequired),
            AuthMethod::Key => SecretAction::Clear,
        };
        let key_passphrase =
            if auth == AuthMethod::Key && connection_method == ConnectionMethod::Native {
                if !self.key_passphrase.is_empty() {
                    SecretAction::Obscure(self.key_passphrase.clone())
                } else {
                    self.existing
                        .as_ref()
                        .filter(|existing| same_key_target(existing, &server))
                        .map_or(SecretAction::Clear, |existing| {
                            if existing.key_pass_obscured.is_empty()
                                && existing.key_pass_credential.is_empty()
                            {
                                SecretAction::Clear
                            } else {
                                SecretAction::Keep(existing.key_pass_obscured.clone())
                            }
                        })
                }
            } else {
                SecretAction::Clear
            };
        Ok(ValidatedConnection {
            server,
            password,
            key_passphrase,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum SecretAction {
    Clear,
    Keep(String),
    Obscure(String),
}

impl fmt::Debug for SecretAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Clear => "Clear",
            Self::Keep(_) => "Keep(<redacted>)",
            Self::Obscure(_) => "Obscure(<redacted>)",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedConnection {
    pub server: ServerConfig,
    pub password: SecretAction,
    pub key_passphrase: SecretAction,
}

impl ValidatedConnection {
    pub fn apply_secrets(
        mut self,
        password: Option<String>,
        key_passphrase: Option<String>,
    ) -> Result<ServerConfig, DraftError> {
        if !matches!(self.password, SecretAction::Keep(_)) {
            self.server.password_credential.clear();
        }
        if !matches!(self.key_passphrase, SecretAction::Keep(_)) {
            self.server.key_pass_credential.clear();
        }
        self.server.password_obscured = resolved_secret(self.password, password)?;
        self.server.key_pass_obscured = resolved_secret(self.key_passphrase, key_passphrase)?;
        Ok(self.server)
    }
}

fn resolved_secret(action: SecretAction, obscured: Option<String>) -> Result<String, DraftError> {
    match action {
        SecretAction::Clear => Ok(String::new()),
        SecretAction::Keep(value) => Ok(value),
        SecretAction::Obscure(_) => obscured
            .filter(|value| !value.is_empty())
            .ok_or(DraftError::SecretNotObscured),
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DraftError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{0} must not contain whitespace or control characters")]
    InvalidScalar(&'static str),
    #[error("Name must not contain control characters")]
    InvalidName,
    #[error("Folder must not contain control characters")]
    InvalidFolder,
    #[error("Port must be a number from 1 to 65535")]
    InvalidPort,
    #[error("Select a private key file")]
    KeyRequired,
    #[error("Key file not found: {0}")]
    KeyMissing(String),
    #[error("Select the private key file, not the .pub public key file")]
    PublicKey,
    #[error("SSH Host is invalid")]
    InvalidHostAlias,
    #[error("Custom mountpoint must be an absolute path or start with ~")]
    InvalidMountpoint,
    #[error("A connection for the same target already exists: {0}")]
    Duplicate(String),
    #[error("Password is required")]
    PasswordRequired,
    #[error("A secret could not be safely obscured")]
    SecretNotObscured,
    #[error("The SSH import plan is inconsistent")]
    InvalidImportPlan,
    #[error("The selected import action is not allowed for SSH Host {0}")]
    InvalidImportAction(String),
}

fn required_scalar(value: &str, field: &'static str) -> Result<String, DraftError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DraftError::Required(field));
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(DraftError::InvalidScalar(field));
    }
    Ok(value.into())
}

fn required_display_name(value: &str) -> Result<String, DraftError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DraftError::Required("Name"));
    }
    if value.chars().any(char::is_control) {
        return Err(DraftError::InvalidName);
    }
    Ok(value.into())
}

fn validate_tags(tags: &[String], legacy_folder: &str) -> Result<Vec<String>, DraftError> {
    if tags.iter().any(|tag| tag.chars().any(char::is_control))
        || legacy_folder.chars().any(char::is_control)
    {
        return Err(DraftError::InvalidFolder);
    }
    let mut normalized = tags.to_vec();
    normalize_tags(&mut normalized, legacy_folder);
    Ok(normalized)
}

fn validate_private_key(value: &str) -> Result<(), DraftError> {
    if value.is_empty() {
        return Err(DraftError::KeyRequired);
    }
    let path = expand_home(Path::new(value));
    if !path.is_file() {
        return Err(DraftError::KeyMissing(value.into()));
    }
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pub"))
    {
        return Err(DraftError::PublicKey);
    }
    Ok(())
}

fn validate_host_alias(value: &str) -> Result<(), DraftError> {
    let valid = !value.is_empty()
        && !value.starts_with('-')
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | ':')
        });
    if valid {
        Ok(())
    } else {
        Err(DraftError::InvalidHostAlias)
    }
}

fn normalize_remote_path(value: &str) -> String {
    let value = value.trim().replace('\\', "/");
    if value == "~" {
        String::new()
    } else if value.starts_with('/') {
        let suffix = value.trim_matches('/');
        if suffix.is_empty() {
            "/".into()
        } else {
            format!("/{suffix}")
        }
    } else {
        value.trim_matches('/').into()
    }
}

fn normalize_mountpoint(value: &str) -> Result<String, DraftError> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        return Ok(String::new());
    }
    if value == HOME_MOUNTPOINT_VALUE || windows_drive(value).is_some() {
        return Ok(value.trim_end_matches(['\\', '/']).into());
    }
    if value.starts_with("~/") || value.starts_with("~\\") || absolute_path(value) {
        Ok(value.into())
    } else {
        Err(DraftError::InvalidMountpoint)
    }
}

fn absolute_path(value: &str) -> bool {
    Path::new(value).is_absolute()
        || (value.len() >= 3
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'\\' | b'/'))
        || value.starts_with("\\\\")
        || value.starts_with("//")
}

fn windows_drive(value: &str) -> Option<char> {
    let bytes = value.as_bytes();
    matches!(bytes, [letter, b':'] | [letter, b':', b'\\' | b'/'] if letter.is_ascii_alphabetic())
        .then(|| char::from(bytes[0]).to_ascii_uppercase())
}

fn connection_fingerprint(server: &ServerConfig) -> (String, String, String, String, String) {
    (
        server.host.trim().to_ascii_lowercase(),
        server.user.trim().into(),
        normalize_port(&server.port).unwrap_or_else(|| server.port.trim().into()),
        normalize_remote_path(&server.remote_path),
        normalize_mountpoint(&server.mountpoint)
            .unwrap_or_else(|_| server.mountpoint.trim().into()),
    )
}

pub fn same_connection_target(left: &ServerConfig, right: &ServerConfig) -> bool {
    connection_fingerprint(left) == connection_fingerprint(right)
}

fn same_password_target(existing: &ServerConfig, candidate: &ServerConfig) -> bool {
    existing.source == candidate.source
        && existing.host_alias == candidate.host_alias
        && existing.host == candidate.host
        && existing.user == candidate.user
        && normalize_port(&existing.port) == normalize_port(&candidate.port)
        && existing.auth == candidate.auth
        && existing.connection_method == candidate.connection_method
}

fn same_key_target(existing: &ServerConfig, candidate: &ServerConfig) -> bool {
    existing.auth == candidate.auth
        && existing.key_file == candidate.key_file
        && existing.connection_method == candidate.connection_method
}

fn unique_id(base: &str, servers: &[ServerConfig]) -> String {
    if !servers.iter().any(|server| server.id == base) {
        return base.into();
    }
    (2..)
        .map(|index| format!("{base}-{index}"))
        .find(|candidate| !servers.iter().any(|server| &server.id == candidate))
        .expect("an unused numeric connection suffix always exists")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStatus {
    New,
    Same,
    SameHost,
    SameTarget,
    Invalid,
}

impl fmt::Display for ImportStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::New => "New",
            Self::Same => "Same configuration",
            Self::SameHost => "Same SSH Host",
            Self::SameTarget => "Same target",
            Self::Invalid => "Invalid",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportAction {
    Ignore,
    Import,
    Overwrite,
}

impl ImportAction {
    pub const ALL: [Self; 3] = [Self::Ignore, Self::Import, Self::Overwrite];
}

impl fmt::Display for ImportAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Ignore => "Ignore",
            Self::Import => "Import",
            Self::Overwrite => "Overwrite",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshImportItem {
    pub host_alias: String,
    pub status: ImportStatus,
    pub reason: String,
    pub server: Option<ServerConfig>,
    pub matched_id: Option<String>,
    pub matched_name: Option<String>,
    pub can_overwrite: bool,
    pub overwrite_protected: bool,
}

impl SshImportItem {
    pub fn default_action(&self) -> ImportAction {
        if self.status == ImportStatus::New {
            ImportAction::Import
        } else {
            ImportAction::Ignore
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshImportPlan {
    pub items: Vec<SshImportItem>,
}

impl SshImportPlan {
    pub fn apply(
        &self,
        actions: &[ImportAction],
        existing: &[ServerConfig],
    ) -> Result<Vec<ServerConfig>, DraftError> {
        if actions.len() != self.items.len() {
            return Err(DraftError::InvalidImportPlan);
        }
        let mut selected = Vec::new();
        for (item, action) in self.items.iter().zip(actions) {
            match action {
                ImportAction::Ignore => {}
                ImportAction::Import if item.status == ImportStatus::New => {
                    if let Some(server) = &item.server {
                        selected.push(server.clone());
                    }
                }
                ImportAction::Overwrite if item.can_overwrite => {
                    let Some(server) = &item.server else {
                        return Err(DraftError::InvalidImportPlan);
                    };
                    let Some(existing) = item
                        .matched_id
                        .as_deref()
                        .and_then(|id| existing.iter().find(|server| server.id == id))
                    else {
                        return Err(DraftError::InvalidImportPlan);
                    };
                    selected.push(merge_imported_connection(existing, server));
                }
                _ => return Err(DraftError::InvalidImportAction(item.host_alias.clone())),
            }
        }
        Ok(selected)
    }
}

pub fn plan_ssh_imports(
    imports: Vec<(String, Result<ServerConfig, String>)>,
    existing: &[ServerConfig],
    protected_ids: &std::collections::HashSet<String>,
) -> SshImportPlan {
    let mut known = existing.to_vec();
    let mut items = Vec::with_capacity(imports.len());
    for (host_alias, result) in imports {
        let mut server = match result {
            Ok(server) => server,
            Err(reason) => {
                items.push(SshImportItem {
                    host_alias,
                    status: ImportStatus::Invalid,
                    reason,
                    server: None,
                    matched_id: None,
                    matched_name: None,
                    can_overwrite: false,
                    overwrite_protected: false,
                });
                continue;
            }
        };
        server.id = unique_id(&sanitize_id(server.display_name()), &known);
        let duplicate = import_duplicate(&server, &known);
        let (status, matched) = duplicate.map_or((ImportStatus::New, None), |(status, server)| {
            (status, Some(server))
        });
        let existing_match =
            matched.filter(|matched| existing.iter().any(|item| item.id == matched.id));
        let overwrite_protected =
            existing_match.is_some_and(|matched| protected_ids.contains(&matched.id));
        let can_overwrite = existing_match.is_some() && !overwrite_protected;
        items.push(SshImportItem {
            host_alias,
            status,
            reason: import_reason(status, overwrite_protected).into(),
            server: Some(server.clone()),
            matched_id: existing_match.map(|matched| matched.id.clone()),
            matched_name: existing_match.map(|matched| matched.display_name().into()),
            can_overwrite,
            overwrite_protected,
        });
        if status == ImportStatus::New {
            known.push(server);
        }
    }
    SshImportPlan { items }
}

fn import_duplicate<'a>(
    candidate: &ServerConfig,
    known: &'a [ServerConfig],
) -> Option<(ImportStatus, &'a ServerConfig)> {
    let alias = candidate.host_alias.trim().to_ascii_lowercase();
    let target = import_target(candidate);
    known
        .iter()
        .find(|server| {
            server.host_alias.trim().eq_ignore_ascii_case(&alias) && import_target(server) == target
        })
        .map(|server| (ImportStatus::Same, server))
        .or_else(|| {
            (!alias.is_empty()).then(|| {
                known
                    .iter()
                    .find(|server| server.host_alias.trim().eq_ignore_ascii_case(&alias))
                    .map(|server| (ImportStatus::SameHost, server))
            })?
        })
        .or_else(|| {
            known
                .iter()
                .find(|server| import_target(server) == target)
                .map(|server| (ImportStatus::SameTarget, server))
        })
}

fn import_target(server: &ServerConfig) -> (String, String, String) {
    (
        server.host.trim().to_ascii_lowercase(),
        server.user.trim().into(),
        normalize_port(&server.port).unwrap_or_else(|| server.port.trim().into()),
    )
}

fn import_reason(status: ImportStatus, protected: bool) -> &'static str {
    if protected {
        return "The matching connection is mounted or busy";
    }
    match status {
        ImportStatus::New => "",
        ImportStatus::Same => "The same configuration already exists",
        ImportStatus::SameHost => "The same SSH Host already exists",
        ImportStatus::SameTarget => "The same HostName, user, and port already exist",
        ImportStatus::Invalid => "The SSH Host could not be resolved",
    }
}

fn merge_imported_connection(existing: &ServerConfig, imported: &ServerConfig) -> ServerConfig {
    let mut merged = ServerConfig {
        id: existing.id.clone(),
        name: imported.name.clone(),
        mode: imported.mode.clone(),
        source: imported.source.clone(),
        host_alias: imported.host_alias.clone(),
        host: imported.host.clone(),
        user: imported.user.clone(),
        port: imported.port.clone(),
        auth: imported.auth,
        key_file: imported.key_file.clone(),
        connection_method: imported.connection_method,
        ssh_config_path: imported.ssh_config_path.clone(),
        ..existing.clone()
    };
    if merged.auth != AuthMethod::Password || !same_password_target(existing, &merged) {
        merged.password_obscured.clear();
        merged.password_credential.clear();
    }
    if merged.auth != AuthMethod::Key || !same_key_target(existing, &merged) {
        merged.key_pass_obscured.clear();
        merged.key_pass_credential.clear();
    }
    merged.folder = merged.tags.first().cloned().unwrap_or_default();
    merged
}

fn expand_home(path: &Path) -> PathBuf {
    let value = path.as_os_str().to_string_lossy();
    if (value == "~" || value.starts_with("~/") || value.starts_with("~\\"))
        && let Some(directories) = directories::BaseDirs::new()
    {
        return if value == "~" {
            directories.home_dir().into()
        } else {
            directories.home_dir().join(&value[2..])
        };
    }
    path.into()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn password_server() -> ServerConfig {
        ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            host: "host.example".into(),
            user: "alice".into(),
            port: "22".into(),
            auth: AuthMethod::Password,
            password_obscured: "kept-secret".into(),
            ..ServerConfig::default()
        }
    }

    #[test]
    fn editor_requirements_follow_connection_semantics() {
        let manual_key = ConnectionDraft::default();
        assert_eq!(
            manual_key.requirements(),
            ConnectionRequirements {
                name: true,
                host_alias: false,
                host: true,
                user: true,
                port: true,
                ssh_config_path: false,
                password: false,
                key_file: true,
            }
        );

        let ssh_config = ConnectionDraft {
            source: ConnectionSource::SshConfig,
            ..ConnectionDraft::default()
        };
        assert!(ssh_config.requirements().host_alias);
        assert!(ssh_config.requirements().ssh_config_path);
        assert!(!ssh_config.requirements().key_file);

        let openssh = ConnectionDraft {
            connection_method: ConnectionMethod::Openssh,
            ..ConnectionDraft::default()
        };
        assert!(!openssh.requirements().key_file);
        assert!(!openssh.requirements().password);
    }

    #[test]
    fn password_requirement_tracks_whether_the_saved_target_is_still_valid() {
        let existing = password_server();
        let mut draft = ConnectionDraft::from_server(&existing);
        assert!(!draft.requirements().password);

        draft.host = "other.example".into();
        assert!(draft.requirements().password);

        draft.password = "replacement".into();
        assert!(draft.requirements().password);
    }

    #[test]
    fn ssh_config_path_requirement_is_enforced_by_validation() {
        let draft = ConnectionDraft {
            source: ConnectionSource::SshConfig,
            name: "Alpha".into(),
            host_alias: "alpha".into(),
            host: "host.example".into(),
            user: "alice".into(),
            ..ConnectionDraft::default()
        };
        assert_eq!(
            draft.validate(&[]),
            Err(DraftError::Required("SSH config file"))
        );
    }

    #[test]
    fn unchanged_password_target_preserves_obscured_secret() {
        let existing = password_server();
        let draft = ConnectionDraft::from_server(&existing);
        let validated = draft.validate(std::slice::from_ref(&existing)).unwrap();
        assert_eq!(validated.password, SecretAction::Keep("kept-secret".into()));
        assert_eq!(
            validated
                .apply_secrets(None, None)
                .unwrap()
                .password_obscured,
            "kept-secret"
        );
    }

    #[test]
    fn unchanged_password_target_preserves_system_credential_reference() {
        let mut existing = password_server();
        existing.password_obscured.clear();
        existing.password_credential = "ssh-mountmate:alpha:password".into();
        let draft = ConnectionDraft::from_server(&existing);
        let validated = draft.validate(std::slice::from_ref(&existing)).unwrap();
        assert_eq!(validated.password, SecretAction::Keep(String::new()));
        let saved = validated.apply_secrets(None, None).unwrap();
        assert!(saved.password_obscured.is_empty());
        assert_eq!(saved.password_credential, "ssh-mountmate:alpha:password");
    }

    #[test]
    fn auto_mount_at_login_survives_draft_validation() {
        let mut existing = password_server();
        existing.auto_mount_at_login = true;
        let draft = ConnectionDraft::from_server(&existing);
        assert!(draft.auto_mount_at_login);
        assert!(
            draft
                .validate(std::slice::from_ref(&existing))
                .unwrap()
                .server
                .auto_mount_at_login
        );
    }

    #[test]
    fn explicit_secret_clear_is_distinct_from_a_blank_keep_input() {
        let mut existing = password_server();
        existing.password_obscured.clear();
        existing.password_credential = "ssh-mountmate:alpha:password".into();
        let mut draft = ConnectionDraft::from_server(&existing);

        assert_eq!(
            draft.preserved_secret_state(crate::credential::CredentialKind::Password),
            PreservedSecretState::System
        );

        draft.clear_preserved_secret(crate::credential::CredentialKind::Password);

        assert_eq!(
            draft.preserved_secret_state(crate::credential::CredentialKind::Password),
            PreservedSecretState::Absent
        );

        assert_eq!(
            draft.validate(std::slice::from_ref(&existing)),
            Err(DraftError::PasswordRequired)
        );
    }

    #[test]
    fn changed_password_target_requires_a_new_password() {
        let existing = password_server();
        let mut draft = ConnectionDraft::from_server(&existing);
        draft.host = "other.example".into();
        assert_eq!(
            draft.validate(&[existing]),
            Err(DraftError::PasswordRequired)
        );
    }

    #[test]
    fn plaintext_secret_is_only_returned_as_an_obscure_action() {
        let mut draft = ConnectionDraft::from_server(&password_server());
        draft.password = "plain-text".into();
        let validated = draft.validate(&[]).unwrap();
        assert_eq!(
            validated.password,
            SecretAction::Obscure("plain-text".into())
        );
        assert!(validated.server.password_obscured.is_empty());
        assert_eq!(
            validated.apply_secrets(None, None),
            Err(DraftError::SecretNotObscured)
        );
    }

    #[test]
    fn exact_duplicate_target_is_rejected() {
        let existing = password_server();
        let mut draft = ConnectionDraft::from_server(&existing);
        draft.editing_id = None;
        draft.existing = None;
        draft.password = "new".into();
        assert_eq!(
            draft.validate(&[existing]),
            Err(DraftError::Duplicate("Alpha".into()))
        );
    }

    #[test]
    fn display_names_allow_spaces_but_reject_control_characters() {
        let draft = ConnectionDraft {
            name: "Existing Alpha".into(),
            host: "host.example".into(),
            user: "alice".into(),
            auth: AuthMethod::Password,
            password: "secret".into(),
            ..ConnectionDraft::default()
        };
        assert_eq!(draft.validate(&[]).unwrap().server.name, "Existing Alpha");

        let mut invalid = draft;
        invalid.name = "Alpha\nBeta".into();
        assert_eq!(invalid.validate(&[]), Err(DraftError::InvalidName));
    }

    #[test]
    fn folder_round_trip_trims_whitespace_and_rejects_controls() {
        let mut draft = ConnectionDraft {
            name: "Alpha".into(),
            folder: "  Projects / Research  ".into(),
            host: "host.example".into(),
            user: "alice".into(),
            auth: AuthMethod::Password,
            password: "secret".into(),
            ..ConnectionDraft::default()
        };
        let server = draft.validate(&[]).unwrap().server;
        assert_eq!(server.folder, "Projects / Research");
        assert_eq!(ConnectionDraft::from_server(&server).folder, server.folder);

        draft.folder = "Projects\nResearch".into();
        assert_eq!(draft.validate(&[]), Err(DraftError::InvalidFolder));
    }

    #[test]
    fn tags_round_trip_trims_stably_deduplicates_and_preserves_spaces() {
        let draft = ConnectionDraft {
            name: "Alpha".into(),
            tags: vec![
                "  Projects  ".into(),
                "研究 / Team".into(),
                "Projects".into(),
                "".into(),
            ],
            host: "host.example".into(),
            user: "alice".into(),
            auth: AuthMethod::Password,
            password: "secret".into(),
            ..ConnectionDraft::default()
        };
        let server = draft.validate(&[]).unwrap().server;
        assert_eq!(server.tags, vec!["Projects", "研究 / Team"]);
        assert_eq!(server.folder, "Projects");
        let reconstructed = ConnectionDraft::from_server(&server);
        assert_eq!(reconstructed.tags, server.tags);
    }

    #[test]
    fn tags_reject_control_characters() {
        let draft = ConnectionDraft {
            name: "Alpha".into(),
            tags: vec!["Projects\nTeam".into()],
            host: "host.example".into(),
            user: "alice".into(),
            auth: AuthMethod::Password,
            password: "secret".into(),
            ..ConnectionDraft::default()
        };
        assert_eq!(draft.validate(&[]), Err(DraftError::InvalidFolder));
    }

    #[test]
    fn refreshing_an_ssh_import_preserves_user_folder() {
        let mut draft = ConnectionDraft {
            folder: "Research".into(),
            ..ConnectionDraft::default()
        };
        draft.apply_imported_server(&ServerConfig {
            name: "Cluster".into(),
            host_alias: "cluster".into(),
            host: "cluster.example".into(),
            user: "alice".into(),
            ..ServerConfig::default()
        });
        assert_eq!(draft.folder, "Research");
    }

    #[test]
    fn duplicate_targets_compare_hostnames_case_insensitively() {
        let existing = password_server();
        let mut draft = ConnectionDraft::from_server(&existing);
        draft.editing_id = None;
        draft.existing = None;
        draft.host = "HOST.EXAMPLE".into();
        draft.password = "new".into();
        assert_eq!(
            draft.validate(&[existing]),
            Err(DraftError::Duplicate("Alpha".into()))
        );
    }

    #[test]
    fn native_key_requires_an_existing_private_key() {
        let temp = tempdir().unwrap();
        let public = temp.path().join("id_ed25519.pub");
        fs::write(&public, "public").unwrap();
        let mut draft = ConnectionDraft {
            name: "Alpha".into(),
            host: "host.example".into(),
            user: "alice".into(),
            key_file: public.display().to_string(),
            ..ConnectionDraft::default()
        };
        assert_eq!(draft.validate(&[]), Err(DraftError::PublicKey));
        draft.key_file = temp.path().join("missing").display().to_string();
        assert!(matches!(
            draft.validate(&[]),
            Err(DraftError::KeyMissing(_))
        ));
    }

    #[test]
    fn openssh_forces_key_auth_without_native_key_validation() {
        let draft = ConnectionDraft {
            name: "Alpha".into(),
            host: "host.example".into(),
            user: "alice".into(),
            auth: AuthMethod::Password,
            connection_method: ConnectionMethod::Openssh,
            ..ConnectionDraft::default()
        };
        assert_eq!(draft.validate(&[]).unwrap().server.auth, AuthMethod::Key);
    }

    #[test]
    fn interactive_ssh_does_not_require_or_persist_noninteractive_secrets() {
        let draft = ConnectionDraft {
            name: "Interactive".into(),
            host: "host.example".into(),
            user: "alice".into(),
            auth: AuthMethod::Password,
            password: "one-time-token".into(),
            key_passphrase: "temporary-passphrase".into(),
            connection_method: ConnectionMethod::Interactive,
            ..ConnectionDraft::default()
        };
        let validated = draft.validate(&[]).unwrap();

        assert_eq!(validated.server.auth, AuthMethod::Key);
        assert_eq!(validated.password, SecretAction::Clear);
        assert_eq!(validated.key_passphrase, SecretAction::Clear);
    }

    #[test]
    fn new_ids_never_collide() {
        let existing = ServerConfig {
            id: "Alpha".into(),
            ..ServerConfig::default()
        };
        let temp = tempdir().unwrap();
        let key = temp.path().join("id");
        fs::write(&key, "private").unwrap();
        let draft = ConnectionDraft {
            name: "Alpha".into(),
            host: "host.example".into(),
            user: "alice".into(),
            key_file: key.display().to_string(),
            ..ConnectionDraft::default()
        };
        assert_eq!(draft.validate(&[existing]).unwrap().server.id, "Alpha-2");
    }

    #[test]
    fn ssh_import_plan_marks_duplicates_and_protects_mounted_overwrites() {
        let existing = ServerConfig {
            id: "alpha".into(),
            name: "Existing Alpha".into(),
            mode: "ssh_config".into(),
            source: "ssh_config".into(),
            host_alias: "alpha".into(),
            host: "alpha.example".into(),
            user: "alice".into(),
            port: "22".into(),
            ..ServerConfig::default()
        };
        let imported = ServerConfig {
            name: "alpha".into(),
            mode: "ssh_config".into(),
            source: "ssh_config".into(),
            host_alias: "alpha".into(),
            host: "alpha.example".into(),
            user: "alice".into(),
            port: "22".into(),
            ..ServerConfig::default()
        };
        let protected = std::collections::HashSet::from(["alpha".into()]);
        let plan = plan_ssh_imports(
            vec![("alpha".into(), Ok(imported))],
            std::slice::from_ref(&existing),
            &protected,
        );
        assert_eq!(plan.items[0].status, ImportStatus::Same);
        assert!(plan.items[0].overwrite_protected);
        assert!(!plan.items[0].can_overwrite);
        assert_eq!(
            plan.apply(&[ImportAction::Overwrite], &[existing]),
            Err(DraftError::InvalidImportAction("alpha".into()))
        );
    }

    #[test]
    fn ssh_import_overwrite_preserves_profile_specific_settings() {
        let existing = ServerConfig {
            id: "alpha".into(),
            name: "Old".into(),
            tags: vec!["Work".into(), "研究".into()],
            host_alias: "alpha".into(),
            host: "old.example".into(),
            user: "alice".into(),
            port: "22".into(),
            remote_path: "/project".into(),
            mountpoint: "Z:".into(),
            cache_mode: "writes".into(),
            ..ServerConfig::default()
        };
        let imported = ServerConfig {
            name: "Alpha".into(),
            mode: "ssh_config".into(),
            source: "ssh_config".into(),
            host_alias: "alpha".into(),
            host: "new.example".into(),
            user: "alice".into(),
            port: "22".into(),
            ssh_config_path: "/tmp/custom-config".into(),
            ..ServerConfig::default()
        };
        let plan = plan_ssh_imports(
            vec![("alpha".into(), Ok(imported))],
            std::slice::from_ref(&existing),
            &std::collections::HashSet::new(),
        );
        assert_eq!(plan.items[0].status, ImportStatus::SameHost);
        let merged = plan
            .apply(&[ImportAction::Overwrite], &[existing])
            .unwrap()
            .remove(0);
        assert_eq!(merged.host, "new.example");
        assert_eq!(merged.remote_path, "/project");
        assert_eq!(merged.mountpoint, "Z:");
        assert_eq!(merged.cache_mode, "writes");
        assert_eq!(merged.ssh_config_path, "/tmp/custom-config");
        assert_eq!(merged.tags, vec!["Work", "研究"]);
        assert_eq!(merged.folder, "Work");
    }

    #[test]
    fn ssh_import_overwrite_drops_secrets_bound_to_the_old_target() {
        let existing = ServerConfig {
            auth: AuthMethod::Password,
            password_obscured: "old-password".into(),
            key_pass_obscured: "old-passphrase".into(),
            password_credential: "old-password-reference".into(),
            key_pass_credential: "old-passphrase-reference".into(),
            host: "old.example".into(),
            user: "alice".into(),
            ..password_server()
        };
        let imported = ServerConfig {
            auth: AuthMethod::Key,
            host: "new.example".into(),
            user: "alice".into(),
            key_file: "/keys/new".into(),
            ..ServerConfig::default()
        };

        let merged = merge_imported_connection(&existing, &imported);

        assert!(merged.password_obscured.is_empty());
        assert!(merged.key_pass_obscured.is_empty());
        assert!(merged.password_credential.is_empty());
        assert!(merged.key_pass_credential.is_empty());
    }
}
