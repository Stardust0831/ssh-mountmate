use std::path::{Path, PathBuf};

use crate::{ServerConfig, Settings};

pub fn known_hosts_marker(host: &str, port: &str) -> String {
    if port.trim().is_empty() || port == "22" {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

pub fn known_hosts_line_matches(line: &str, marker: &str) -> bool {
    line.split_whitespace()
        .next()
        .is_some_and(|hosts| hosts.split(',').any(|host| host == marker))
}

#[derive(Debug, Clone)]
pub struct MountCommand<'a> {
    pub rclone: &'a Path,
    pub config: &'a Path,
    pub server: &'a ServerConfig,
    pub settings: &'a Settings,
    pub remote: &'a str,
    pub mountpoint: &'a Path,
    pub cache_dir: &'a Path,
    pub log_path: &'a Path,
    pub rc_addr: &'a str,
    pub windows: bool,
}

impl MountCommand<'_> {
    pub fn build(&self) -> Vec<String> {
        let cache_mode = self.server.effective_cache_mode(self.settings);
        let mut command = vec![
            self.rclone.display().to_string(),
            "--config".into(),
            self.config.display().to_string(),
            "--rc".into(),
            "--rc-no-auth".into(),
            "--rc-addr".into(),
            self.rc_addr.into(),
            "mount".into(),
            self.remote.into(),
            self.mountpoint.display().to_string(),
            "--vfs-fast-fingerprint".into(),
            "--links".into(),
            "--cache-dir".into(),
            self.cache_dir.display().to_string(),
            "--log-file".into(),
            self.log_path.display().to_string(),
            "--volname".into(),
            self.server.display_name().into(),
            "--vfs-cache-mode".into(),
            cache_mode.into(),
        ];
        push_option(
            &mut command,
            "--vfs-cache-max-size",
            &self.settings.vfs_cache_max_size,
        );
        push_option(
            &mut command,
            "--vfs-cache-max-age",
            &self.settings.vfs_cache_max_age,
        );
        push_option(
            &mut command,
            "--vfs-cache-min-free-space",
            &self.settings.vfs_cache_min_free_space,
        );
        if matches!(cache_mode, "writes" | "full") {
            push_option(
                &mut command,
                "--vfs-write-back",
                &self.settings.vfs_write_back,
            );
        }
        push_option(
            &mut command,
            "--dir-cache-time",
            &self.settings.dir_cache_time,
        );
        push_option(&mut command, "--buffer-size", &self.settings.buffer_size);
        if self.windows && self.server.network_mode && is_windows_drive(self.mountpoint) {
            command.push("--network-mode".into());
        }
        command
    }
}

fn push_option(command: &mut Vec<String>, option: &str, value: &str) {
    if !value.is_empty() {
        command.extend([option.into(), value.into()]);
    }
}

pub fn is_windows_drive(path: &Path) -> bool {
    let value = path.as_os_str().to_string_lossy();
    value.len() == 2 && value.as_bytes()[0].is_ascii_alphabetic() && value.ends_with(':')
}

pub fn normalize_explorer_refresh_path(value: &str, windows: bool) -> String {
    if !windows {
        return value.to_owned();
    }
    let mut value = value.to_owned();
    if value.starts_with('"') {
        value.remove(0);
    }
    if value.ends_with('"') {
        value.pop();
        value.push('\\');
    }
    value
}

pub fn normalize_refresh_relative_path(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let normalized = normalized.trim_matches('/');
    if matches!(normalized, "" | "." | "\"") {
        String::new()
    } else {
        normalized.to_owned()
    }
}

pub fn cache_directory(settings: &Settings, server: &ServerConfig) -> PathBuf {
    settings.cache_root.join(server.remote_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(cache_mode: &str) -> Vec<String> {
        let server = ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            cache_mode: cache_mode.into(),
            ..ServerConfig::default()
        };
        MountCommand {
            rclone: Path::new("rclone"),
            config: Path::new("rclone.conf"),
            server: &server,
            settings: &Settings::default(),
            remote: "alpha:",
            mountpoint: Path::new("R:"),
            cache_dir: Path::new("cache"),
            log_path: Path::new("alpha.log"),
            rc_addr: "127.0.0.1:1234",
            windows: true,
        }
        .build()
    }

    #[test]
    fn mount_command_keeps_reliability_flags_and_defaults() {
        let command = command("");
        assert!(command.contains(&"--links".into()));
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--vfs-cache-mode", "full"])
        );
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--vfs-cache-max-age", "30m"])
        );
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--vfs-write-back", "0s"])
        );
        assert!(
            command
                .windows(2)
                .any(|item| item == ["--dir-cache-time", "5m"])
        );
    }

    #[test]
    fn write_back_is_not_passed_for_non_write_cache_modes() {
        let command = command("off");
        assert!(!command.contains(&"--vfs-write-back".into()));
    }

    #[test]
    fn host_marker_includes_nonstandard_port() {
        assert_eq!(known_hosts_marker("example.com", "22"), "example.com");
        assert_eq!(
            known_hosts_marker("example.com", "12022"),
            "[example.com]:12022"
        );
        assert!(known_hosts_line_matches(
            "[example.com]:12022 ssh-ed25519 AAAA",
            "[example.com]:12022"
        ));
    }

    #[test]
    fn explorer_drive_root_repairs_trailing_quote() {
        assert_eq!(normalize_explorer_refresh_path("Y:\"", true), "Y:\\");
        assert_eq!(
            normalize_explorer_refresh_path("\"C:\\Mount Folder\"", true),
            "C:\\Mount Folder\\"
        );
        assert_eq!(normalize_refresh_relative_path("\""), "");
    }
}
