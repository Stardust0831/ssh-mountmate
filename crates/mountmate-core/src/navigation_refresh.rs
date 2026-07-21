//! Safety and scheduling primitives for passive Explorer cache refresh.
//!
//! This module is deliberately independent from the Windows observer.  It
//! makes the path and queue policy testable on every platform and gives the
//! tray app a small, nonblocking work coordinator.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::model::MountState;
use crate::rclone::{normalize_explorer_refresh_path, normalize_refresh_relative_path};

pub const REFRESH_DEDUPE_WINDOW: Duration = Duration::from_secs(5);
pub const MAX_PENDING_REFRESHES: usize = 32;
pub const MAX_RUNNING_REFRESHES: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationEvent {
    pub window_id: u64,
    pub target: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountIdentity {
    pub server_id: String,
    pub pid: u32,
    pub process_started_at: Option<u64>,
}

impl MountIdentity {
    pub fn from_state(state: &MountState) -> Self {
        Self {
            server_id: state.server_id.clone(),
            pid: state.pid,
            process_started_at: state.process_started_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshJob {
    pub token: u64,
    pub window_id: u64,
    pub target: PathBuf,
    pub relative_dir: String,
    pub identity: MountIdentity,
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueResult {
    Queued,
    Deduplicated,
    DroppedOldest,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NavigationPathError {
    #[error("path contains a NUL or control character")]
    ControlCharacter,
    #[error("path traversal is not allowed")]
    Traversal,
    #[error("device, alternate data stream, or shell namespace path is not allowed")]
    DeviceOrNamespace,
    #[error("path is outside the mounted directory")]
    OutsideMount,
}

/// Resolve an Explorer path to the VFS directory that should be refreshed.
/// A regular file is mapped to its parent when metadata can identify it.
pub fn validated_relative_dir(
    requested: &Path,
    mountpoint: &Path,
    windows: bool,
) -> Option<String> {
    validated_relative_dir_result(requested, mountpoint, windows).ok()
}

pub fn validated_relative_dir_result(
    requested: &Path,
    mountpoint: &Path,
    windows: bool,
) -> Result<String, NavigationPathError> {
    let raw_requested = requested.to_string_lossy();
    let raw_mountpoint = mountpoint.to_string_lossy();
    validate_raw_path(&raw_requested, windows)?;
    validate_raw_path(&raw_mountpoint, windows)?;

    let requested = normalize_explorer_refresh_path(&raw_requested, windows);
    let mountpoint = normalize_explorer_refresh_path(&raw_mountpoint, windows);
    let target = Path::new(&requested);
    let mount = Path::new(&mountpoint);
    let target = if std::fs::metadata(target).is_ok_and(|metadata| metadata.is_file()) {
        target.parent().unwrap_or(target)
    } else {
        target
    };
    let requested = target.to_string_lossy();
    let mountpoint = mount.to_string_lossy();
    let requested_normalized = lexical_path(&requested, windows);
    let mountpoint_normalized = lexical_path(&mountpoint, windows);
    let equal = if windows {
        requested_normalized.eq_ignore_ascii_case(&mountpoint_normalized)
    } else {
        requested_normalized == mountpoint_normalized
    };
    if equal {
        return Ok(String::new());
    }
    let prefix = format!("{mountpoint_normalized}/");
    let relative = if windows {
        requested_normalized.get(prefix.len()..).filter(|_| {
            requested_normalized
                .get(..prefix.len())
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&prefix))
        })
    } else {
        requested_normalized.strip_prefix(&prefix)
    }
    .ok_or(NavigationPathError::OutsideMount)?;
    if relative.is_empty() {
        return Ok(String::new());
    }
    if relative.split('/').any(|component| component == "..") {
        return Err(NavigationPathError::Traversal);
    }
    if relative.split('/').any(|component| component.contains(':')) {
        return Err(NavigationPathError::DeviceOrNamespace);
    }
    Ok(normalize_refresh_relative_path(relative))
}

fn validate_raw_path(value: &str, windows: bool) -> Result<(), NavigationPathError> {
    if value.chars().any(|ch| ch == '\0' || ch.is_control()) {
        return Err(NavigationPathError::ControlCharacter);
    }
    let normalized = value.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    if lower.starts_with("shell:")
        || lower.starts_with("::{")
        || lower.starts_with("::")
        || lower.starts_with("//./")
        || lower.starts_with("//?/")
        || lower.starts_with("/device/")
        || (windows && lower.starts_with("//"))
    {
        return Err(NavigationPathError::DeviceOrNamespace);
    }
    if normalized.split('/').any(|component| component == "..") {
        return Err(NavigationPathError::Traversal);
    }
    if windows && normalized.split('/').any(is_reserved_windows_component) {
        return Err(NavigationPathError::DeviceOrNamespace);
    }
    Ok(())
}

fn is_reserved_windows_component(component: &str) -> bool {
    let component = component.trim_end_matches([' ', '.']);
    let stem = component.split('.').next().unwrap_or_default();
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"))
        .is_some_and(|number| matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

fn lexical_path(value: &str, windows: bool) -> String {
    let mut normalized = value.replace('\\', "/");
    while normalized.ends_with('/') && normalized.len() > 1 {
        normalized.pop();
    }
    if windows {
        normalized.make_ascii_lowercase();
    }
    normalized
}

#[derive(Debug, Default)]
pub struct RefreshScheduler {
    pending: VecDeque<RefreshJob>,
    running: HashMap<u64, RefreshJob>,
    running_mounts: HashSet<String>,
    last_enqueued: HashMap<String, Instant>,
    next_token: u64,
}

impl RefreshScheduler {
    pub fn new() -> Self {
        Self {
            next_token: 1,
            ..Self::default()
        }
    }

    pub fn enqueue(
        &mut self,
        event: NavigationEvent,
        relative_dir: String,
        identity: MountIdentity,
        now: Instant,
    ) -> EnqueueResult {
        self.last_enqueued
            .retain(|_, last| now.saturating_duration_since(*last) < REFRESH_DEDUPE_WINDOW);
        let key = canonical_key(&event.target);
        if self
            .last_enqueued
            .get(&key)
            .is_some_and(|last| now.saturating_duration_since(*last) < REFRESH_DEDUPE_WINDOW)
            || self.pending.iter().any(|job| job.key == key)
            || self.running.values().any(|job| job.key == key)
        {
            return EnqueueResult::Deduplicated;
        }
        let job = RefreshJob {
            token: self.next_token,
            window_id: event.window_id,
            target: event.target,
            relative_dir,
            identity,
            key: key.clone(),
        };
        self.next_token = self.next_token.wrapping_add(1).max(1);
        let dropped = if self.pending.len() >= MAX_PENDING_REFRESHES {
            if let Some(dropped) = self.pending.pop_front() {
                self.last_enqueued.remove(&dropped.key);
            }
            true
        } else {
            false
        };
        self.last_enqueued.insert(key, now);
        self.pending.push_back(job);
        if dropped {
            EnqueueResult::DroppedOldest
        } else {
            EnqueueResult::Queued
        }
    }

    pub fn take_ready(&mut self) -> Option<RefreshJob> {
        if self.running.len() >= MAX_RUNNING_REFRESHES {
            return None;
        }
        let index = self
            .pending
            .iter()
            .position(|job| !self.running_mounts.contains(&job.identity.server_id))?;
        let job = self.pending.remove(index)?;
        self.running_mounts.insert(job.identity.server_id.clone());
        self.running.insert(job.token, job.clone());
        Some(job)
    }

    pub fn finish(&mut self, token: u64) -> Option<RefreshJob> {
        let job = self.running.remove(&token)?;
        self.running_mounts.remove(&job.identity.server_id);
        Some(job)
    }

    pub fn cancel_stale(&mut self, current: &HashMap<String, MountIdentity>) {
        self.pending.retain(|job| {
            current
                .get(&job.identity.server_id)
                .is_some_and(|identity| identity == &job.identity)
        });
    }

    pub fn is_current(&self, job: &RefreshJob, current: &HashMap<String, MountIdentity>) -> bool {
        current
            .get(&job.identity.server_id)
            .is_some_and(|identity| identity == &job.identity)
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn running_len(&self) -> usize {
        self.running.len()
    }
}

fn canonical_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn identity(id: &str, pid: u32) -> MountIdentity {
        MountIdentity {
            server_id: id.into(),
            pid,
            process_started_at: Some(1),
        }
    }

    fn event(path: &str) -> NavigationEvent {
        NavigationEvent {
            window_id: 1,
            target: PathBuf::from(path),
        }
    }

    #[test]
    fn scheduler_deduplicates_paths_for_five_seconds() {
        let now = Instant::now();
        let mut scheduler = RefreshScheduler::new();
        assert_eq!(
            scheduler.enqueue(event("/mnt/a"), "".into(), identity("a", 1), now),
            EnqueueResult::Queued
        );
        assert_eq!(
            scheduler.enqueue(
                event("/mnt/a"),
                "".into(),
                identity("a", 1),
                now + Duration::from_secs(4)
            ),
            EnqueueResult::Deduplicated
        );
        assert_eq!(
            scheduler.enqueue(
                event("/mnt/a"),
                "".into(),
                identity("a", 1),
                now + REFRESH_DEDUPE_WINDOW
            ),
            EnqueueResult::Deduplicated
        );
        let job = scheduler.take_ready().unwrap();
        scheduler.finish(job.token);
        assert_eq!(
            scheduler.enqueue(
                event("/mnt/a"),
                "".into(),
                identity("a", 1),
                now + REFRESH_DEDUPE_WINDOW
            ),
            EnqueueResult::Queued
        );
    }

    #[test]
    fn scheduler_bounds_pending_and_limits_global_and_mount_concurrency() {
        let now = Instant::now();
        let mut scheduler = RefreshScheduler::new();
        for index in 0..(MAX_PENDING_REFRESHES + 4) {
            let path = format!("/mnt/{index}");
            scheduler.enqueue(
                event(&path),
                "".into(),
                identity(&format!("m{index}"), index as u32),
                now,
            );
        }
        assert_eq!(scheduler.pending_len(), MAX_PENDING_REFRESHES);
        assert!(scheduler.take_ready().is_some());
        assert!(scheduler.take_ready().is_some());
        assert!(scheduler.take_ready().is_none());
    }

    #[test]
    fn scheduler_allows_one_running_job_per_mount() {
        let now = Instant::now();
        let mut scheduler = RefreshScheduler::new();
        scheduler.enqueue(event("/mnt/a/one"), "one".into(), identity("a", 1), now);
        scheduler.enqueue(event("/mnt/a/two"), "two".into(), identity("a", 1), now);
        let first = scheduler.take_ready().unwrap();
        assert!(scheduler.take_ready().is_none());
        scheduler.finish(first.token);
        assert!(scheduler.take_ready().is_some());
    }

    #[test]
    fn path_validation_rejects_traversal_ads_devices_and_sibling_collisions() {
        let mount = Path::new("Y:\\Mount");
        assert_eq!(
            validated_relative_dir_result(Path::new("Y:\\Mount\\folder"), mount, true).unwrap(),
            "folder"
        );
        assert!(matches!(
            validated_relative_dir_result(Path::new("Y:\\Mount\\..\\other"), mount, true),
            Err(NavigationPathError::Traversal)
        ));
        assert!(matches!(
            validated_relative_dir_result(Path::new("Y:\\Mount\\file:stream"), mount, true),
            Err(NavigationPathError::DeviceOrNamespace)
        ));
        assert!(matches!(
            validated_relative_dir_result(Path::new("Y:\\Mount2"), mount, true),
            Err(NavigationPathError::OutsideMount)
        ));
        for path in [
            r"\\server\share\folder",
            r"\\?\Y:\Mount\folder",
            r"Y:\Mount\NUL",
            r"Y:\Mount\con.txt",
            r"Y:\Mount\COM1 ",
            r"Y:\Mount\lpt9.log",
        ] {
            assert!(matches!(
                validated_relative_dir_result(Path::new(path), mount, true),
                Err(NavigationPathError::DeviceOrNamespace)
            ));
        }
    }

    #[test]
    fn stale_mount_identity_drops_pending_requests() {
        let now = Instant::now();
        let mut scheduler = RefreshScheduler::new();
        scheduler.enqueue(event("/mnt/a"), "".into(), identity("a", 1), now);
        let mut current = HashMap::new();
        current.insert("a".into(), identity("a", 2));
        scheduler.cancel_stale(&current);
        assert_eq!(scheduler.pending_len(), 0);
    }

    #[test]
    fn scheduler_prunes_expired_dedupe_history() {
        let now = Instant::now();
        let mut scheduler = RefreshScheduler::new();
        scheduler.enqueue(event("/mnt/old"), "".into(), identity("old", 1), now);
        let job = scheduler.take_ready().unwrap();
        scheduler.finish(job.token);
        assert_eq!(scheduler.last_enqueued.len(), 1);

        scheduler.enqueue(
            event("/mnt/new"),
            "".into(),
            identity("new", 2),
            now + REFRESH_DEDUPE_WINDOW,
        );
        assert_eq!(scheduler.last_enqueued.len(), 1);
        assert!(scheduler.last_enqueued.contains_key("/mnt/new"));
    }
}
