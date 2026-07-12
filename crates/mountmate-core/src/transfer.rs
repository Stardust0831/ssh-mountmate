use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct QueueResponse {
    #[serde(default)]
    pub queue: Vec<QueueItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct QueueItem {
    #[serde(default)]
    pub id: serde_json::Value,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub uploading: bool,
    #[serde(default)]
    pub tries: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTransfer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub percentage: f64,
    #[serde(default)]
    pub speed: f64,
    #[serde(default)]
    pub speed_avg: f64,
    #[serde(default)]
    pub eta: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CoreStatsResponse {
    #[serde(default)]
    pub transferring: Vec<ActiveTransfer>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskCacheStats {
    #[serde(default)]
    pub uploads_queued: usize,
    #[serde(default)]
    pub uploads_in_progress: usize,
    #[serde(default)]
    pub errored_files: usize,
    #[serde(default)]
    pub out_of_space: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VfsStatsResponse {
    #[serde(default)]
    pub disk_cache: DiskCacheStats,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferFile {
    pub id: serde_json::Value,
    pub name: String,
    pub size: u64,
    pub bytes: u64,
    pub percentage: f64,
    pub speed: f64,
    pub eta: Option<f64>,
    pub uploading: bool,
    pub tries: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferSnapshot {
    pub files: Vec<TransferFile>,
    pub queued: usize,
    pub uploading: usize,
    pub queued_bytes: u64,
    pub transferred_bytes: u64,
    pub percentage: f64,
    pub errors: usize,
    pub out_of_space: bool,
    pub synced: bool,
}

pub fn normalized_transfer_name(value: &str) -> String {
    value.replace('\\', "/").trim_matches('/').to_lowercase()
}

pub fn transfer_matches(queued: &str, active: &str) -> bool {
    let queued = normalized_transfer_name(queued);
    let active = normalized_transfer_name(active);
    !queued.is_empty()
        && !active.is_empty()
        && (queued == active
            || queued.ends_with(&format!("/{active}"))
            || active.ends_with(&format!("/{queued}")))
}

pub fn build_transfer_snapshot(
    queue: QueueResponse,
    vfs: VfsStatsResponse,
    core: CoreStatsResponse,
) -> TransferSnapshot {
    let mut files = Vec::with_capacity(queue.queue.len());
    let mut queued_bytes = 0_u64;
    let mut transferred_bytes = 0_u64;
    for item in queue.queue {
        queued_bytes = queued_bytes.saturating_add(item.size);
        let active = core
            .transferring
            .iter()
            .find(|candidate| transfer_matches(&item.name, &candidate.name));
        let active_bytes = active.map_or(0, |transfer| transfer.bytes);
        let uploaded = if item.size == 0 {
            active_bytes
        } else {
            active_bytes.min(item.size)
        };
        transferred_bytes = transferred_bytes.saturating_add(uploaded);
        let percentage = active.map_or_else(
            || {
                if item.size == 0 {
                    0.0
                } else {
                    uploaded as f64 * 100.0 / item.size as f64
                }
            },
            |transfer| {
                if transfer.percentage > 0.0 {
                    transfer.percentage
                } else if item.size == 0 {
                    0.0
                } else {
                    uploaded as f64 * 100.0 / item.size as f64
                }
            },
        );
        files.push(TransferFile {
            id: item.id,
            name: item.name,
            size: item.size,
            bytes: uploaded,
            percentage: percentage.clamp(0.0, 100.0),
            speed: active.map_or(0.0, |transfer| transfer.speed_avg.max(transfer.speed)),
            eta: active.and_then(|transfer| transfer.eta),
            uploading: item.uploading || active.is_some(),
            tries: item.tries,
        });
    }
    let queued = files.len().max(vfs.disk_cache.uploads_queued);
    let uploading = files
        .iter()
        .filter(|file| file.uploading)
        .count()
        .max(vfs.disk_cache.uploads_in_progress);
    let errors = vfs.disk_cache.errored_files;
    TransferSnapshot {
        files,
        queued,
        uploading,
        queued_bytes,
        transferred_bytes,
        percentage: if queued_bytes == 0 {
            100.0
        } else {
            (transferred_bytes as f64 * 100.0 / queued_bytes as f64).clamp(0.0, 100.0)
        },
        errors,
        out_of_space: vfs.disk_cache.out_of_space,
        synced: queued == 0 && uploading == 0 && errors == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_combines_queue_and_real_remote_bytes() {
        let snapshot = build_transfer_snapshot(
            QueueResponse {
                queue: vec![QueueItem {
                    name: "folder/file.bin".into(),
                    size: 100,
                    uploading: true,
                    ..QueueItem::default()
                }],
            },
            VfsStatsResponse::default(),
            CoreStatsResponse {
                transferring: vec![ActiveTransfer {
                    name: "file.bin".into(),
                    bytes: 40,
                    speed_avg: 12.5,
                    ..ActiveTransfer::default()
                }],
            },
        );
        assert_eq!(snapshot.queued, 1);
        assert_eq!(snapshot.transferred_bytes, 40);
        assert_eq!(snapshot.percentage, 40.0);
        assert!(!snapshot.synced);
    }

    #[test]
    fn empty_confirmed_queue_is_synced() {
        let snapshot = build_transfer_snapshot(
            QueueResponse::default(),
            VfsStatsResponse::default(),
            CoreStatsResponse::default(),
        );
        assert!(snapshot.synced);
        assert_eq!(snapshot.percentage, 100.0);
    }

    #[test]
    fn disk_cache_queue_prevents_false_completion() {
        let snapshot = build_transfer_snapshot(
            QueueResponse::default(),
            VfsStatsResponse {
                disk_cache: DiskCacheStats {
                    uploads_queued: 1,
                    ..DiskCacheStats::default()
                },
            },
            CoreStatsResponse::default(),
        );
        assert!(!snapshot.synced);
        assert_eq!(snapshot.queued, 1);
    }
}
