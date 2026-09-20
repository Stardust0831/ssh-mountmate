//! Best-effort maintenance of updater-owned files, independent of helper version.
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use fs2::FileExt;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::{application_data::validate_removal_tree, paths::AppPaths};

/// Shared by preparation and cleanup, including profiles using the legacy state
/// directory. Keep the file in place: unlinking a lock creates two lock domains.
pub(crate) fn lock_updates(paths: &AppPaths) -> Result<Arc<File>, String> {
    let path = paths.data_dir.join("update-maintenance.lock");
    crate::data_migration::validate_ancestors(&path).map_err(|e| e.to_string())?;
    fs::create_dir_all(&paths.data_dir).map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    FileExt::try_lock_exclusive(&file).map_err(|_| {
        "Update maintenance is in progress; retry shortly. 更新正在收尾，请稍后重试。".to_owned()
    })?;
    Ok(Arc::new(file))
}

#[derive(Debug, Default, Clone)]
pub struct CleanupReport {
    pub removed: usize,
    pub retained: usize,
    pub deferred: bool,
}

/// Called by the new GUI after writing its health marker. Older helpers already
/// remove that marker after committing, so the first upgrade into this version
/// is covered without changing the authenticated helper protocol.
pub fn cleanup_update_files(
    profiles: &[AppPaths],
    health_marker: Option<&Path>,
    timeout: Duration,
) -> Result<CleanupReport, String> {
    cleanup_with_probe(profiles, health_marker, timeout, || {
        updater_is_running(profiles)
    })
}

fn cleanup_with_probe(
    profiles: &[AppPaths],
    health_marker: Option<&Path>,
    timeout: Duration,
    mut updater_running: impl FnMut() -> bool,
) -> Result<CleanupReport, String> {
    let mut profiles = profiles.to_vec();
    profiles.sort_by(|a, b| a.data_dir.cmp(&b.data_dir));
    let mut locks = Vec::new();
    let mut locked_roots = Vec::new();
    for profile in &profiles {
        if !locked_roots.contains(&profile.data_dir) {
            locks.push(lock_updates(profile)?);
            locked_roots.push(profile.data_dir.clone());
        }
    }
    let deadline = Instant::now() + timeout;
    loop {
        // A health marker remaining after helper exit means commit failed. Do
        // not erase this evidence or attempt to recover/delete adjacent backups.
        let uncommitted = health_marker.is_some_and(|p| {
            !matches!(fs::symlink_metadata(p), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        });
        if !updater_running() && !uncommitted {
            break;
        }
        if Instant::now() >= deadline {
            return Ok(CleanupReport {
                deferred: true,
                ..CleanupReport::default()
            });
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let mut report = CleanupReport::default();
    let mut roots = Vec::new();
    for profile in &profiles {
        for (root, kind) in [
            (profile.update_cache_dir(), FileKind::Cache),
            (profile.update_helper_dir(), FileKind::Helper),
            (profile.update_state_dir(), FileKind::State),
        ] {
            if roots.contains(&root) {
                continue;
            }
            roots.push(root.clone());
            if crate::data_migration::validate_ancestors(&root).is_err() {
                report.retained += 1;
                continue;
            }
            let entries = match fs::read_dir(&root) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    report.retained += 1;
                    continue;
                }
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    report.retained += 1;
                    continue;
                };
                if !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| owned_name(name, kind))
                {
                    continue;
                }
                if remove_owned_entry(&entry.path()).is_ok() {
                    report.removed += 1;
                } else {
                    report.retained += 1;
                }
            }
        }
    }
    Ok(report)
}

pub(crate) fn updater_is_running(profiles: &[AppPaths]) -> bool {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );
    if system.processes().values().any(|process| {
        if matches!(
            process.status(),
            sysinfo::ProcessStatus::Dead | sysinfo::ProcessStatus::Zombie
        ) {
            return false;
        }
        if let Some(executable) = process
            .exe()
            .or_else(|| process.cmd().first().map(Path::new))
        {
            return profiles.iter().any(|profile| {
                crate::application_data::path_is_within(
                    executable,
                    &profile.update_helper_dir(),
                    cfg!(windows),
                )
            });
        }
        // If process identity is unreadable, err on the side of retaining files.
        process
            .name()
            .to_string_lossy()
            .starts_with("SSHMountMate-helper-")
    }) {
        return true;
    }
    // Older GUIs do not hold the maintenance lock while awaiting confirmation.
    // A plan whose original process still exists must be left for that GUI.
    profiles.iter().any(|profile| {
        let root = profile.update_state_dir();
        if crate::data_migration::validate_ancestors(&root).is_err() {
            return false;
        }
        let Ok(entries) = fs::read_dir(root) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let name = entry.file_name();
            if !name
                .to_str()
                .is_some_and(|s| s.starts_with("plan-") && owned_name(s, FileKind::State))
            {
                return false;
            }
            if crate::data_migration::validate_ancestors(&entry.path()).is_err() {
                return false;
            }
            let Ok(plan) =
                crate::storage::read_json::<crate::update_helper::UpdateHelperPlan>(&entry.path())
            else {
                // A corrupt plan cannot authorize an installation. With no
                // helper running it is a stale updater file.
                return false;
            };
            system
                .process(sysinfo::Pid::from_u32(plan.parent.pid))
                .is_some_and(|process| {
                    process.start_time() == plan.parent.started_at
                        && !matches!(
                            process.status(),
                            sysinfo::ProcessStatus::Dead | sysinfo::ProcessStatus::Zombie
                        )
                })
        })
    })
}

#[derive(Clone, Copy)]
enum FileKind {
    Cache,
    Helper,
    State,
}

fn hex_id(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn owned_name(name: &str, kind: FileKind) -> bool {
    match kind {
        FileKind::Cache => {
            if name
                .strip_prefix("payload-")
                .is_some_and(|id| hex_id(id, 16))
            {
                return true;
            }
            let name = name
                .strip_suffix(".part")
                .or_else(|| name.strip_suffix(".backup"))
                .unwrap_or(name);
            ["windows", "macos", "linux"].iter().any(|os| {
                ["x64", "arm64"]
                    .iter()
                    .any(|arch| name == format!("SSHMountMate-{os}-{arch}.zip"))
            })
        }
        FileKind::Helper => {
            let name = name.strip_suffix(".exe").unwrap_or(name);
            name.strip_prefix("SSHMountMate-helper-")
                .is_some_and(|id| hex_id(id, 16))
                || name
                    .strip_prefix(".SSHMountMate-helper-")
                    .is_some_and(|id| hex_id(id, 32))
        }
        FileKind::State => name.strip_suffix(".json").is_some_and(|stem| {
            stem.strip_prefix("plan-")
                .or_else(|| stem.strip_prefix("health-"))
                .is_some_and(|id| hex_id(id, 32))
        }),
    }
}

pub(crate) fn remove_owned_entry(path: &Path) -> Result<(), String> {
    validate_removal_tree(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    }
    .map_err(|e| e.to_string())
}

/// Downloads and extracted copies are redundant once the sibling payload has
/// been staged. Drop also handles failures while downloading/preparing.
pub(crate) struct DownloadScratch(pub Vec<PathBuf>);

impl Drop for DownloadScratch {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = remove_owned_entry(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(root: &Path) -> AppPaths {
        AppPaths {
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            data_dir: root.join("data"),
        }
    }

    fn write(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"fixture").unwrap();
    }

    fn leftovers(paths: &AppPaths) -> Vec<PathBuf> {
        vec![
            paths
                .update_cache_dir()
                .join("SSHMountMate-windows-x64.zip"),
            paths
                .update_cache_dir()
                .join("SSHMountMate-linux-arm64.zip.part"),
            paths
                .update_cache_dir()
                .join("SSHMountMate-macos-arm64.zip.backup"),
            paths
                .update_cache_dir()
                .join("payload-0123456789abcdef/SSHMountMate.exe"),
            paths
                .update_helper_dir()
                .join("SSHMountMate-helper-0123456789abcdef.exe"),
            paths
                .update_helper_dir()
                .join(".SSHMountMate-helper-0123456789abcdef0123456789abcdef"),
            paths
                .update_state_dir()
                .join("health-0123456789abcdef0123456789abcdef.json"),
            paths
                .update_state_dir()
                .join("plan-0123456789abcdef0123456789abcdef.json"),
        ]
    }

    #[test]
    fn cleans_current_and_legacy_updater_files_without_touching_user_data() {
        let temp = tempfile::tempdir().unwrap();
        let current = profile(&temp.path().join("current"));
        let mut legacy = profile(&temp.path().join("legacy"));
        legacy.data_dir = current.data_dir.clone();
        let profiles = [current.clone(), legacy.clone()];
        let stale: Vec<_> = profiles.iter().flat_map(leftovers).collect();
        let preserved = [
            current.config_dir.join("servers.json"),
            current.cache_dir.join("remote/vfs/queued-upload"),
            current.state_dir.join("mount.json"),
            legacy.cache_dir.join("remote/unuploaded"),
            current.data_dir.join("bin/rclone.exe"),
            current.update_cache_dir().join("personal.zip"),
            current
                .update_cache_dir()
                .join("payload-important/notes.txt"),
            current
                .update_helper_dir()
                .join("SSHMountMate-helper-custom.exe"),
            current.update_state_dir().join("notes.json"),
            temp.path().join("SSHMountMate-v0.6.10.exe"),
            temp.path().join(".SSHMountMate.ssh-mountmate-backup.exe"),
        ];
        for path in stale.iter().chain(&preserved) {
            write(path);
        }
        let report = cleanup_with_probe(&profiles, None, Duration::ZERO, || false).unwrap();
        assert_eq!(report.removed, 14); // helper directory is shared
        assert_eq!(report.retained, 0);
        assert!(!report.deferred);
        assert!(stale.iter().all(|p| !p.exists()));
        assert!(preserved.iter().all(|p| fs::read(p).unwrap() == b"fixture"));
    }

    #[test]
    fn old_helper_commit_and_exit_are_both_required_before_first_upgrade_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let paths = profile(temp.path());
        let stale = leftovers(&paths);
        for path in &stale {
            write(path);
        }
        let marker = paths.update_state_dir().join("health-active.json");
        write(&marker);
        let mut probes = 0;
        let report = cleanup_with_probe(
            std::slice::from_ref(&paths),
            Some(&marker),
            Duration::from_secs(2),
            || {
                probes += 1;
                assert!(stale.iter().all(|p| p.exists()));
                if probes == 1 {
                    fs::remove_file(&marker).unwrap();
                }
                probes < 3
            },
        )
        .unwrap();
        assert_eq!(probes, 3);
        assert!(!report.deferred);
        assert!(stale.iter().all(|p| !p.exists()));
    }

    #[test]
    fn unfinished_update_is_retained_and_retried_on_next_startup() {
        let temp = tempfile::tempdir().unwrap();
        let paths = profile(temp.path());
        let stale = leftovers(&paths);
        for path in &stale {
            write(path);
        }
        for (health, running) in [(None, true), (Some(stale[6].as_path()), false)] {
            let report =
                cleanup_with_probe(std::slice::from_ref(&paths), health, Duration::ZERO, || {
                    running
                })
                .unwrap();
            assert!(report.deferred);
            assert!(stale.iter().all(|p| p.exists()));
        }
        cleanup_with_probe(&[paths], None, Duration::ZERO, || false).unwrap();
        assert!(stale.iter().all(|p| !p.exists()));
    }

    #[test]
    fn pending_preparation_prevents_cleanup_across_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let paths = profile(temp.path());
        let lock = lock_updates(&paths).unwrap();
        let stale = leftovers(&paths);
        for path in &stale {
            write(path);
        }
        assert!(
            cleanup_with_probe(std::slice::from_ref(&paths), None, Duration::ZERO, || false)
                .is_err()
        );
        assert!(stale.iter().all(|p| p.exists()));
        drop(lock);
        cleanup_with_probe(&[paths], None, Duration::ZERO, || false).unwrap();
        assert!(stale.iter().all(|p| !p.exists()));
    }

    #[test]
    fn scratch_is_removed_after_success_or_failure_without_removing_staged_payload() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("archive.zip");
        let extracted = temp.path().join("extracted");
        let staged = temp.path().join("prepared.exe");
        write(&staged);
        for fails in [false, true] {
            let work = || -> Result<(), ()> {
                let _scratch = DownloadScratch(vec![archive.clone(), extracted.clone()]);
                write(&archive);
                write(&extracted.join("SSHMountMate.exe"));
                if fails {
                    return Err(());
                }
                Ok(())
            };
            assert_eq!(work().is_err(), fails);
            assert!(!archive.exists());
            assert!(!extracted.exists());
            assert!(staged.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_roots_and_payload_children_are_retained() {
        let temp = tempfile::tempdir().unwrap();
        let paths = profile(temp.path());
        let outside = temp.path().join("user-files");
        let keep = outside.join("SSHMountMate-linux-x64.zip");
        write(&keep);
        fs::create_dir_all(&paths.cache_dir).unwrap();
        std::os::unix::fs::symlink(&outside, paths.update_cache_dir()).unwrap();
        let helper = paths
            .update_helper_dir()
            .join("SSHMountMate-helper-0123456789abcdef");
        fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&keep, &helper).unwrap();
        let report =
            cleanup_with_probe(std::slice::from_ref(&paths), None, Duration::ZERO, || false)
                .unwrap();
        assert_eq!(report.retained, 2);
        assert_eq!(fs::read(&keep).unwrap(), b"fixture");
        fs::remove_file(paths.update_cache_dir()).unwrap();
        let payload = paths.update_cache_dir().join("payload-0123456789abcdef");
        fs::create_dir_all(&payload).unwrap();
        std::os::unix::fs::symlink(&outside, payload.join("linked")).unwrap();
        let report = cleanup_with_probe(&[paths], None, Duration::ZERO, || false).unwrap();
        assert_eq!(report.retained, 2);
        assert_eq!(fs::read(keep).unwrap(), b"fixture");
    }
}
