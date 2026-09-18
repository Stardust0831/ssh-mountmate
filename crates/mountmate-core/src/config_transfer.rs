use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ServerConfig;
use crate::connection::{SshImportPlan, plan_server_imports};

pub const CONFIG_EXPORT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionExport {
    pub schema: u32,
    pub application: String,
    pub connections: Vec<ServerConfig>,
}

impl ConnectionExport {
    pub fn from_servers(servers: &[ServerConfig]) -> Self {
        let connections = servers
            .iter()
            .cloned()
            .map(|mut server| {
                // Passwords and key passphrases are deliberately never exported.
                server.password_obscured.clear();
                server.key_pass_obscured.clear();
                server.password_credential.clear();
                server.key_pass_credential.clear();
                server
            })
            .collect();
        Self {
            schema: CONFIG_EXPORT_SCHEMA,
            application: crate::APP_NAME.to_owned(),
            connections,
        }
    }

    pub fn into_servers(self) -> Result<Vec<ServerConfig>, String> {
        if self.schema != CONFIG_EXPORT_SCHEMA {
            return Err(format!(
                "unsupported SSH MountMate config schema: {}",
                self.schema
            ));
        }
        if self.application != crate::APP_NAME || self.connections.len() > 10_000 {
            return Err("Invalid SSH MountMate configuration export".into());
        }
        let mut servers = self.connections;
        for server in &mut servers {
            if server.host.trim().is_empty()
                || server.user.trim().is_empty()
                || server.host.starts_with('-')
                || server.user.starts_with('-')
                || [&server.host, &server.user]
                    .iter()
                    .any(|s| s.chars().any(|c| c.is_whitespace() || c.is_control()))
                || crate::model::normalize_port(&server.port).is_none()
                || [
                    &server.name,
                    &server.remote_path,
                    &server.mountpoint,
                    &server.key_file,
                    &server.ssh_config_path,
                ]
                .iter()
                .any(|value| value.chars().any(char::is_control))
                || server.tags.len() > crate::model::MAX_CONNECTION_TAGS
                || server.tags.iter().any(|tag| {
                    tag.chars().count() > crate::model::MAX_TAG_CHARS
                        || tag.chars().any(char::is_control)
                })
                || !["manual", "ssh_config"].contains(&server.mode.as_str())
                || !["manual", "ssh_config", "ssh_config_batch", "sai_cluster"]
                    .contains(&server.source.as_str())
            {
                return Err("Invalid connection in SSH MountMate config".into());
            }
            // Imported files cannot claim ownership of arbitrary SSH files or
            // silently start mounts when the user next logs in.
            server.auto_mount_at_login = false;
            server.ssh_config_managed = false;
            server.copy_key_to_ssh_dir = false;
            server.managed_ssh_config_path.clear();
            server.normalize();
            if !server.password_obscured.is_empty()
                || !server.key_pass_obscured.is_empty()
                || !server.password_credential.is_empty()
                || !server.key_pass_credential.is_empty()
            {
                return Err("connection export contains a secret".into());
            }
        }
        Ok(servers)
    }
}

pub fn write_connection_export(path: &Path, servers: &[ServerConfig]) -> Result<(), String> {
    let content = serde_json::to_vec_pretty(&ConnectionExport::from_servers(servers))
        .map_err(|error| error.to_string())?;
    crate::storage::atomic_write(path, &content).map_err(|error| error.to_string())
}

pub fn read_connection_import(path: &Path) -> Result<Vec<ServerConfig>, String> {
    let mut content = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut content)
        .map_err(|e| e.to_string())?;
    if content.len() > 8 * 1024 * 1024 {
        return Err("Config file exceeds 8 MiB".into());
    }
    let export: ConnectionExport =
        serde_json::from_slice(&content).map_err(|error| error.to_string())?;
    export.into_servers()
}

pub fn plan_connection_import(
    path: &Path,
    existing: &[ServerConfig],
    protected_ids: &HashSet<String>,
) -> Result<SshImportPlan, String> {
    let servers = read_connection_import(path)?;
    Ok(plan_server_imports(servers, existing, protected_ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthMethod, ConnectionMethod};
    #[test]
    fn round_trip_preserves_details_and_never_exports_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let server = ServerConfig {
            id: "alpha".into(),
            name: "Work server".into(),
            host: "example.org".into(),
            user: "alice".into(),
            auth: AuthMethod::Password,
            connection_method: ConnectionMethod::Native,
            password_obscured: "sensitive".into(),
            password_credential: "credential-reference".into(),
            remote_path: "data".into(),
            tags: vec!["work".into()],
            auto_mount_at_login: true,
            ..Default::default()
        };
        let path = temp.path().join("export.json");
        write_connection_export(&path, &[server]).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("sensitive"));
        assert!(!text.contains("credential-reference"));
        let imported = read_connection_import(&path).unwrap();
        assert_eq!(imported[0].name, "Work server");
        assert_eq!(imported[0].remote_path, "data");
        assert_eq!(imported[0].tags, ["work"]);
        assert!(!imported[0].auto_mount_at_login);
        assert!(imported[0].password_obscured.is_empty());
    }
    #[test]
    fn import_keeps_distinct_mounts_and_protects_mounted_connections() {
        let base = ServerConfig {
            id: "alpha".into(),
            name: "One".into(),
            host: "example.org".into(),
            user: "alice".into(),
            remote_path: "one".into(),
            ..Default::default()
        };
        let second = ServerConfig {
            name: "Two".into(),
            remote_path: "two".into(),
            ..base.clone()
        };
        let plan = plan_server_imports(
            vec![base.clone(), second],
            &[base],
            &HashSet::from(["alpha".into()]),
        );
        assert!(!plan.items[0].can_overwrite);
        assert_eq!(plan.items[1].status, crate::connection::ImportStatus::New);
    }
    #[test]
    fn rejects_unknown_schema_and_credentials() {
        let mut data = ConnectionExport::from_servers(&[]);
        data.schema = 999;
        assert!(data.into_servers().is_err());
        let mut data = ConnectionExport::from_servers(&[ServerConfig {
            host: "example.org".into(),
            user: "alice".into(),
            ..Default::default()
        }]);
        data.connections[0].password_credential = "not-portable".into();
        assert!(data.into_servers().is_err());
    }

    #[test]
    fn json_overwrite_restores_exported_fields_but_keeps_local_id() {
        let local = ServerConfig {
            id: "local-id".into(),
            name: "Old".into(),
            host: "example.org".into(),
            user: "alice".into(),
            tags: vec!["old".into()],
            ..Default::default()
        };
        let imported = ServerConfig {
            id: "foreign-id".into(),
            name: "Restored".into(),
            tags: vec!["work".into()],
            connection_method: ConnectionMethod::Openssh,
            ..local.clone()
        };
        let plan = plan_server_imports(
            vec![imported],
            std::slice::from_ref(&local),
            &HashSet::new(),
        );
        let result = plan
            .apply(&[crate::connection::ImportAction::Overwrite], &[local])
            .unwrap();
        assert_eq!(result[0].id, "local-id");
        assert_eq!(result[0].name, "Restored");
        assert_eq!(result[0].tags, ["work"]);
        assert_eq!(result[0].connection_method, ConnectionMethod::Openssh);
    }
}
