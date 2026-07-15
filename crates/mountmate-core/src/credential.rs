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

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("system credential store is unavailable: {0}")]
    Unavailable(String),
    #[error("system credential verification failed for {0}")]
    Verification(String),
    #[error("system credential is missing: {0}")]
    Missing(String),
    #[error("could not convert an existing rclone-obscured secret: {0}")]
    Reveal(String),
    #[error("could not prepare a secret for rclone: {0}")]
    Obscure(String),
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
    store.set(reference, secret)?;
    match store.get(reference) {
        Ok(stored) if stored == secret => Ok(()),
        _ => {
            let _ = store.delete(reference);
            Err(CredentialError::Verification(reference.into()))
        }
    }
}

#[derive(Debug, Clone)]
pub struct CredentialChange {
    pub reference: String,
    previous: Option<String>,
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
    store_verified(store, reference, secret)?;
    Ok(CredentialChange {
        reference: reference.into(),
        previous,
    })
}

pub fn rollback_change(
    store: &dyn CredentialStore,
    change: &CredentialChange,
) -> Result<(), CredentialError> {
    match &change.previous {
        Some(previous) => store_verified(store, &change.reference, previous),
        None => store.delete(&change.reference),
    }
}

pub fn migrate_server_to_system(
    server: &ServerConfig,
    store: &dyn CredentialStore,
    reveal: impl Fn(&str) -> Result<String, CredentialError>,
) -> Result<ServerConfig, CredentialError> {
    let mut migrated = server.clone();
    let mut created = Vec::new();
    let result = (|| {
        migrate_obscured_field(
            &mut migrated.password_obscured,
            &mut migrated.password_credential,
            &server.id,
            CredentialKind::Password,
            store,
            &reveal,
            &mut created,
        )?;
        migrate_obscured_field(
            &mut migrated.key_pass_obscured,
            &mut migrated.key_pass_credential,
            &server.id,
            CredentialKind::KeyPassphrase,
            store,
            &reveal,
            &mut created,
        )?;
        Ok(migrated)
    })();
    if result.is_err() {
        for reference in created {
            let _ = store.delete(&reference);
        }
    }
    result
}

fn migrate_obscured_field(
    obscured: &mut String,
    reference: &mut String,
    server_id: &str,
    kind: CredentialKind,
    store: &dyn CredentialStore,
    reveal: &impl Fn(&str) -> Result<String, CredentialError>,
    created: &mut Vec<String>,
) -> Result<(), CredentialError> {
    if !reference.is_empty() {
        store.get(reference)?;
        obscured.clear();
        return Ok(());
    }
    if obscured.is_empty() {
        return Ok(());
    }
    let plaintext = reveal(obscured)?;
    let new_reference = credential_reference(server_id, kind);
    store_verified(store, &new_reference, &plaintext)?;
    created.push(new_reference.clone());
    obscured.clear();
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
    let mut migrated = hydrate_server_from_system(server, store, obscure)?;
    migrated.password_credential.clear();
    migrated.key_pass_credential.clear();
    Ok(migrated)
}

pub fn delete_server_credentials(
    server: &ServerConfig,
    store: &dyn CredentialStore,
) -> Result<(), CredentialError> {
    for reference in [&server.password_credential, &server.key_pass_credential] {
        if !reference.is_empty() {
            store.delete(reference)?;
        }
    }
    Ok(())
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
            Ok(())
        }

        fn get(&self, reference: &str) -> Result<String, CredentialError> {
            self.values
                .lock()
                .unwrap()
                .get(reference)
                .cloned()
                .ok_or_else(|| CredentialError::Missing(reference.into()))
        }

        fn delete(&self, reference: &str) -> Result<(), CredentialError> {
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

    #[test]
    fn migration_verifies_every_secret_before_clearing_obscured_values() {
        let store = MemoryStore::default();
        let migrated = migrate_server_to_system(&server(), &store, |value| {
            Ok(value.replace("obscured-", "plain-"))
        })
        .unwrap();
        assert!(migrated.password_obscured.is_empty());
        assert!(migrated.key_pass_obscured.is_empty());
        assert_eq!(
            store.get(&migrated.password_credential).unwrap(),
            "plain-password"
        );
        assert_eq!(
            store.get(&migrated.key_pass_credential).unwrap(),
            "plain-passphrase"
        );
    }

    #[test]
    fn partial_migration_failure_removes_new_vault_entries_and_preserves_input() {
        let store = MemoryStore::default();
        *store.fail_reference.lock().unwrap() =
            Some(credential_reference("alpha", CredentialKind::KeyPassphrase));
        let original = server();
        assert!(migrate_server_to_system(&original, &store, |value| Ok(value.into())).is_err());
        assert_eq!(original.password_obscured, "obscured-password");
        assert!(store.values.lock().unwrap().is_empty());
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
