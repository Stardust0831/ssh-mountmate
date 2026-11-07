//! Observations belong to one mounted session. UI error debouncing never makes stale data valid.
use mountmate_core::capacity::CapacityInfo;
use mountmate_core::transfer::{TransferFile, TransferSnapshot};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct FeedbackRequest {
    pub id: String,
    generation: u64,
}

#[derive(Default)]
pub struct MountFeedbacks {
    mounts: HashMap<String, MountFeedback>,
    next_generation: u64,
}

#[derive(Default)]
pub struct MountFeedback {
    generation: u64,
    pub transfers: TransferState,
    pub capacity: Option<CapacityInfo>,
    pub capacity_failed: bool,
}

impl MountFeedbacks {
    pub fn start_read(&mut self, id: &str) -> FeedbackRequest {
        let state = self.mounts.entry(id.to_owned()).or_insert_with(|| {
            self.next_generation = self.next_generation.wrapping_add(1);
            MountFeedback {
                generation: self.next_generation,
                ..MountFeedback::default()
            }
        });
        FeedbackRequest {
            id: id.to_owned(),
            generation: state.generation,
        }
    }

    pub fn accept(&mut self, request: &FeedbackRequest) -> Option<&mut MountFeedback> {
        self.mounts
            .get_mut(&request.id)
            .filter(|state| state.generation == request.generation)
    }

    pub fn forget(&mut self, id: &str) {
        self.mounts.remove(id);
    }

    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.mounts.retain(|id, _| keep(id));
    }

    pub fn snapshot(&self, id: &str) -> Option<&TransferSnapshot> {
        self.mounts.get(id)?.transfers.current()
    }

    pub fn transfer_error(&self, id: &str) -> Option<&String> {
        self.mounts.get(id)?.transfers.visible_error()
    }

    pub fn synced(&self, id: &str) -> bool {
        self.mounts
            .get(id)
            .is_some_and(|state| state.transfers.synced())
    }

    pub fn capacity(&self, id: &str) -> Option<&CapacityInfo> {
        self.mounts.get(id)?.capacity.as_ref()
    }

    pub fn capacity_failed(&self, id: &str) -> bool {
        self.mounts
            .get(id)
            .is_some_and(|state| state.capacity_failed)
    }
}

#[derive(Default)]
pub struct TransferState {
    snapshot: Option<TransferSnapshot>,
    history: TransferProgressHistory,
    error: Option<String>,
    failures: u8,
    synced_polls: u8,
}

impl TransferState {
    pub fn update(&mut self, snapshot: TransferSnapshot) -> (&TransferSnapshot, u8) {
        self.failures = 0;
        self.error = None;
        self.synced_polls = if snapshot.synced {
            self.synced_polls.saturating_add(1)
        } else {
            0
        };
        let snapshot = self
            .snapshot
            .insert(TransferProgressHistory::smooth(&mut self.history, snapshot));
        (snapshot, self.synced_polls)
    }

    /// Returns whether a new persistent error should be logged.
    pub fn fail(&mut self, error: String) -> bool {
        self.synced_polls = 0;
        self.failures = self.failures.saturating_add(1);
        let announce =
            self.failures == 3 || self.failures > 3 && self.error.as_ref() != Some(&error);
        self.error = Some(error);
        announce
    }

    pub fn current(&self) -> Option<&TransferSnapshot> {
        self.snapshot.as_ref().filter(|_| self.error.is_none())
    }

    pub fn visible_error(&self) -> Option<&String> {
        self.error.as_ref().filter(|_| self.failures >= 3)
    }

    pub fn synced(&self) -> bool {
        self.synced_polls >= 2 && self.current().is_some_and(|snapshot| snapshot.synced)
    }
}

#[derive(Debug, Default)]
pub struct TransferProgressHistory {
    active: bool,
    completed_bytes: u64,
    known_files: HashMap<String, (u64, u64)>,
}

impl TransferProgressHistory {
    pub fn smooth(
        history: &mut TransferProgressHistory,
        mut snapshot: TransferSnapshot,
    ) -> TransferSnapshot {
        let has_work = transfer_is_active(&snapshot);
        // A new queue after a confirmed idle period starts a fresh progress
        // session. This prevents bytes from unrelated transfers being mixed.
        if has_work && !history.active {
            *history = TransferProgressHistory::default();
            history.active = true;
        }

        if history.active {
            let current_keys: HashSet<String> =
                snapshot.files.iter().map(transfer_file_key).collect();

            // A file disappearing from a non-empty queue is rclone's normal
            // completion signal. Do not infer completion when the endpoint
            // returned no per-file details at all; that can be transient.
            if !snapshot.files.is_empty() || !has_work {
                let completed = history
                    .known_files
                    .keys()
                    .filter(|key| !current_keys.contains(*key))
                    .cloned()
                    .collect::<Vec<_>>();
                for key in completed {
                    if let Some((size, bytes)) = history.known_files.remove(&key) {
                        history.completed_bytes =
                            history.completed_bytes.saturating_add(size.max(bytes));
                    }
                }
            }

            for file in &snapshot.files {
                let key = transfer_file_key(file);
                let entry = history
                    .known_files
                    .entry(key)
                    .or_insert((file.size, file.bytes));
                entry.0 = entry.0.max(file.size);
                entry.1 = entry.1.max(file.bytes);
            }

            if history.completed_bytes > 0 {
                let current_queued_bytes = snapshot.queued_bytes;
                snapshot.queued_bytes = snapshot
                    .queued_bytes
                    .saturating_add(history.completed_bytes);
                snapshot.transferred_bytes = snapshot
                    .transferred_bytes
                    .saturating_add(history.completed_bytes);
                snapshot.percentage = if has_work && current_queued_bytes == 0 {
                    // rclone can briefly report upload counters without any
                    // per-file byte total. Keep the percentage indeterminate
                    // instead of presenting completed bytes as 100%.
                    0.0
                } else if snapshot.queued_bytes == 0 {
                    100.0
                } else {
                    (snapshot.transferred_bytes as f64 * 100.0 / snapshot.queued_bytes as f64)
                        .clamp(0.0, 100.0)
                };
            }

            if !has_work {
                history.active = false;
                history.known_files.clear();
            }
        }

        snapshot
    }
}

fn transfer_file_key(file: &TransferFile) -> String {
    let name = mountmate_core::transfer::normalized_transfer_name(&file.name);
    if !name.is_empty() {
        format!("name:{}", name)
    } else {
        format!("id:{}", file.id)
    }
}

pub fn transfer_is_active(snapshot: &TransferSnapshot) -> bool {
    snapshot.queued > 0 || snapshot.uploading > 0 || snapshot.errors > 0 || snapshot.out_of_space
}
