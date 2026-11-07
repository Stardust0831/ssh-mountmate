//! Configuration persistence and credential transactions. No App or window state is needed.
use crate::i18n::Locale;
use crate::{diagnostic_trace, localize_draft_error, mountpoint_choice, reconcile_login_startup};
use mountmate_core::connection::{SecretAction, ValidatedConnection};
use mountmate_core::credential::{
    CredentialChange, CredentialError, CredentialKind, CredentialMigration, CredentialStore,
    SystemCredentialStore, credential_reference, delete_credential_references,
    prepare_server_to_obscure, prepare_server_to_system, replace_verified, rollback_change,
};
use mountmate_core::mountpoint::preflight_custom_mountpoint;
use mountmate_core::paths::AppPaths;
use mountmate_core::rclone::clear_rclone_remote_secrets;
use mountmate_core::service::MountService;
use mountmate_core::ssh::{prepare_managed_ssh_server, remove_managed_ssh_server};
use mountmate_core::{CredentialStorage, ServerConfig, Settings, storage};
use mountmate_platform::Platform;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct ServerMutation {
    pub servers: Vec<ServerConfig>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SettingsMutation {
    pub settings: Settings,
    pub servers: Vec<ServerConfig>,
    pub warning: Option<String>,
}

pub struct SettingsChange {
    pub settings: Settings,
    pub previous_settings: Settings,
    pub previous_servers: Vec<ServerConfig>,
    pub preference_updates: Vec<storage::ServerPreferenceUpdate>,
}

pub fn save_connection(
    paths: &AppPaths,
    service: &MountService,
    validated: ValidatedConnection,
    previous: Option<ServerConfig>,
    credential_storage: CredentialStorage,
    locale: Locale,
) -> Result<ServerMutation, String> {
    let custom_mountpoint = (validated.server.primary_mount
        && mountpoint_choice(&validated.server.mountpoint) == "custom")
        .then(|| validated.server.mountpoint.clone());
    if let Some(mountpoint) = custom_mountpoint {
        let home = directories::BaseDirs::new()
            .map(|directories| directories.home_dir().to_owned())
            .unwrap_or_else(|| PathBuf::from("."));
        preflight_custom_mountpoint(&mountpoint, &home).map_err(|error| error.to_string())?;
    }
    for mapping in &validated.server.mounts {
        if mountpoint_choice(&mapping.mountpoint) == "custom" {
            let home = directories::BaseDirs::new()
                .map(|d| d.home_dir().to_owned())
                .unwrap_or_default();
            preflight_custom_mountpoint(&mapping.mountpoint, &home).map_err(|e| e.to_string())?;
        }
    }
    let server_id = validated.server.id.clone();
    let password = prepare_secret_action(
        service,
        credential_storage,
        &server_id,
        CredentialKind::Password,
        &validated.password,
    )?;
    let key_passphrase = match prepare_secret_action(
        service,
        credential_storage,
        &server_id,
        CredentialKind::KeyPassphrase,
        &validated.key_passphrase,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = rollback_prepared_secret(&password);
            return Err(error);
        }
    };
    let mut server =
        match validated.apply_secrets(password.obscured.clone(), key_passphrase.obscured.clone()) {
            Ok(server) => server,
            Err(error) => {
                let _ = rollback_prepared_secrets([&password, &key_passphrase]);
                return Err(localize_draft_error(locale, &error));
            }
        };
    password.apply(&mut server);
    key_passphrase.apply(&mut server);
    let managed_snapshot = match capture_managed_profile(previous.as_ref()) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let rollback = rollback_prepared_secrets([&password, &key_passphrase]);
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback) => {
                    format!("{error}; credential rollback failed: {rollback}")
                }
            });
        }
    };
    if let Err(error) = prepare_managed_ssh_server(&mut server, &Platform) {
        let rollback = rollback_prepared_secrets([&password, &key_passphrase]);
        return Err(match rollback {
            Ok(()) => error.to_string(),
            Err(rollback) => {
                format!("{error}; credential rollback failed: {rollback}")
            }
        });
    }
    let servers = match storage::upsert_server(paths, server.clone()) {
        Ok(servers) => servers,
        Err(error) => {
            let managed_rollback =
                rollback_prepared_managed_profile(&server, managed_snapshot.as_ref());
            let credential_rollback = rollback_prepared_secrets([&password, &key_passphrase]);
            let mut message = error.to_string();
            if let Err(rollback) = managed_rollback {
                message.push_str(&format!("; managed SSH rollback failed: {rollback}"));
            }
            if let Err(rollback) = credential_rollback {
                message.push_str(&format!("; credential rollback failed: {rollback}"));
            }
            return Err(message);
        }
    };
    let mut cleanup_errors = Vec::new();
    if let Some(previous) = &previous {
        if previous.ssh_config_managed
            && (!server.ssh_config_managed
                || previous.managed_ssh_config_path != server.managed_ssh_config_path)
            && let Err(error) = remove_managed_ssh_server(previous)
        {
            cleanup_errors.push(match locale {
                Locale::English => {
                    format!("old managed SSH cleanup failed: {error}")
                }
                Locale::Chinese => {
                    format!("旧托管 SSH 配置清理失败：{error}")
                }
            });
        }
        if let Err(error) = delete_retired_connection_credentials(previous, &server) {
            cleanup_errors.push(match locale {
                Locale::English => {
                    format!("retired credential cleanup failed: {error}")
                }
                Locale::Chinese => {
                    format!("旧系统凭据清理失败：{error}")
                }
            });
        }
    }
    let warning = (!cleanup_errors.is_empty()).then(|| match locale {
        Locale::English => {
            format!("Connection saved; {}", cleanup_errors.join("; "))
        }
        Locale::Chinese => {
            format!("连接已保存，但{}", cleanup_errors.join("；"))
        }
    });
    if let Some(warning) = &warning {
        diagnostic_trace(warning);
    }
    Ok(ServerMutation { servers, warning })
}

pub fn save_settings(
    paths: &AppPaths,
    service: &MountService,
    change: SettingsChange,
    startup_lock: &Mutex<()>,
    locale: Locale,
) -> Result<SettingsMutation, String> {
    let SettingsChange {
        settings,
        previous_settings,
        previous_servers,
        preference_updates,
    } = change;
    let storage_changed = settings.credential_storage != previous_settings.credential_storage;
    if storage_changed {
        diagnostic_trace(&credential_presence_summary("before", &previous_servers));
    }
    let credential_migration = if storage_changed {
        Some(migrate_servers_for_storage(
            paths,
            service,
            &previous_servers,
            settings.credential_storage,
        )?)
    } else {
        None
    };
    let migrated_servers =
        match storage::update_server_preferences_batch(paths, &preference_updates) {
            Ok(servers) => servers,
            Err(error) => {
                return Err(rollback_credential_storage_change(
                    paths,
                    &previous_servers,
                    credential_migration.as_ref(),
                    error.to_string(),
                ));
            }
        };
    if storage_changed {
        diagnostic_trace(&credential_presence_summary("after", &migrated_servers));
    }
    if storage_changed && settings.credential_storage == CredentialStorage::System {
        for server in &migrated_servers {
            if let Err(error) = clear_rclone_remote_secrets(paths, server.remote_name()) {
                return Err(rollback_credential_storage_change(
                    paths,
                    &previous_servers,
                    credential_migration.as_ref(),
                    error.to_string(),
                ));
            }
        }
    }
    if let Err(error) = storage::save_settings(paths, &settings) {
        let mut message = rollback_credential_storage_change(
            paths,
            &previous_servers,
            credential_migration.as_ref(),
            error.to_string(),
        );
        if credential_migration.is_none() {
            let rollback_updates = previous_servers
                .iter()
                .map(|server| storage::ServerPreferenceUpdate {
                    id: server.id.clone(),
                    tags: None,
                    auto_mount_at_login: Some(server.auto_mount_at_login),
                })
                .collect::<Vec<_>>();
            if let Err(rollback) =
                storage::update_server_preferences_batch(paths, &rollback_updates)
            {
                message.push_str(&format!("; login-startup rollback failed: {rollback}"));
            }
        }
        return Err(message);
    }
    let mut warnings = Vec::new();
    if let Err(error) = reconcile_login_startup(paths, startup_lock) {
        warnings.push(match locale {
            Locale::English => format!(
                "settings were saved, but login startup integration will be retried next launch: {error}"
            ),
            Locale::Chinese => format!(
                "设置已保存，但登录自启集成失败，将在下次启动时重试：{error}"
            ),
        });
    }
    if storage_changed
        && settings.credential_storage == CredentialStorage::Obscure
        && let Some(error) = credential_migration
            .as_ref()
            .and_then(|migration| migration.retire_system_references().err())
    {
        warnings.push(match locale {
            Locale::English => format!("some retired vault entries could not be removed: {error}"),
            Locale::Chinese => {
                format!("部分旧系统凭据无法删除：{error}")
            }
        });
    }
    let warning = (!warnings.is_empty()).then(|| warnings.join("; "));
    Ok(SettingsMutation {
        settings,
        servers: migrated_servers,
        warning,
    })
}

struct ManagedProfileSnapshot {
    path: PathBuf,
    content: Option<Vec<u8>>,
}

fn capture_managed_profile(
    previous: Option<&ServerConfig>,
) -> Result<Option<ManagedProfileSnapshot>, String> {
    let Some(previous) = previous.filter(|server| server.ssh_config_managed) else {
        return Ok(None);
    };
    let path = PathBuf::from(previous.managed_ssh_config_path.trim());
    if path.as_os_str().is_empty() {
        return Ok(None);
    }
    let content = match fs::read(&path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    Ok(Some(ManagedProfileSnapshot { path, content }))
}

fn rollback_prepared_managed_profile(
    prepared: &ServerConfig,
    previous: Option<&ManagedProfileSnapshot>,
) -> Result<(), String> {
    if !prepared.ssh_config_managed {
        return Ok(());
    }
    let prepared_path = PathBuf::from(prepared.managed_ssh_config_path.trim());
    if let Some(previous) = previous.filter(|snapshot| snapshot.path == prepared_path)
        && let Some(content) = &previous.content
    {
        return storage::atomic_write(&previous.path, content).map_err(|error| error.to_string());
    }
    remove_managed_ssh_server(prepared).map_err(|error| error.to_string())
}

struct PreparedSecret {
    kind: CredentialKind,
    obscured: Option<String>,
    credential: Option<String>,
    change: Option<CredentialChange>,
}

impl PreparedSecret {
    fn apply(&self, server: &mut ServerConfig) {
        let Some(reference) = &self.credential else {
            return;
        };
        match self.kind {
            CredentialKind::Password => {
                server.password_credential.clone_from(reference);
            }
            CredentialKind::KeyPassphrase => {
                server.key_pass_credential.clone_from(reference);
            }
        }
    }
}

fn prepare_secret_action(
    service: &MountService,
    storage: CredentialStorage,
    server_id: &str,
    kind: CredentialKind,
    action: &SecretAction,
) -> Result<PreparedSecret, String> {
    let SecretAction::Obscure(secret) = action else {
        return Ok(PreparedSecret {
            kind,
            obscured: None,
            credential: None,
            change: None,
        });
    };
    let obscured = service
        .obscure_secret(secret)
        .map_err(|error| error.to_string())?;
    if storage == CredentialStorage::Obscure {
        return Ok(PreparedSecret {
            kind,
            obscured: Some(obscured),
            credential: None,
            change: None,
        });
    }
    let reference = credential_reference(server_id, kind);
    let change = replace_verified(&SystemCredentialStore, &reference, secret)
        .map_err(|error| error.to_string())?;
    Ok(PreparedSecret {
        kind,
        obscured: Some(obscured),
        credential: Some(reference),
        change: Some(change),
    })
}

fn rollback_prepared_secret(secret: &PreparedSecret) -> Result<(), String> {
    if let Some(change) = &secret.change {
        rollback_change(&SystemCredentialStore, change).map_err(|error| error.to_string())
    } else {
        Ok(())
    }
}

fn rollback_prepared_secrets<'a>(
    secrets: impl IntoIterator<Item = &'a PreparedSecret>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for secret in secrets {
        if let Err(error) = rollback_prepared_secret(secret) {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

struct CredentialStorageMigration {
    migrations: Vec<CredentialMigration>,
    retired_references: Vec<String>,
}

impl CredentialStorageMigration {
    fn retire_system_references(&self) -> Result<(), String> {
        delete_credential_references(&SystemCredentialStore, &self.retired_references)
            .map_err(|error| error.to_string())
    }
}

fn rollback_credential_storage_change(
    paths: &AppPaths,
    previous_servers: &[ServerConfig],
    migration: Option<&CredentialStorageMigration>,
    message: String,
) -> String {
    let Some(migration) = migration else {
        return message;
    };
    rollback_migrations_after_persistence(paths, previous_servers, &migration.migrations, message)
}

fn credential_presence_summary(stage: &str, servers: &[ServerConfig]) -> String {
    let records = servers
        .iter()
        .map(|server| {
            format!(
                "{}[password_obscured={},password_reference={},key_pass_obscured={},key_pass_reference={}]",
                server.id,
                !server.password_obscured.is_empty(),
                !server.password_credential.is_empty(),
                !server.key_pass_obscured.is_empty(),
                !server.key_pass_credential.is_empty(),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("credential migration {stage}: {records}")
}

fn rollback_migrations_after_persistence(
    paths: &AppPaths,
    previous_servers: &[ServerConfig],
    migrations: &[CredentialMigration],
    mut message: String,
) -> String {
    match storage::save_servers(paths, previous_servers) {
        Ok(()) => {
            for migration in migrations.iter().rev() {
                if let Err(rollback) = migration.rollback(&SystemCredentialStore) {
                    message.push_str(&format!("; credential rollback failed: {rollback}"));
                }
            }
        }
        Err(rollback) => {
            // Keep the verified system representation when the server record
            // could not be restored; deleting it here could remove the last
            // usable copy referenced by the persisted record.
            message.push_str(&format!("; server rollback failed: {rollback}"));
        }
    }
    message
}

fn migrate_servers_for_storage(
    paths: &AppPaths,
    service: &MountService,
    servers: &[ServerConfig],
    target: CredentialStorage,
) -> Result<CredentialStorageMigration, String> {
    let mut migrations = Vec::with_capacity(servers.len());
    for server in servers {
        let result = match target {
            CredentialStorage::System => {
                prepare_server_to_system(server, &SystemCredentialStore, |obscured| {
                    service
                        .reveal_secret(obscured)
                        .map_err(|error| CredentialError::Reveal(error.to_string()))
                })
            }
            CredentialStorage::Obscure => {
                prepare_server_to_obscure(server, &SystemCredentialStore, |secret| {
                    service
                        .obscure_secret(secret)
                        .map_err(|error| CredentialError::Obscure(error.to_string()))
                })
            }
        };
        match result {
            Ok(migration) => migrations.push(migration),
            Err(error) => {
                let mut message = error.to_string();
                for migration in migrations.iter().rev() {
                    if let Err(rollback) = migration.rollback(&SystemCredentialStore) {
                        message.push_str(&format!("; credential rollback failed: {rollback}"));
                    }
                }
                return Err(message);
            }
        }
    }
    let candidates = migrations
        .iter()
        .map(|migration| migration.candidate().clone())
        .collect::<Vec<_>>();
    let persisted_candidates =
        persist_and_reload_servers(paths, &candidates, servers, &migrations)?;
    let mut finalized = Vec::with_capacity(migrations.len());
    let mut retired_references = Vec::new();
    for migration in &migrations {
        let Some(persisted) = persisted_candidates
            .iter()
            .find(|server| server.id == migration.candidate().id)
        else {
            return Err(rollback_migrations_after_persistence(
                paths,
                servers,
                &migrations,
                "credential migration persistence verification failed".into(),
            ));
        };
        let finalized_server = match target {
            CredentialStorage::System => migration
                .finalize_to_system(persisted)
                .map(|server| (server, Vec::new())),
            CredentialStorage::Obscure => migration.finalize_to_obscure(persisted).map(|commit| {
                let retired = commit.retired_references().to_vec();
                (commit.into_server(), retired)
            }),
        };
        match finalized_server {
            Ok((server, retired)) => {
                finalized.push(server);
                retired_references.extend(retired);
            }
            Err(error) => {
                return Err(rollback_migrations_after_persistence(
                    paths,
                    servers,
                    &migrations,
                    error.to_string(),
                ));
            }
        }
    }
    persist_and_reload_servers(paths, &finalized, servers, &migrations)?;
    Ok(CredentialStorageMigration {
        migrations,
        retired_references,
    })
}

fn persist_and_reload_servers(
    paths: &AppPaths,
    candidate: &[ServerConfig],
    original: &[ServerConfig],
    migrations: &[CredentialMigration],
) -> Result<Vec<ServerConfig>, String> {
    let result = storage::save_servers(paths, candidate)
        .map_err(|error| error.to_string())
        .and_then(|_| storage::load_servers(paths).map_err(|error| error.to_string()))
        .and_then(|persisted| {
            (persisted == candidate)
                .then_some(persisted)
                .ok_or_else(|| "credential migration persistence verification failed".to_owned())
        });
    match result {
        Ok(persisted) => Ok(persisted),
        Err(error) => Err(rollback_migrations_after_persistence(
            paths, original, migrations, error,
        )),
    }
}

pub fn delete_retired_connection_credentials(
    previous: &ServerConfig,
    current: &ServerConfig,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for (old_reference, new_reference) in [
        (&previous.password_credential, &current.password_credential),
        (&previous.key_pass_credential, &current.key_pass_credential),
    ] {
        if !old_reference.is_empty()
            && old_reference != new_reference
            && let Err(error) = SystemCredentialStore.delete(old_reference)
        {
            errors.push(error.to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_migration_diagnostics_include_only_presence_state() {
        let server = ServerConfig {
            id: "alpha".into(),
            password_obscured: "password-secret-sentinel".into(),
            key_pass_credential: "credential-secret-sentinel".into(),
            ..ServerConfig::default()
        };
        let summary = credential_presence_summary("before", &[server]);
        assert!(summary.contains("password_obscured=true"));
        assert!(summary.contains("password_reference=false"));
        assert!(summary.contains("key_pass_reference=true"));
        assert!(!summary.contains("password-secret-sentinel"));
        assert!(!summary.contains("credential-secret-sentinel"));
    }
    #[test]
    fn managed_profile_rollback_restores_the_previous_contents() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("alpha.conf");
        fs::write(&path, b"old profile").unwrap();
        let prepared = ServerConfig {
            ssh_config_managed: true,
            managed_ssh_config_path: path.display().to_string(),
            ..ServerConfig::default()
        };
        let snapshot = ManagedProfileSnapshot {
            path: path.clone(),
            content: Some(b"old profile".to_vec()),
        };
        fs::write(&path, b"new profile").unwrap();

        rollback_prepared_managed_profile(&prepared, Some(&snapshot)).unwrap();

        assert_eq!(fs::read(path).unwrap(), b"old profile");
    }
    #[test]
    fn system_secret_replacement_retains_the_obscured_compatibility_copy() {
        let mut server = ServerConfig {
            password_obscured: "obscured-password".into(),
            key_pass_obscured: "obscured-passphrase".into(),
            ..ServerConfig::default()
        };
        PreparedSecret {
            kind: CredentialKind::Password,
            obscured: Some("obscured-password".into()),
            credential: Some("password-reference".into()),
            change: None,
        }
        .apply(&mut server);
        PreparedSecret {
            kind: CredentialKind::KeyPassphrase,
            obscured: Some("obscured-passphrase".into()),
            credential: Some("passphrase-reference".into()),
            change: None,
        }
        .apply(&mut server);

        assert_eq!(server.password_obscured, "obscured-password");
        assert_eq!(server.key_pass_obscured, "obscured-passphrase");
        assert_eq!(server.password_credential, "password-reference");
        assert_eq!(server.key_pass_credential, "passphrase-reference");
    }
}
