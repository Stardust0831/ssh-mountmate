#[cfg(unix)]
use mountmate_core::ConnectionMethod;
use mountmate_core::config_transfer::ConnectionExport;
use mountmate_core::connection::{ConnectionDraft, validate_mount_mappings};
use mountmate_core::model::MountMapping;
use mountmate_core::paths::AppPaths;
use mountmate_core::storage::{load_servers, save_servers, update_mount_paths};
use mountmate_core::{AuthMethod, ServerConfig};
use std::fs;
use std::path::Path;

fn profile() -> ServerConfig {
    let mut server = ServerConfig {
        id: "work".into(),
        name: "Work".into(),
        host: "host.example".into(),
        user: "alice".into(),
        auth: AuthMethod::Password,
        password_obscured: "test-secret".into(),
        password_credential: "test-reference".into(),
        mountpoint: "P:".into(),
        mounts: vec![MountMapping {
            remote_path: "/projects".into(),
            mountpoint: "Q:".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    server.normalize();
    server
}
fn paths(root: &Path) -> AppPaths {
    AppPaths {
        config_dir: root.join("config"),
        cache_dir: root.join("cache"),
        state_dir: root.join("state"),
        data_dir: root.join("data"),
    }
}

#[test]
fn old_configuration_keeps_its_original_runtime_and_cache_identity() {
    let old: ServerConfig = serde_json::from_str(r#"{"id":"work","mode":"ssh_config","host_alias":"cluster","remote_path":"/home/me","mountpoint":"P:"}"#).unwrap();
    let targets = old.mount_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, "work");
    assert_eq!(targets[0].remote_name(), "cluster");
    assert_eq!(targets[0].remote_spec(), "cluster:/home/me");
}

#[test]
fn mappings_share_authentication_but_keep_separate_runtime_and_cache_names() {
    let mut server = profile();
    server.mode = "ssh_config".into();
    server.host_alias = "cluster".into();
    let targets = server.mount_targets();
    assert_ne!(targets[0].id, targets[1].id);
    assert_ne!(targets[0].remote_name(), targets[1].remote_name());
    for target in &targets {
        assert_eq!(target.connection_id(), server.id);
        assert_eq!(target.password_credential, server.password_credential);
        assert_eq!(target.password_obscured, server.password_obscured);
        assert_eq!(target.host_alias, server.host_alias);
        assert!(target.mounts.is_empty());
        assert_eq!(target.mount_targets(), vec![target.clone()]);
    }
    let saved = serde_json::to_string(&server).unwrap();
    assert_eq!(saved.matches("test-secret").count(), 1);
    assert_eq!(saved.matches("test-reference").count(), 1);
    assert!(!saved.contains("connection_id"));
}

#[test]
fn mapping_ids_survive_reordering_and_removal_of_the_original_path() {
    let mut server = profile();
    server.mounts.push(MountMapping {
        remote_path: "/datasets".into(),
        mountpoint: "R:".into(),
        ..Default::default()
    });
    let targets = server.mount_targets();
    server.primary_mount = false;
    server.mounts.reverse();
    assert_eq!(
        server.mount_targets(),
        vec![targets[2].clone(), targets[1].clone()]
    );
}

#[test]
fn validation_rejects_collisions_and_invalid_ids_but_allows_distinct_directories() {
    let mut server = profile();
    validate_mount_mappings(&server, &[]).unwrap();
    server.mounts[0].mountpoint = "p:\\".into();
    assert!(validate_mount_mappings(&server, &[]).is_err());
    server.mountpoint = "/mnt/project".into();
    server.mounts[0].mountpoint = "/mnt/sub/../project/".into();
    assert!(validate_mount_mappings(&server, &[]).is_err());
    server = profile();
    let other = ServerConfig {
        id: "other".into(),
        mountpoint: "Q:".into(),
        ..Default::default()
    };
    assert!(validate_mount_mappings(&server, &[other]).is_err());
    server.mounts[0].id = "../../outside".into();
    assert!(validate_mount_mappings(&server, &[]).is_err());
}

#[test]
fn editing_multiple_paths_preserves_shared_password_without_reentry() {
    let server = profile();
    let mut draft = ConnectionDraft::from_server(&server);
    draft.mounts[0].remote_path = "~/other".into();
    draft.mounts.push(MountMapping {
        mountpoint: "R:".into(),
        ..Default::default()
    });
    let validated = draft.validate(std::slice::from_ref(&server)).unwrap();
    assert_eq!(validated.server.mounts[0].remote_path, "other");
    assert_eq!(
        validated.server.password_credential,
        server.password_credential
    );
    assert_eq!(validated.server.mounts.len(), 2);
}

#[test]
fn export_round_trip_preserves_mappings_and_accepts_legacy_schema_without_secrets() {
    let mut server = profile();
    server.primary_mount = false;
    let export = ConnectionExport::from_servers(std::slice::from_ref(&server));
    assert_eq!(export.schema, 2);
    let json = serde_json::to_string(&export).unwrap();
    assert!(!json.contains("test-secret"));
    assert!(!json.contains("test-reference"));
    let restored = export.into_servers().unwrap().remove(0);
    assert_eq!(restored.mounts, server.mounts);
    assert!(!restored.primary_mount);
    let mut old = ConnectionExport::from_servers(&[ServerConfig {
        mounts: vec![],
        ..profile()
    }]);
    old.schema = 1;
    assert_eq!(old.into_servers().unwrap()[0].mount_targets().len(), 1);
}

#[test]
fn add_and_remove_idle_rows_preserves_active_sibling_state_cache_and_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let server = profile();
    save_servers(&paths, std::slice::from_ref(&server)).unwrap();
    let child = server.mount_targets().remove(1);
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(paths.state_file(&child.id), b"active child state").unwrap();
    let cache = paths.mount_cache_dir(child.remote_name());
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("pending-upload"), b"unsent data").unwrap();
    let mut mappings = server.mounts.clone();
    mappings.push(MountMapping {
        mountpoint: "R:".into(),
        ..Default::default()
    });
    let saved = update_mount_paths(
        &paths,
        &server,
        server.remote_path.clone(),
        server.mountpoint.clone(),
        mappings,
        false,
    )
    .unwrap()
    .remove(0);
    assert_eq!(saved.password_credential, server.password_credential);
    assert_eq!(saved.password_obscured, server.password_obscured);
    assert_eq!(saved.mount_targets()[0], child);
    assert_eq!(
        fs::read(paths.state_file(&child.id)).unwrap(),
        b"active child state"
    );
    assert_eq!(
        fs::read(cache.join("pending-upload")).unwrap(),
        b"unsent data"
    );
    let mut changed = saved.mounts.clone();
    changed[0].remote_path = "/wrong".into();
    assert!(
        update_mount_paths(
            &paths,
            &saved,
            saved.remote_path.clone(),
            saved.mountpoint.clone(),
            changed,
            false
        )
        .is_err()
    );
    let removed = saved.mounts[1..].to_vec();
    assert!(
        update_mount_paths(
            &paths,
            &saved,
            saved.remote_path.clone(),
            saved.mountpoint.clone(),
            removed,
            false
        )
        .is_err()
    );
    assert_eq!(load_servers(&paths).unwrap(), vec![saved]);
}

#[test]
fn mapping_save_detects_stale_editor_and_concurrent_mount_start() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let server = profile();
    save_servers(&paths, std::slice::from_ref(&server)).unwrap();
    let lock = mountmate_core::storage::FileLock::acquire(
        &paths.mount_lock(&server.id),
        std::time::Duration::ZERO,
    )
    .unwrap();
    assert!(
        update_mount_paths(
            &paths,
            &server,
            "/other".into(),
            server.mountpoint.clone(),
            server.mounts.clone(),
            true
        )
        .is_err()
    );
    drop(lock);
    let mut newer = server.clone();
    newer.user = "bob".into();
    save_servers(&paths, &[newer]).unwrap();
    assert!(
        update_mount_paths(
            &paths,
            &server,
            "/other".into(),
            server.mountpoint.clone(),
            server.mounts.clone(),
            true
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn interactive_mappings_reuse_the_same_authenticated_control_socket() {
    let temp = tempfile::tempdir().unwrap();
    let mut server = profile();
    server.connection_method = ConnectionMethod::Interactive;
    let targets = server.mount_targets();
    let root = temp.path().canonicalize().unwrap();
    let paths = paths(&root);
    let sessions: Vec<_> = targets
        .iter()
        .map(|s| {
            mountmate_core::interactive_ssh::InteractiveSshSession::for_server(&paths, &root, s)
                .unwrap()
        })
        .collect();
    assert_eq!(
        sessions[0].connector_arguments(),
        sessions[1].connector_arguments()
    );
}

#[test]
fn loading_rejects_unsafe_mapping_ids_before_runtime_paths_are_created() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    let mut server = profile();
    server.mounts[0].id = "../../escape".into();
    save_servers(&paths, &[server]).unwrap();
    assert!(load_servers(&paths).is_err());
    assert!(!paths.state_dir.exists());
}
