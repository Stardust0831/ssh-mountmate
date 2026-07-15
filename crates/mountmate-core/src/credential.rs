use std::fmt;

use thiserror::Error;

use crate::{APP_ID, ServerConfig};

const CREDENTIAL_SERVICE: &str = "io.github.stardust0831.ssh-mountmate";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    Password,
    KeyPassphrase,
}

impl CredentialKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::KeyPassphrase => "key-passphrase",
        }
    }
}

#[derive(Error)]
pub enum CredentialError {
    #[error("system credential store is unavailable")]
    Unavailable(String),
    #[error("system credential verification failed for {0}")]
    Verification(String),
    #[error("system credential is missing: {0}")]
    Missing(String),
    #[error("could not convert an existing rclone-obscured secret")]
    Reveal(String),
    #[error("could not prepare a secret for rclone")]
    Obscure(String),
    #[error("credential migration persistence verification failed for {0}")]
    Persistence(String),
    #[error("credential rollback failed for {0}")]
    Rollback(String),
}

impl fmt::Debug for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(_) => formatter.write_str("CredentialError::Unavailable(<redacted>)"),
            Self::Verification(reference) => formatter
                .debug_tuple("CredentialError::Verification")
                .field(reference)
                .finish(),
            Self::Missing(reference) => formatter
                .debug_tuple("CredentialError::Missing")
                .field(reference)
                .finish(),
            Self::Reveal(_) => formatter.write_str("CredentialError::Reveal(<redacted>)"),
            Self::Obscure(_) => formatter.write_str("CredentialError::Obscure(<redacted>)"),
            Self::Persistence(field) => formatter
                .debug_tuple("CredentialError::Persistence")
                .field(field)
                .finish(),
            Self::Rollback(reference) => formatter
                .debug_tuple("CredentialError::Rollback")
                .field(reference)
                .finish(),
        }
    }
}

pub trait CredentialStore: Send + Sync {
    fn set(&self, reference: &str, secret: &str) -> Result<(), CredentialError>;
    fn get(&self, reference: &str) -> Result<String, CredentialError>;
    fn delete(&self, reference: &str) -> Result<(), CredentialError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemCredentialStore;

impl SystemCredentialStore {
    fn entry(reference: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(CREDENTIAL_SERVICE, reference)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }
}

impl CredentialStore for SystemCredentialStore {
    fn set(&self, reference: &str, secret: &str) -> Result<(), CredentialError> {
        Self::entry(reference)?
            .set_password(secret)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }

    fn get(&self, reference: &str) -> Result<String, CredentialError> {
        Self::entry(reference)?
            .get_password()
            .map_err(|error| match error {
                keyring::Error::NoEntry => CredentialError::Missing(reference.into()),
                other => CredentialError::Unavailable(other.to_string()),
            })
    }

    fn delete(&self, reference: &str) -> Result<(), CredentialError> {
        match Self::entry(reference)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(CredentialError::Unavailable(error.to_string())),
        }
    }
}

pub fn credential_reference(server_id: &str, kind: CredentialKind) -> String {
    format!("{APP_ID}:{server_id}:{}", kind.suffix())
}

pub fn store_verified(
    store: &dyn CredentialStore,
    reference: &str,
    secret: &str,
) -> Result<(), CredentialError> {
    replace_verified(store, reference, secret).map(|_| ())
}

#[derive(Clone)]
pub struct CredentialChange {
    pub reference: String,
    previous: Option<String>,
}

impl fmt::Debug for CredentialChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialChange")
            .field("reference", &self.reference)
            .field("had_previous", &self.previous.is_some())
            .finish()
    }
}

impl CredentialChange {
    pub fn reference(&self) -> &str {
        &self.reference
    }

    pub fn had_previous(&self) -> bool {
        self.previous.is_some()
    }
}

fn restore_previous(
    store: &dyn CredentialStore,
    reference: &str,
    previous: Option<&str>,
) -> Result<(), CredentialError> {
    match previous {
        Some(previous) => {
            store
                .set(reference, previous)
                .map_err(|_| CredentialError::Rollback(reference.into()))?;
            match store.get(reference) {
                Ok(restored) if restored == previous => Ok(()),
                _ => Err(CredentialError::Rollback(reference.into())),
            }
        }
        None => store
            .delete(reference)
            .map_err(|_| CredentialError::Rollback(reference.into())),
    }
}

pub fn replace_verified(
    store: &dyn CredentialStore,
    reference: &str,
    secret: &str,
) -> Result<CredentialChange, CredentialError> {
    let previous = match store.get(reference) {
        Ok(value) => Some(value),
        Err(CredentialError::Missing(_)) => None,
        Err(error) => return Err(error),
    };
    if let Err(error) = store.set(reference, secret) {
        return match restore_previous(store, reference, previous.as_deref()) {
            Ok(()) => Err(error),
            Err(rollback) => Err(rollback),
        };
    }
    let verified = match store.get(reference) {
        Ok(stored) if stored == secret => Ok(()),
        Ok(_) | Err(_) => Err(CredentialError::Verification(reference.into())),
    };
    if let Err(error) = verified {
        return match restore_previous(store, reference, previous.as_deref()) {
            Ok(()) => Err(error),
            Err(rollback) => Err(rollback),
        };
    }
    Ok(CredentialChange {
        reference: reference.into(),
        previous,
    })
}

pub fn rollback_change(
    store: &dyn CredentialStore,
    change: &CredentialChange,
) -> Result<(), CredentialError> {
    restore_previous(store, &change.reference, change.previous.as_deref())
}

fn rollback_changes(
    store: &dyn CredentialStore,
    changes: &[CredentialChange],
) -> Result<(), CredentialError> {
    let mut rollback_error = None;
    for change in changes.iter().rev() {
        if let Err(error) = rollback_change(store, change) {
            rollback_error = Some(error);
        }
    }
    rollback_error.map_or(Ok(()), Err)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialMigrationKind {
    ToSystem,
    ToObscure,
}

/// A credential migration after its system-store writes have been verified but
/// before the old representation is retired from the persisted server record.
///
/// The candidate intentionally contains both representations. Callers can
/// persist and reload it, then call the matching `finalize_*` method. If any
/// persistence boundary fails, `rollback` restores every store entry touched
/// by this migration, including entries that were overwritten.
pub struct CredentialMigration {
    candidate: ServerConfig,
    changes: Vec<CredentialChange>,
    retired_references: Vec<String>,
    kind: CredentialMigrationKind,
}

impl fmt::Debug for CredentialMigration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialMigration")
            .field("server_id", &self.candidate.id)
            .field("kind", &self.kind)
            .field("changes", &self.changes)
            .field("retired_reference_count", &self.retired_references.len())
            .finish()
    }
}

impl fmt::Debug for CredentialMigrationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ToSystem => "ToSystem",
            Self::ToObscure => "ToObscure",
        })
    }
}

impl CredentialMigration {
    pub fn candidate(&self) -> &ServerConfig {
        &self.candidate
    }

    pub fn into_candidate(self) -> ServerConfig {
        self.candidate
    }

    pub fn changes(&self) -> &[CredentialChange] {
        &self.changes
    }

    pub fn retired_references(&self) -> &[String] {
        &self.retired_references
    }

    pub fn rollback(&self, store: &dyn CredentialStore) -> Result<(), CredentialError> {
        rollback_changes(store, &self.changes)
    }

    pub fn finalize_to_system(
        &self,
        persisted: &ServerConfig,
    ) -> Result<ServerConfig, CredentialError> {
        self.ensure_persisted_candidate(persisted, CredentialMigrationKind::ToSystem)?;
        let mut finalized = persisted.clone();
        if !self.candidate.password_credential.is_empty() {
            finalized.password_obscured.clear();
        }
        if !self.candidate.key_pass_credential.is_empty() {
            finalized.key_pass_obscured.clear();
        }
        Ok(finalized)
    }

    pub fn finalize_to_obscure(
        &self,
        persisted: &ServerConfig,
    ) -> Result<CredentialCommit, CredentialError> {
        self.ensure_persisted_candidate(persisted, CredentialMigrationKind::ToObscure)?;
        let mut finalized = persisted.clone();
        let mut retired_references = Vec::new();
        if !self.candidate.password_credential.is_empty() {
            retired_references.push(self.candidate.password_credential.clone());
            finalized.password_credential.clear();
        }
        if !self.candidate.key_pass_credential.is_empty() {
            retired_references.push(self.candidate.key_pass_credential.clone());
            finalized.key_pass_credential.clear();
        }
        Ok(CredentialCommit {
            server: finalized,
            retired_references,
        })
    }

    fn ensure_persisted_candidate(
        &self,
        persisted: &ServerConfig,
        expected_kind: CredentialMigrationKind,
    ) -> Result<(), CredentialError> {
        if self.kind != expected_kind || persisted.id != self.candidate.id {
            return Err(CredentialError::Persistence("server".into()));
        }
        for (field, staged_reference, persisted_reference, staged_obscured, persisted_obscured) in [
            (
                "password",
                &self.candidate.password_credential,
                &persisted.password_credential,
                &self.candidate.password_obscured,
                &persisted.password_obscured,
            ),
            (
                "key passphrase",
                &self.candidate.key_pass_credential,
                &persisted.key_pass_credential,
                &self.candidate.key_pass_obscured,
                &persisted.key_pass_obscured,
            ),
        ] {
            if staged_reference != persisted_reference {
                return Err(CredentialError::Persistence(field.into()));
            }
            if !staged_obscured.is_empty() && staged_obscured != persisted_obscured {
                return Err(CredentialError::Persistence(field.into()));
            }
        }
        Ok(())
    }
}

/// The final persisted server record and references that may be retired only
/// after this record has itself been persisted and reloaded successfully.
pub struct CredentialCommit {
    server: ServerConfig,
    retired_references: Vec<String>,
}

impl fmt::Debug for CredentialCommit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialCommit")
            .field("server_id", &self.server.id)
            .field("retired_reference_count", &self.retired_references.len())
            .finish()
    }
}

impl CredentialCommit {
    pub fn server(&self) -> &ServerConfig {
        &self.server
    }

    pub fn into_server(self) -> ServerConfig {
        self.server
    }

    pub fn retired_references(&self) -> &[String] {
        &self.retired_references
    }
}

pub fn prepare_server_to_system(
    server: &ServerConfig,
    store: &dyn CredentialStore,
    reveal: impl Fn(&str) -> Result<String, CredentialError>,
) -> Result<CredentialMigration, CredentialError> {
    let mut candidate = server.clone();
    let mut changes = Vec::new();
    let result = (|| -> Result<(), CredentialError> {
        migrate_obscured_field(
            &candidate.password_obscured,
            &mut candidate.password_credential,
            &server.id,
            CredentialKind::Password,
            store,
            &reveal,
            &mut changes,
        )?;
        migrate_obscured_field(
            &candidate.key_pass_obscured,
            &mut candidate.key_pass_credential,
            &server.id,
            CredentialKind::KeyPassphrase,
            store,
            &reveal,
            &mut changes,
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(CredentialMigration {
            candidate,
            changes,
            retired_references: Vec::new(),
            kind: CredentialMigrationKind::ToSystem,
        }),
        Err(error) => match rollback_changes(store, &changes) {
            Ok(()) => Err(error),
            Err(rollback) => Err(rollback),
        },
    }
}

pub fn migrate_server_to_system_transaction(
    server: &ServerConfig,
    store: &dyn CredentialStore,
    reveal: impl Fn(&str) -> Result<String, CredentialError>,
) -> Result<CredentialMigration, CredentialError> {
    prepare_server_to_system(server, store, reveal)
}

pub fn migrate_server_to_system(
    server: &ServerConfig,
    store: &dyn CredentialStore,
    reveal: impl Fn(&str) -> Result<String, CredentialError>,
) -> Result<ServerConfig, CredentialError> {
    let migration = prepare_server_to_system(server, store, reveal)?;
    let candidate = migration.candidate.clone();
    migration.finalize_to_system(&candidate)
}

fn migrate_obscured_field(
    obscured: &str,
    reference: &mut String,
    server_id: &str,
    kind: CredentialKind,
    store: &dyn CredentialStore,
    reveal: &impl Fn(&str) -> Result<String, CredentialError>,
    changes: &mut Vec<CredentialChange>,
) -> Result<(), CredentialError> {
    if !reference.is_empty() {
        store.get(reference)?;
        return Ok(());
    }
    if obscured.is_empty() {
        return Ok(());
    }
    let plaintext = reveal(obscured)?;
    let new_reference = credential_reference(server_id, kind);
    let change = replace_verified(store, &new_reference, &plaintext)?;
    changes.push(change);
    *reference = new_reference;
    Ok(())
}

pub fn hydrate_server_from_system(
    server: &ServerConfig,
    store: &dyn CredentialStore,
    obscure: impl Fn(&str) -> Result<String, CredentialError>,
) -> Result<ServerConfig, CredentialError> {
    let mut hydrated = server.clone();
    hydrate_field(
        &server.password_credential,
        &mut hydrated.password_obscured,
        store,
        &obscure,
    )?;
    hydrate_field(
        &server.key_pass_credential,
        &mut hydrated.key_pass_obscured,
        store,
        &obscure,
    )?;
    Ok(hydrated)
}

pub fn prepare_server_to_obscure(
    server: &ServerConfig,
    store: &dyn CredentialStore,
    obscure: impl Fn(&str) -> Result<String, CredentialError>,
) -> Result<CredentialMigration, CredentialError> {
    let candidate = hydrate_server_from_system(server, store, obscure)?;
    let retired_references = [
        &candidate.password_credential,
        &candidate.key_pass_credential,
    ]
    .into_iter()
    .filter(|reference| !reference.is_empty())
    .cloned()
    .collect();
    Ok(CredentialMigration {
        candidate,
        changes: Vec::new(),
        retired_references,
        kind: CredentialMigrationKind::ToObscure,
    })
}

pub fn migrate_server_to_obscure_transaction(
    server: &ServerConfig,
    store: &dyn CredentialStore,
    obscure: impl Fn(&str) -> Result<String, CredentialError>,
) -> Result<CredentialMigration, CredentialError> {
    prepare_server_to_obscure(server, store, obscure)
}

fn hydrate_field(
    reference: &str,
    obscured: &mut String,
    store: &dyn CredentialStore,
    obscure: &impl Fn(&str) -> Result<String, CredentialError>,
) -> Result<(), CredentialError> {
    if reference.is_empty() {
        return Ok(());
    }
    let plaintext = store.get(reference)?;
    *obscured = obscure(&plaintext)?;
    Ok(())
}

pub fn migrate_server_to_obscure(
    server: &ServerConfig,
    store: &dyn CredentialStore,
    obscure: impl Fn(&str) -> Result<String, CredentialError>,
) -> Result<ServerConfig, CredentialError> {
    let migration = prepare_server_to_obscure(server, store, obscure)?;
    let candidate = migration.candidate.clone();
    Ok(migration.finalize_to_obscure(&candidate)?.into_server())
}

pub fn delete_credential_references(
    store: &dyn CredentialStore,
    references: &[String],
) -> Result<(), CredentialError> {
    let mut existing = Vec::new();
    for reference in references.iter().filter(|reference| !reference.is_empty()) {
        match store.get(reference) {
            Ok(secret) => existing.push((reference.as_str(), secret)),
            Err(CredentialError::Missing(_)) => {}
            Err(error) => return Err(error),
        }
    }
    let mut deleted = Vec::new();
    for (reference, _) in &existing {
        if let Err(error) = store.delete(reference) {
            let mut rollback_error = None;
            for (restored_reference, secret) in &existing[..=deleted.len()] {
                if let Err(rollback) = store_verified(store, restored_reference, secret) {
                    rollback_error = Some(rollback);
                }
            }
            return match rollback_error {
                Some(rollback) => Err(rollback),
                None => Err(error),
            };
        }
        deleted.push(*reference);
    }
    Ok(())
}

pub fn delete_server_credentials(
    server: &ServerConfig,
    store: &dyn CredentialStore,
) -> Result<(), CredentialError> {
    let references = [
        server.password_credential.clone(),
        server.key_pass_credential.clone(),
    ];
    delete_credential_references(store, &references)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryStore {
        values: Mutex<HashMap<String, String>>,
        fail_reference: Mutex<Option<String>>,
        fail_get_reference: Mutex<Option<String>>,
        fail_delete_reference: Mutex<Option<String>>,
        fail_after_set_reference: Mutex<Option<String>>,
        mismatch_once_reference: Mutex<Option<String>>,
        mismatch_ready_reference: Mutex<Option<String>>,
    }

    impl CredentialStore for MemoryStore {
        fn set(&self, reference: &str, secret: &str) -> Result<(), CredentialError> {
            if self.fail_reference.lock().unwrap().as_deref() == Some(reference) {
                return Err(CredentialError::Unavailable("injected failure".into()));
            }
            self.values
                .lock()
                .unwrap()
                .insert(reference.into(), secret.into());
            let mut mismatch = self.mismatch_once_reference.lock().unwrap();
            if mismatch.as_deref() == Some(reference) {
                mismatch.take();
                drop(mismatch);
                *self.mismatch_ready_reference.lock().unwrap() = Some(reference.into());
            }
            if self.fail_after_set_reference.lock().unwrap().as_deref() == Some(reference) {
                return Err(CredentialError::Unavailable("injected failure".into()));
            }
            Ok(())
        }

        fn get(&self, reference: &str) -> Result<String, CredentialError> {
            if self.fail_get_reference.lock().unwrap().as_deref() == Some(reference) {
                return Err(CredentialError::Unavailable("injected failure".into()));
            }
            if self
                .mismatch_ready_reference
                .lock()
                .unwrap()
                .take()
                .as_deref()
                == Some(reference)
            {
                return Ok("verification-mismatch".into());
            }
            self.values
                .lock()
                .unwrap()
                .get(reference)
                .cloned()
                .ok_or_else(|| CredentialError::Missing(reference.into()))
        }

        fn delete(&self, reference: &str) -> Result<(), CredentialError> {
            if self.fail_delete_reference.lock().unwrap().as_deref() == Some(reference) {
                return Err(CredentialError::Unavailable("injected failure".into()));
            }
            self.values.lock().unwrap().remove(reference);
            Ok(())
        }
    }

    fn server() -> ServerConfig {
        ServerConfig {
            id: "alpha".into(),
            password_obscured: "obscured-password".into(),
            key_pass_obscured: "obscured-passphrase".into(),
            ..ServerConfig::default()
        }
    }

    fn has_value(store: &MemoryStore, reference: &str, expected: &str) -> bool {
        matches!(store.get(reference), Ok(value) if value == expected)
    }

    #[test]
    fn migration_verifies_every_secret_before_clearing_obscured_values() {
        let store = MemoryStore::default();
        let migrated = migrate_server_to_system(&server(), &store, |value| {
            Ok(value.replace("obscured-", "plain-"))
        })
        .unwrap();
        assert!(migrated.password_obscured.is_empty());
        assert!(migrated.key_pass_obscured.is_empty());
        assert!(has_value(
            &store,
            &migrated.password_credential,
            "plain-password"
        ));
        assert!(has_value(
            &store,
            &migrated.key_pass_credential,
            "plain-passphrase"
        ));
    }

    #[test]
    fn partial_migration_failure_removes_new_entries_and_preserves_input() {
        let store = MemoryStore::default();
        *store.fail_reference.lock().unwrap() =
            Some(credential_reference("alpha", CredentialKind::KeyPassphrase));
        let original = server();
        assert!(migrate_server_to_system(&original, &store, |value| Ok(value.into())).is_err());
        assert_eq!(original.password_obscured, "obscured-password");
        assert!(store.values.lock().unwrap().is_empty());
    }

    #[test]
    fn partial_migration_failure_restores_an_overwritten_prior_entry() {
        let store = MemoryStore::default();
        let password_reference = credential_reference("alpha", CredentialKind::Password);
        store
            .values
            .lock()
            .unwrap()
            .insert(password_reference.clone(), "prior-password".into());
        *store.fail_reference.lock().unwrap() =
            Some(credential_reference("alpha", CredentialKind::KeyPassphrase));
        let original = server();
        assert!(
            migrate_server_to_system(&original, &store, |value| {
                Ok(value.replace("obscured-", "new-"))
            })
            .is_err()
        );
        assert!(has_value(&store, &password_reference, "prior-password"));
    }

    #[test]
    fn verification_mismatch_restores_prior_and_missing_entries_are_removed() {
        let store = MemoryStore::default();
        let reference = "existing";
        store
            .values
            .lock()
            .unwrap()
            .insert(reference.into(), "prior".into());
        *store.mismatch_once_reference.lock().unwrap() = Some(reference.into());
        assert!(matches!(
            replace_verified(&store, reference, "replacement"),
            Err(CredentialError::Verification(_))
        ));
        assert!(has_value(&store, reference, "prior"));

        let missing = "missing";
        *store.mismatch_once_reference.lock().unwrap() = Some(missing.into());
        assert!(matches!(
            store_verified(&store, missing, "replacement"),
            Err(CredentialError::Verification(_))
        ));
        assert!(matches!(
            store.get(missing),
            Err(CredentialError::Missing(_))
        ));
    }

    #[test]
    fn set_and_get_failures_preserve_prior_entry() {
        let store = MemoryStore::default();
        let reference = "set-failure";
        store
            .values
            .lock()
            .unwrap()
            .insert(reference.into(), "prior".into());
        *store.fail_after_set_reference.lock().unwrap() = Some(reference.into());
        assert!(replace_verified(&store, reference, "replacement").is_err());
        assert!(has_value(&store, reference, "prior"));

        let get_reference = "get-failure";
        *store.fail_get_reference.lock().unwrap() = Some(get_reference.into());
        assert!(replace_verified(&store, get_reference, "replacement").is_err());
        assert!(matches!(
            store.get(get_reference),
            Err(CredentialError::Unavailable(_))
        ));
    }

    #[test]
    fn rollback_failure_is_reported_without_secret_text() {
        let store = MemoryStore::default();
        let reference = "rollback-failure";
        store
            .values
            .lock()
            .unwrap()
            .insert(reference.into(), "prior-secret".into());
        *store.mismatch_once_reference.lock().unwrap() = Some(reference.into());
        *store.fail_after_set_reference.lock().unwrap() = Some(reference.into());
        let error = replace_verified(&store, reference, "replacement-secret").unwrap_err();
        let debug = format!("{error:?}");
        let display = error.to_string();
        assert!(!debug.contains("prior-secret"));
        assert!(!debug.contains("replacement-secret"));
        assert!(!display.contains("prior-secret"));
        assert!(!display.contains("replacement-secret"));
    }

    #[test]
    fn staged_migration_defers_retiring_both_representations() {
        let store = MemoryStore::default();
        let original = server();
        let migration = prepare_server_to_system(&original, &store, |value| {
            Ok(value.replace("obscured-", "plain-"))
        })
        .unwrap();
        assert!(!migration.candidate().password_obscured.is_empty());
        assert!(!migration.candidate().key_pass_obscured.is_empty());
        let finalized = migration.finalize_to_system(migration.candidate()).unwrap();
        assert!(finalized.password_obscured.is_empty());
        assert!(finalized.key_pass_obscured.is_empty());
    }

    #[test]
    fn finalization_rejects_a_persisted_record_that_lost_the_old_representation() {
        let store = MemoryStore::default();
        let original = server();
        let migration = prepare_server_to_system(&original, &store, |value| {
            Ok(value.replace("obscured-", "plain-"))
        })
        .unwrap();
        let mut persisted = migration.candidate().clone();
        persisted.password_obscured.clear();
        assert!(matches!(
            migration.finalize_to_system(&persisted),
            Err(CredentialError::Persistence(_))
        ));
    }

    #[test]
    fn staged_reverse_migration_defers_deleting_references() {
        let store = MemoryStore::default();
        let mut original = ServerConfig {
            id: "alpha".into(),
            password_credential: credential_reference("alpha", CredentialKind::Password),
            key_pass_credential: credential_reference("alpha", CredentialKind::KeyPassphrase),
            ..ServerConfig::default()
        };
        store_verified(&store, &original.password_credential, "plain-password").unwrap();
        store_verified(&store, &original.key_pass_credential, "plain-passphrase").unwrap();
        let migration =
            prepare_server_to_obscure(&original, &store, |value| Ok(format!("obscured-{value}")))
                .unwrap();
        assert!(!migration.candidate().password_credential.is_empty());
        assert!(!migration.candidate().key_pass_credential.is_empty());
        let commit = migration
            .finalize_to_obscure(migration.candidate())
            .unwrap();
        original = commit.into_server();
        assert!(original.password_credential.is_empty());
        assert!(original.key_pass_credential.is_empty());
        assert!(
            store
                .get(&credential_reference("alpha", CredentialKind::Password))
                .is_ok()
        );
        assert!(
            store
                .get(&credential_reference(
                    "alpha",
                    CredentialKind::KeyPassphrase
                ))
                .is_ok()
        );
    }

    #[test]
    fn deleting_multiple_references_restores_prior_entries_on_failure() {
        let store = MemoryStore::default();
        let first = "first".to_owned();
        let second = "second".to_owned();
        store.values.lock().unwrap().extend([
            (first.clone(), "first-secret".into()),
            (second.clone(), "second-secret".into()),
        ]);
        *store.fail_delete_reference.lock().unwrap() = Some(second.clone());
        assert!(delete_credential_references(&store, &[first.clone(), second.clone()]).is_err());
        assert!(store.get(&first).is_ok());
        assert!(store.get(&second).is_ok());
    }

    #[test]
    fn hydration_prefers_system_references_without_mutating_persisted_server() {
        let store = MemoryStore::default();
        let mut persisted = ServerConfig {
            id: "alpha".into(),
            password_credential: credential_reference("alpha", CredentialKind::Password),
            ..ServerConfig::default()
        };
        store_verified(&store, &persisted.password_credential, "plain-password").unwrap();
        let hydrated =
            hydrate_server_from_system(&persisted, &store, |value| Ok(format!("obscured-{value}")))
                .unwrap();
        assert_eq!(hydrated.password_obscured, "obscured-plain-password");
        assert!(persisted.password_obscured.is_empty());
        persisted.password_obscured = "fallback".into();
        let hydrated =
            hydrate_server_from_system(&persisted, &store, |value| Ok(format!("obscured-{value}")))
                .unwrap();
        assert_eq!(hydrated.password_obscured, "obscured-plain-password");
    }

    #[test]
    #[ignore = "requires an unlocked native OS credential store"]
    fn live_system_credential_roundtrip() {
        let reference = format!("{APP_ID}:ci:{}", uuid::Uuid::new_v4().simple());
        let store = SystemCredentialStore;
        store_verified(&store, &reference, "test-only-system-secret").unwrap();
        assert_eq!(store.get(&reference).unwrap(), "test-only-system-secret");
        store.delete(&reference).unwrap();
        assert!(matches!(
            store.get(&reference),
            Err(CredentialError::Missing(_))
        ));
    }
}
