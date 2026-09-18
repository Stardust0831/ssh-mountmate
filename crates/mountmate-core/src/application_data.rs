//! Inventory and validation for explicit removal of application-owned files.
use crate::{data_migration::is_link, paths::AppPaths};
use std::{
    fs,
    path::{Path, PathBuf},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

pub fn ensure_no_profile_processes(paths: &AppPaths) -> Result<(), String> {
    if profile_has_processes(paths)? {
        Err(
            "An application mount/update process is still running. 请先取消挂载并等待更新结束。"
                .into(),
        )
    } else {
        Ok(())
    }
}

pub fn profile_has_processes(paths: &AppPaths) -> Result<bool, String> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );
    for (pid, process) in system.processes() {
        if pid.as_u32() == std::process::id() {
            continue;
        }
        let owned_executable = process
            .exe()
            .is_some_and(|exe| exe.starts_with(&paths.data_dir));
        let references_profile = process.cmd().iter().any(|arg| {
            let path = Path::new(arg);
            path.starts_with(&paths.state_dir)
                || path.starts_with(&paths.cache_dir)
                || path.starts_with(&paths.config_dir)
        });
        if owned_executable || references_profile {
            return Ok(true);
        }
    }
    // Mount states are independent of the connection list, including deleted or
    // orphaned connections. Fail closed for unreadable records.
    if paths.state_dir.exists() {
        for entry in fs::read_dir(&paths.state_dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "json")
                && path.file_name().is_none_or(|n| n != "app-command.json")
            {
                let value: serde_json::Value =
                    crate::storage::read_json(&path).map_err(|e| e.to_string())?;
                if let Some(pid) = value.get("pid").and_then(|v| v.as_u64())
                    && pid <= u32::MAX as u64
                    && system
                        .process(sysinfo::Pid::from_u32(pid as u32))
                        .is_some_and(|process| {
                            value
                                .get("process_started_at")
                                .and_then(|v| v.as_u64())
                                .is_none_or(|started| started == process.start_time())
                        })
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
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
