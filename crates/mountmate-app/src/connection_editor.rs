//! Connection editor state and shared remote-path editing for primary and additional mappings.
use crate::sai_setup;
use mountmate_core::connection::{ConnectionDraft, ImportAction, SshImportPlan};

#[derive(Default)]
pub struct ConnectionEditor {
    pub draft: Option<ConnectionDraft>,
    pub sai: sai_setup::State,
    pub tags_input: String,
    pub custom_mountpoint: String,
    pub drives: Vec<char>,
    pub preflight: MountpointPreflight,
    pub preflight_queue: MountpointPreflightQueue,
    pub import_loading: bool,
    pub import_plan: Option<SshImportPlan>,
    pub import_actions: Vec<ImportAction>,
}

#[derive(Debug, Clone, Copy)]
pub enum MappingRow {
    Primary,
    Additional(usize),
}

#[derive(Debug, Clone)]
pub enum RemotePathChange {
    Base(String),
    Suffix(String),
}

impl ConnectionEditor {
    pub fn change_remote_path(&mut self, row: MappingRow, change: RemotePathChange) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        let path = match row {
            MappingRow::Primary => &mut draft.remote_path,
            MappingRow::Additional(index) => {
                let Some(mapping) = draft.mounts.get_mut(index) else {
                    return;
                };
                &mut mapping.remote_path
            }
        };
        let (base, suffix) = split_remote_path(path);
        *path = match change {
            RemotePathChange::Base(base) => compose_remote_path(&base, &suffix),
            RemotePathChange::Suffix(suffix) => compose_remote_path(&base, &suffix),
        };
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MountpointPreflight {
    #[default]
    NotRequired,
    Checking(String),
    Valid(String),
    Invalid {
        value: String,
        error: String,
    },
}

impl MountpointPreflight {
    pub fn allows_save(&self) -> bool {
        matches!(self, Self::NotRequired | Self::Valid(_))
    }
}

#[derive(Default)]
pub struct MountpointPreflightQueue {
    pub generation: u64,
    in_flight: Option<u64>,
    pending: Option<PendingMountpointPreflight>,
}

pub struct PendingMountpointPreflight {
    pub generation: u64,
    pub value: String,
    ready: bool,
}

impl MountpointPreflightQueue {
    pub fn replace(&mut self, value: Option<String>) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.pending = value.map(|value| PendingMountpointPreflight {
            generation: self.generation,
            value,
            ready: false,
        });
        // A blocking filesystem probe cannot be cancelled. Keep its slot
        // occupied across edits and cancellation until it actually finishes.
        self.generation
    }

    pub fn mark_ready(&mut self, generation: u64) {
        if let Some(pending) = &mut self.pending
            && pending.generation == generation
        {
            pending.ready = true;
        }
    }

    pub fn take_ready(&mut self) -> Option<PendingMountpointPreflight> {
        if self.in_flight.is_some() || !self.pending.as_ref().is_some_and(|pending| pending.ready) {
            return None;
        }
        let pending = self.pending.take()?;
        self.in_flight = Some(pending.generation);
        Some(pending)
    }

    pub fn finish(&mut self, generation: u64) {
        if self.in_flight == Some(generation) {
            self.in_flight = None;
        }
    }
}

pub fn mountpoint_preflight_result_is_current(
    result_generation: u64,
    current_generation: u64,
    result_value: &str,
    current_value: Option<&str>,
) -> bool {
    result_generation == current_generation && current_value == Some(result_value)
}

pub fn split_remote_path(value: &str) -> (String, String) {
    // Keep the editable suffix intact. Normalizing here would erase a newly
    // typed separator or space before the user can enter the next component.
    if let Some(suffix) = value.strip_prefix('/') {
        ("/".into(), suffix.into())
    } else {
        ("$HOME".into(), value.into())
    }
}

pub fn compose_remote_path(base: &str, suffix: &str) -> String {
    // ConnectionDraft::validate normalizes the completed path when saving.
    if base == "/" {
        format!("/{suffix}")
    } else {
        suffix.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mountpoint_preflight_coalesces_typing_until_the_latest_debounce_finishes() {
        let mut queue = MountpointPreflightQueue::default();
        let first = queue.replace(Some("/mount/a".into()));
        let latest = queue.replace(Some("/mount/abc".into()));
        queue.mark_ready(first);
        assert!(queue.take_ready().is_none());
        queue.mark_ready(latest);
        let request = queue.take_ready().unwrap();
        assert_eq!(request.value, "/mount/abc");
        assert_eq!(request.generation, latest);
        assert!(queue.take_ready().is_none());
    }
    #[test]
    fn mountpoint_preflight_limits_slow_probes_and_discards_intermediate_paths() {
        let mut queue = MountpointPreflightQueue::default();
        let slow = queue.replace(Some("/slow-network/mount".into()));
        queue.mark_ready(slow);
        assert!(queue.take_ready().is_some());
        let intermediate = queue.replace(Some("/local/m".into()));
        queue.mark_ready(intermediate);
        assert!(queue.take_ready().is_none());
        let latest = queue.replace(Some("/local/mount".into()));
        queue.mark_ready(latest);
        assert!(queue.take_ready().is_none());

        queue.finish(slow);
        let request = queue.take_ready().unwrap();
        assert_eq!(request.value, "/local/mount");
        assert_eq!(request.generation, latest);
        // A delayed or duplicate completion must not release a newer probe.
        queue.finish(slow);
        assert_eq!(queue.in_flight, Some(latest));
    }
    #[test]
    fn mountpoint_preflight_cancel_and_reopen_same_path_preserves_single_worker() {
        let mut queue = MountpointPreflightQueue::default();
        let old = queue.replace(Some("/mount".into()));
        queue.mark_ready(old);
        assert!(queue.take_ready().is_some());

        // Cancel, clearing the field, and choosing Auto all invalidate pending
        // work without freeing the still-running filesystem probe's slot.
        queue.replace(None);
        queue.mark_ready(old);
        assert!(queue.take_ready().is_none());
        queue.replace(Some("/different".into()));
        let reopened = queue.replace(Some("/mount".into()));
        queue.mark_ready(reopened);
        assert!(queue.take_ready().is_none());
        assert!(!mountpoint_preflight_result_is_current(
            old,
            queue.generation,
            "/mount",
            Some("/mount"),
        ));
        queue.finish(old);
        let request = queue.take_ready().unwrap();
        assert_eq!(request.generation, reopened);
        assert_eq!(request.value, "/mount");
    }
}
