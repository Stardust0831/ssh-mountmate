//! Inventory and validation for explicit removal of application-owned files.
use crate::{data_migration::is_link, paths::AppPaths};
use std::{
    fs,
    path::{Path, PathBuf},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileProcessKind {
    Mount,
    Update,
    ApplicationFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileProcess {
    pid: u32,
    name: String,
    kind: ProfileProcessKind,
}

pub fn ensure_no_profile_processes(paths: &AppPaths) -> Result<(), String> {
    wait_for_profile_processes(std::time::Duration::ZERO, || profile_processes(paths))
}

/// An update can still be committing its backup after the new GUI appears.
/// Wait briefly for that helper, but never stop it or an active mount.
pub fn ensure_profiles_idle_for_uninstall(profiles: &[AppPaths]) -> Result<(), String> {
    wait_for_profile_processes(std::time::Duration::from_secs(5), || {
        let mut processes = Vec::new();
        for paths in profiles {
            processes.extend(profile_processes(paths)?);
        }
        processes.sort_by_key(|p| p.pid);
        processes.dedup_by_key(|p| p.pid);
        Ok(processes)
    })
}

fn wait_for_profile_processes(
    timeout: std::time::Duration,
    mut inspect: impl FnMut() -> Result<Vec<ProfileProcess>, String>,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let processes = inspect()?;
        if processes.is_empty() {
            return Ok(());
        }
        if processes
            .iter()
            .all(|p| p.kind == ProfileProcessKind::Update)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(150));
            continue;
        }
        let details = processes
            .iter()
            .map(|process| {
                let activity = match process.kind {
                    ProfileProcessKind::Mount => "active mount / 挂载仍在运行，请先取消挂载",
                    ProfileProcessKind::Update => "finishing update / 更新正在收尾，请稍后重试",
                    ProfileProcessKind::ApplicationFile => {
                        "using application files / 后台进程仍在使用应用文件，请先退出该进程"
                    }
                };
                format!("{} (PID {}): {activity}", process.name, process.pid)
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "Application files are still in use / 以下进程仍在使用应用文件：\n{details}"
        ));
    }
}

pub fn profile_has_processes(paths: &AppPaths) -> Result<bool, String> {
    profile_processes(paths).map(|processes| !processes.is_empty())
}

fn profile_processes(paths: &AppPaths) -> Result<Vec<ProfileProcess>, String> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );
    // Include orphaned mount records, not only connections still in servers.json.
    let mut states = Vec::new();
    if paths.state_dir.exists() {
        for entry in fs::read_dir(&paths.state_dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "json")
                && path.file_name().is_none_or(|n| n != "app-command.json")
            {
                let value: serde_json::Value =
                    crate::storage::read_json(&path).map_err(|e| e.to_string())?;
                if value.get("pid").is_some() {
                    let state: crate::MountState = serde_json::from_value(value).map_err(|e| {
                        format!(
                            "Cannot read mount record / 无法读取挂载记录 {}: {e}",
                            path.display()
                        )
                    })?;
                    states.push(state);
                }
            }
        }
    }
    let mut blockers = Vec::new();
    for (pid, process) in system.processes() {
        if pid.as_u32() == std::process::id()
            || matches!(
                process.status(),
                sysinfo::ProcessStatus::Dead | sysinfo::ProcessStatus::Zombie
            )
        {
            continue;
        }
        let arguments: Vec<String> = process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let kind = classify_profile_process(paths, process.exe(), &arguments, cfg!(windows))
            .or_else(|| {
                states
                    .iter()
                    .any(|state| {
                        state.pid == pid.as_u32()
                            && recorded_mount_matches(
                                state,
                                process.exe(),
                                &arguments,
                                process.start_time(),
                                cfg!(windows),
                            )
                    })
                    .then_some(ProfileProcessKind::Mount)
            });
        if let Some(kind) = kind {
            blockers.push(ProfileProcess {
                pid: pid.as_u32(),
                name: process.name().to_string_lossy().into_owned(),
                kind,
            });
        }
    }
    blockers.sort_by_key(|p| p.pid);
    Ok(blockers)
}

fn normalized_path(path: &Path, windows: bool) -> String {
    let path = crate::mountpoint::path_key(path, windows);
    if windows {
        if let Some(unc) = path.strip_prefix("//?/unc/") {
            return format!("//{unc}");
        }
        return path.strip_prefix("//?/").unwrap_or(&path).to_owned();
    }
    path
}

fn path_is_within(path: &Path, root: &Path, windows: bool) -> bool {
    let path = normalized_path(path, windows);
    let root = normalized_path(root, windows);
    !root.is_empty() && (path == root || path.starts_with(&format!("{root}/")))
}

fn profile_option(arguments: &[String], option: &str, paths: &AppPaths, windows: bool) -> bool {
    arguments
        .iter()
        .enumerate()
        .skip(1)
        .any(|(index, argument)| {
            let value = if argument == option {
                arguments.get(index + 1).map(String::as_str)
            } else {
                argument
                    .strip_prefix(option)
                    .and_then(|v| v.strip_prefix('='))
            };
            value.is_some_and(|value| {
                [&paths.state_dir, &paths.cache_dir, &paths.config_dir]
                    .iter()
                    .any(|root| path_is_within(Path::new(value), root, windows))
            })
        })
}

fn classify_profile_process(
    paths: &AppPaths,
    executable: Option<&Path>,
    arguments: &[String],
    windows: bool,
) -> Option<ProfileProcessKind> {
    let owned_executable = executable.is_some_and(|exe| {
        path_is_within(exe, &paths.data_dir, windows)
            || paths
                .legacy_managed_bin_dirs()
                .iter()
                .any(|root| path_is_within(exe, root, windows))
    });
    let executable = executable.or_else(|| arguments.first().map(Path::new));
    let name = executable
        .map(|exe| normalized_path(exe, windows))
        .and_then(|exe| exe.rsplit('/').next().map(str::to_owned))
        .unwrap_or_default();
    let rclone = name == "rclone" || name == "rclone.exe" || name.starts_with("rclone-");
    if rclone
        && arguments
            .iter()
            .any(|arg| matches!(arg.as_str(), "mount" | "nfsmount"))
        && (owned_executable
            || ["--config", "--cache-dir", "--log-file", "--rc-htpasswd"]
                .iter()
                .any(|option| profile_option(arguments, option, paths, windows)))
    {
        return Some(ProfileProcessKind::Mount);
    }
    // The helper executable lives in the shared data tree even when its plan
    // still refers to the legacy state directory during a cross-version update.
    if owned_executable
        && arguments
            .get(1)
            .is_some_and(|arg| arg == "--run-update-helper")
    {
        return Some(ProfileProcessKind::Update);
    }
    // An executable in our data tree cannot be removed while it is running.
    // A file manager/editor merely receiving a profile path is not such a user.
    owned_executable.then_some(ProfileProcessKind::ApplicationFile)
}

fn recorded_mount_matches(
    state: &crate::MountState,
    executable: Option<&Path>,
    arguments: &[String],
    started_at: u64,
    windows: bool,
) -> bool {
    if state
        .process_started_at
        .is_some_and(|started| started != started_at)
    {
        return false;
    }
    if !arguments.is_empty() {
        return crate::process::argv_matches_state(arguments, state, windows);
    }
    if !state.rclone.as_os_str().is_empty()
        && let Some(executable) = executable
    {
        return normalized_path(executable, windows) == normalized_path(&state.rclone, windows);
    }
    // Unreadable process arguments are not evidence of PID reuse, but an old
    // record without any process identity must never block on a PID alone.
    state.process_started_at == Some(started_at)
}

/// Only recognize updater transaction names, never a broad executable glob.
pub fn is_update_transaction(name: &str) -> bool {
    is_update_transaction_for(name, "SSHMountMate")
}

fn is_update_transaction_for(name: &str, current_stem: &str) -> bool {
    let Some((program, phase)) = name
        .strip_prefix('.')
        .and_then(|s| s.split_once(".ssh-mountmate-"))
    else {
        return false;
    };
    if program != current_stem
        && program != "SSHMountMate"
        && !program
            .strip_prefix("SSHMountMate-v")
            .is_some_and(|v| semver::Version::parse(v).is_ok())
    {
        return false;
    }
    let phase = phase.strip_suffix(".exe").unwrap_or(phase);
    phase == "backup"
        || ["prepared-", "recovered-"].iter().any(|p| {
            phase
                .strip_prefix(p)
                .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        })
}

pub fn validate_removal_tree(path: &Path) -> Result<(), String> {
    crate::data_migration::validate_ancestors(path).map_err(|e| e.to_string())?;
    validate_removal_children(path)
}

fn validate_removal_children(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if is_link(&metadata) {
        return Err(format!("Linked paths are not removed: {}", path.display()));
    }
    if metadata.is_dir() {
        for e in fs::read_dir(path).map_err(|e| e.to_string())? {
            validate_removal_children(&e.map_err(|e| e.to_string())?.path())?;
        }
    } else if !metadata.is_file() {
        return Err(format!("Unexpected file type: {}", path.display()));
    }
    Ok(())
}

pub fn uninstall_inventory(
    paths: &AppPaths,
    executable: &Path,
    servers: &[crate::ServerConfig],
    settings: &crate::Settings,
) -> Result<Vec<PathBuf>, String> {
    let mut targets = vec![
        paths.config_dir.clone(),
        paths.cache_dir.clone(),
        paths.state_dir.clone(),
        paths.data_dir.clone(),
        executable.to_owned(),
    ];
    targets.extend(paths.legacy_application_directories());
    // Custom cache roots may be shared. Only the exact per-connection cache is
    // owned by this application, never the user's chosen parent directory.
    if !settings.cache_root.as_os_str().is_empty() && settings.cache_root != paths.cache_dir {
        for server in servers {
            let remote = server.remote_name();
            if remote.is_empty()
                || Path::new(remote).components().count() != 1
                || remote == "."
                || remote == ".."
                || remote.contains(['/', '\\', ':'])
            {
                return Err("Invalid per-connection cache path".into());
            }
            targets.push(crate::service::expand_home_path(&settings.cache_root).join(remote));
        }
    }
    let parent = executable.parent().ok_or("Executable has no parent")?;
    let stem = executable
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Invalid executable name")?;
    for entry in fs::read_dir(parent).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| is_update_transaction_for(name, stem))
        {
            targets.push(entry.path());
        }
    }
    targets.sort();
    targets.dedup();
    let all = targets.clone();
    targets.retain(|p| !all.iter().any(|root| root != p && p.starts_with(root)));
    for target in &targets {
        if !target.is_absolute() || target.parent().is_none() {
            return Err("Uninstall requires absolute owned paths".into());
        }
        validate_removal_tree(target)?;
    }
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_paths() -> AppPaths {
        let data_dir = PathBuf::from("C:/Users/test/AppData/Local/ssh-mountmate");
        AppPaths {
            config_dir: data_dir.join("config"),
            cache_dir: data_dir.join("cache"),
            state_dir: data_dir.join("state"),
            data_dir,
        }
    }

    fn classify(exe: &str, args: &[&str]) -> Option<ProfileProcessKind> {
        let arguments = std::iter::once(exe)
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        classify_profile_process(&windows_paths(), Some(Path::new(exe)), &arguments, true)
    }

    #[test]
    fn profile_path_arguments_are_not_proof_of_an_active_mount_or_update() {
        assert_eq!(
            classify(
                "C:/Windows/explorer.exe",
                &["C:/Users/test/AppData/Local/ssh-mountmate/state"]
            ),
            None
        );
        assert_eq!(
            classify(
                "C:/Windows/notepad.exe",
                &["C:/Users/test/AppData/Local/ssh-mountmate/config/settings.json"]
            ),
            None
        );
        assert_eq!(
            classify(
                "C:/Tools/rclone.exe",
                &[
                    "about",
                    "server:",
                    "--config",
                    "C:/Users/test/AppData/Local/ssh-mountmate/config/rclone.conf"
                ]
            ),
            None
        );
        assert_eq!(
            classify(
                "C:/Tools/SSHMountMate.exe",
                &[
                    "--update-health-marker",
                    "C:/Users/test/AppData/Local/ssh-mountmate/state/update/health.json"
                ]
            ),
            None
        );
    }

    #[test]
    fn recognizes_real_helpers_and_orphan_mounts_for_both_profile_layouts() {
        for plan in [
            "C:/Users/test/AppData/Local/ssh-mountmate/state/update/plan.json",
            "C:/Users/test/AppData/Local/rsshmount/State/update/plan.json",
        ] {
            let mut paths = windows_paths();
            if plan.contains("rsshmount") {
                paths.state_dir = PathBuf::from("C:/Users/test/AppData/Local/rsshmount/State");
            }
            let exe = paths
                .update_helper_dir()
                .join("SSHMountMate-helper-123.exe");
            let arguments = vec![
                exe.display().to_string(),
                "--run-update-helper".into(),
                plan.into(),
                "--update-helper-token".into(),
                "secret-token".into(),
            ];
            assert_eq!(
                classify_profile_process(&paths, Some(&exe), &arguments, true),
                Some(ProfileProcessKind::Update)
            );
            assert_eq!(
                classify_profile_process(&windows_paths(), Some(&exe), &arguments, true),
                Some(ProfileProcessKind::Update)
            );
        }
        assert_eq!(
            classify(
                "C:/Tools/rclone.exe",
                &[
                    "mount",
                    "server:",
                    "R:",
                    "--log-file=C:/Users/test/AppData/Local/ssh-mountmate/state/server.log"
                ]
            ),
            Some(ProfileProcessKind::Mount)
        );
        assert_eq!(
            classify(
                r"\\?\C:\Users\TEST\AppData\Local\ssh-mountmate\bin\rclone-123.exe",
                &["mount", "server:", "R:"]
            ),
            Some(ProfileProcessKind::Mount)
        );
        assert_eq!(
            classify(
                "C:/Tools/rclone.exe",
                &[
                    "mount",
                    "other:",
                    "R:",
                    "--log-file",
                    "C:/Users/test/AppData/Local/ssh-mountmate-other/state/other.log"
                ]
            ),
            None
        );
        // Background tools in the data directory still block removal of their
        // executable, but no longer claim that a mount or update is active.
        assert_eq!(
            classify(
                "C:/Users/test/AppData/Local/ssh-mountmate/bin/rclone-123.exe",
                &["version"]
            ),
            Some(ProfileProcessKind::ApplicationFile)
        );
    }

    fn recorded_mount() -> crate::MountState {
        crate::MountState {
            pid: 42,
            server_id: "server".into(),
            remote: "server:data".into(),
            mountpoint: PathBuf::from("R:"),
            log: windows_paths().mount_log("server"),
            rc_addr: "127.0.0.1:1234".into(),
            rc_user: String::new(),
            rc_pass: String::new(),
            phase: crate::MountPhase::Mounted,
            process_started_at: Some(100),
            rclone: PathBuf::from("C:/Tools/rclone.exe"),
            mount_backend: crate::MountBackend::Fuse,
        }
    }

    #[test]
    fn old_state_cannot_block_uninstall_just_because_a_pid_is_reused() {
        let mut state = recorded_mount();
        let arguments = vec![
            state.rclone.display().to_string(),
            "mount".into(),
            state.remote.clone(),
            "R:".into(),
            "--log-file".into(),
            state.log.display().to_string(),
        ];
        assert!(recorded_mount_matches(
            &state,
            Some(&state.rclone),
            &arguments,
            100,
            true
        ));
        assert!(!recorded_mount_matches(
            &state,
            Some(&state.rclone),
            &arguments,
            101,
            true
        ));
        state.process_started_at = None;
        assert!(recorded_mount_matches(
            &state,
            Some(&state.rclone),
            &arguments,
            200,
            true
        ));
        assert!(!recorded_mount_matches(
            &state,
            Some(Path::new("C:/Windows/notepad.exe")),
            &["notepad.exe".into()],
            100,
            true
        ));
        assert!(!recorded_mount_matches(&state, None, &[], 100, true));
        // When Windows denies command-line access, retain protection for a
        // process whose executable or recorded start time still matches.
        assert!(recorded_mount_matches(
            &state,
            Some(&state.rclone),
            &[],
            100,
            true
        ));
        state.process_started_at = Some(100);
        assert!(recorded_mount_matches(&state, None, &[], 100, true));
    }

    #[test]
    fn uninstall_rechecks_until_update_helper_finishes() {
        let mut inspections = 0;
        wait_for_profile_processes(std::time::Duration::from_secs(1), || {
            inspections += 1;
            Ok(if inspections == 1 {
                vec![ProfileProcess {
                    pid: 42,
                    name: "SSHMountMate-helper-123.exe".into(),
                    kind: ProfileProcessKind::Update,
                }]
            } else {
                Vec::new()
            })
        })
        .unwrap();
        assert_eq!(inspections, 2);
    }

    #[test]
    fn persistent_update_and_real_mount_report_the_blocking_process() {
        let update_error = wait_for_profile_processes(std::time::Duration::ZERO, || {
            Ok(vec![ProfileProcess {
                pid: 42,
                name: "SSHMountMate-helper-123.exe".into(),
                kind: ProfileProcessKind::Update,
            }])
        })
        .unwrap_err();
        assert!(update_error.contains("SSHMountMate-helper-123.exe (PID 42)"));
        assert!(update_error.contains("更新正在收尾"));
        assert!(!update_error.contains("挂载仍在运行"));
        let mut inspections = 0;
        let mount_error = wait_for_profile_processes(std::time::Duration::from_secs(5), || {
            inspections += 1;
            Ok(vec![ProfileProcess {
                pid: 99,
                name: "rclone.exe".into(),
                kind: ProfileProcessKind::Mount,
            }])
        })
        .unwrap_err();
        assert_eq!(inspections, 1);
        assert!(mount_error.contains("rclone.exe (PID 99)"));
        assert!(mount_error.contains("请先取消挂载"));
    }

    #[test]
    fn recognizes_only_transaction_files() {
        assert!(is_update_transaction_for(
            ".my-app.ssh-mountmate-backup.exe",
            "my-app"
        ));
        assert!(!is_update_transaction_for(
            ".neighbor.ssh-mountmate-backup.exe",
            "my-app"
        ));
        assert!(is_update_transaction(
            ".SSHMountMate.ssh-mountmate-backup.exe"
        ));
        assert!(is_update_transaction(
            ".SSHMountMate-v0.6.9.ssh-mountmate-prepared-0123456789abcdef0123456789abcdef.exe"
        ));
        for name in [
            "SSHMountMate.exe",
            ".other.ssh-mountmate-backup.exe",
            ".SSHMountMate.ssh-mountmate-recovered-important.exe",
        ] {
            assert!(!is_update_transaction(name));
        }
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_trees_without_touching_target() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, b"keep").unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert!(validate_removal_tree(&root).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }
}
