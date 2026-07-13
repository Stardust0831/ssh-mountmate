use std::fs;
use std::path::{Path, PathBuf};

use configparser::ini::Ini;
use mountmate_core::model::{ConnectionMethod, MountPhase, SETTINGS_SCHEMA_VERSION};
use mountmate_core::paths::AppPaths;
use mountmate_core::rclone::{RcloneRemote, write_rclone_remote};
use mountmate_core::storage::{
    load_servers, load_settings, read_json, save_servers, save_settings,
};
use mountmate_core::{AuthMethod, MountState};
use tempfile::tempdir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/legacy-v0.3")
        .join(name)
}

fn copy(source: &str, destination: &Path) {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::copy(fixture(source), destination).unwrap();
}

#[test]
fn complete_python_layout_migrates_without_losing_user_data() {
    let temp = tempdir().unwrap();
    let paths = AppPaths {
        config_dir: temp.path().join("config/rsshmount"),
        cache_dir: temp.path().join("cache/rsshmount"),
        state_dir: temp.path().join("state/rsshmount"),
        data_dir: temp.path().join("data/ssh-mountmate"),
    };
    copy("servers.json", &paths.servers_file());
    copy("settings.json", &paths.settings_file());
    copy("rclone.conf", &paths.rclone_config());
    copy("state.json", &paths.state_file("legacy-password"));
    let cache_sentinel = paths.cache_dir.join("legacy-password/cache-sentinel.txt");
    let log_sentinel = paths.mount_log("legacy-password");
    copy("cache-sentinel.txt", &cache_sentinel);
    copy("log-sentinel.txt", &log_sentinel);

    let servers = load_servers(&paths).unwrap();
    assert_eq!(servers.len(), 3);
    let password = servers
        .iter()
        .find(|server| server.id == "legacy-password")
        .unwrap();
    assert_eq!(password.auth, AuthMethod::Password);
    assert_eq!(password.password_obscured, "obscured-password-secret");
    assert_eq!(password.port, "2202");
    assert_eq!(password.cache_mode, "minimal");
    assert!(password.network_mode);

    let sai = servers
        .iter()
        .find(|server| server.id == "legacy-sai")
        .unwrap();
    assert_eq!(sai.key_pass_obscured, "obscured-key-passphrase");
    assert!(sai.ssh_config_managed);
    assert!(sai.copy_key_to_ssh_dir);
    assert_eq!(sai.managed_ssh_config_path, "/legacy/.ssh/ssh-mountmate.d");

    let openssh = servers
        .iter()
        .find(|server| server.id == "legacy-openssh")
        .unwrap();
    assert_eq!(openssh.connection_method, ConnectionMethod::Openssh);
    assert_eq!(openssh.ssh_config_path, "/legacy/.ssh/config");

    let settings = load_settings(&paths).unwrap();
    assert_eq!(settings.settings_schema_version, SETTINGS_SCHEMA_VERSION);
    assert_eq!(settings.cache_root, Path::new("/legacy/custom-cache"));
    assert_eq!(settings.vfs_cache_mode, "writes");
    assert_eq!(settings.vfs_cache_max_size, "50G");
    assert_eq!(settings.vfs_cache_max_age, "2h");
    assert_eq!(settings.vfs_cache_min_free_space, "5G");
    assert_eq!(settings.vfs_write_back, "17s");
    assert_eq!(settings.dir_cache_time, "9m");
    assert_eq!(settings.buffer_size, "32M");
    assert!(settings.startup_all);
    assert!(!settings.auto_show_transfers);
    assert!(!settings.auto_check_updates);
    assert_eq!(settings.language, "zh-CN");

    let state: MountState = read_json(&paths.state_file("legacy-password")).unwrap();
    assert_eq!(state.pid, 4242);
    assert_eq!(state.server_id, "legacy-password");
    assert_eq!(state.phase, MountPhase::Mounted);
    assert_eq!(state.rc_addr, "127.0.0.1:45321");

    let remote = RcloneRemote::for_server(password, None, None, false).unwrap();
    write_rclone_remote(&paths, &remote).unwrap();
    let mut config = Ini::new_cs();
    config.load(paths.rclone_config()).unwrap();
    assert_eq!(
        config.get("unrelated-cloud", "access_key_id").as_deref(),
        Some("preserved-access-key")
    );
    assert_eq!(
        config
            .get("unrelated-cloud", "secret_access_key")
            .as_deref(),
        Some("preserved-secret")
    );
    assert_eq!(
        config.get("legacy-password", "pass").as_deref(),
        Some("obscured-password-secret")
    );

    save_servers(&paths, &servers).unwrap();
    save_settings(&paths, &settings).unwrap();
    assert_eq!(load_servers(&paths).unwrap(), servers);
    assert_eq!(load_settings(&paths).unwrap(), settings);
    assert_eq!(
        fs::read(cache_sentinel).unwrap(),
        fs::read(fixture("cache-sentinel.txt")).unwrap()
    );
    assert_eq!(
        fs::read(log_sentinel).unwrap(),
        fs::read(fixture("log-sentinel.txt")).unwrap()
    );
}
