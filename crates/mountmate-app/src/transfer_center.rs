use iced::widget::{Space, column, container, progress_bar, row, text};
use iced::{Center, Element, Fill};
use mountmate_core::ServerConfig;
use mountmate_core::transfer::{TransferFile, TransferSnapshot};

use super::i18n::{Locale, TextKey};
use super::{Message, format_bytes, transfer_is_active, transfer_label};

#[derive(Debug, Default, PartialEq)]
pub(crate) struct TransferTotals {
    pub(crate) pending_files: usize,
    pub(crate) uploading: usize,
    pub(crate) queued: usize,
    pub(crate) errors: usize,
    pub(crate) out_of_space: bool,
    pub(crate) total_bytes: u64,
    pub(crate) transferred_bytes: u64,
    pub(crate) unknown_connections: usize,
    pub(crate) percentage: f64,
    pub(crate) progress_available: bool,
}

pub(crate) fn totals<'a>(
    snapshots: impl IntoIterator<Item = Option<&'a TransferSnapshot>>,
) -> TransferTotals {
    let mut totals = TransferTotals::default();
    let mut has_unknown_work = false;
    for snapshot in snapshots {
        let Some(snapshot) = snapshot else {
            totals.unknown_connections += 1;
            has_unknown_work = true;
            continue;
        };
        totals.pending_files += snapshot.queued.max(snapshot.uploading);
        totals.uploading += snapshot.uploading;
        totals.queued += snapshot.queued.saturating_sub(snapshot.uploading);
        totals.errors += snapshot.errors;
        totals.out_of_space |= snapshot.out_of_space;
        totals.total_bytes = totals.total_bytes.saturating_add(snapshot.queued_bytes);
        totals.transferred_bytes = totals
            .transferred_bytes
            .saturating_add(snapshot.transferred_bytes.min(snapshot.queued_bytes));
        if (snapshot.queued > 0 || snapshot.uploading > 0)
            && (snapshot.files.is_empty() || snapshot.queued_bytes == 0)
        {
            has_unknown_work = true;
        }
    }
    totals.progress_available = !totals.out_of_space
        && totals.unknown_connections == 0
        && !has_unknown_work
        && (totals.pending_files == 0 || totals.total_bytes > 0);
    totals.percentage = if totals.pending_files == 0
        && totals.errors == 0
        && totals.unknown_connections == 0
        && !totals.out_of_space
    {
        100.0
    } else if totals.progress_available && totals.total_bytes > 0 {
        (totals.transferred_bytes as f64 * 100.0 / totals.total_bytes as f64).clamp(0.0, 100.0)
    } else {
        0.0
    };
    totals
}

pub(crate) fn connection_view<'a>(
    server: &'a ServerConfig,
    snapshot: Option<&'a TransferSnapshot>,
    error: Option<&'a String>,
    locale: Locale,
) -> Element<'a, Message> {
    let mut content = column![text(server.display_name()).size(21)].spacing(7);
    if let Some(error) = error {
        content = content
            .push(text(locale.text(TextKey::TransferStateUnavailable)).size(14))
            .push(text(error).size(12));
    } else if let Some(snapshot) = snapshot {
        content = content.push(text(transfer_label(locale, snapshot)).size(14));
        if transfer_is_active(snapshot) {
            content = content.push(progress_bar(0.0..=100.0, snapshot.percentage as f32));
        }
        if snapshot.out_of_space {
            content = content.push(
                text(match locale {
                    Locale::English => "The local VFS cache is out of space",
                    Locale::Chinese => "本地 VFS 缓存空间不足",
                })
                .size(13),
            );
        }
        if snapshot.files.is_empty() && transfer_is_active(snapshot) {
            content = content.push(
                text(match locale {
                    Locale::English => {
                        "rclone reports pending work but has not exposed per-file details"
                    }
                    Locale::Chinese => "rclone 报告仍有待处理任务，但尚未提供逐文件详情",
                })
                .size(12),
            );
        }
        for file in &snapshot.files {
            content = content.push(file_view(file, locale));
        }
    } else {
        content = content.push(text(locale.text(TextKey::CheckingTransferState)).size(14));
    }
    container(content)
        .padding(14)
        .width(Fill)
        .style(container::rounded_box)
        .into()
}

fn file_view(file: &TransferFile, locale: Locale) -> Element<'_, Message> {
    let state = if file.uploading {
        match locale {
            Locale::English => "Uploading",
            Locale::Chinese => "上传中",
        }
    } else if file.tries > 0 {
        match locale {
            Locale::English => "Queued for retry",
            Locale::Chinese => "等待重试",
        }
    } else {
        match locale {
            Locale::English => "Queued",
            Locale::Chinese => "排队中",
        }
    };
    let amount = if file.size == 0 {
        match locale {
            Locale::English => {
                format!("{} uploaded, total size unknown", format_bytes(file.bytes))
            }
            Locale::Chinese => format!("已上传 {}，总大小未知", format_bytes(file.bytes)),
        }
    } else {
        match locale {
            Locale::English => format!(
                "{} of {}",
                format_bytes(file.bytes),
                format_bytes(file.size)
            ),
            Locale::Chinese => {
                format!("{} / {}", format_bytes(file.bytes), format_bytes(file.size))
            }
        }
    };
    let activity = if file.uploading {
        let eta = file
            .eta
            .map(|eta| format_eta(locale, eta))
            .unwrap_or_else(|| format_eta(locale, f64::NAN));
        format!("{}/s - {eta}", format_bytes(file.speed.max(0.0) as u64))
    } else {
        match locale {
            Locale::English => "Queued locally; waiting for write-back delay or upload slot".into(),
            Locale::Chinese => "已在本地排队，等待写回延迟或上传槽位".into(),
        }
    };
    let retries = if file.tries > 0 {
        match locale {
            Locale::English => format!(" - {} attempt(s)", file.tries),
            Locale::Chinese => format!(" - 已尝试 {} 次", file.tries),
        }
    } else {
        String::new()
    };
    container(
        column![
            row![
                text(&file.name).size(14).width(Fill),
                text(format!("{state}{retries}")).size(12),
            ]
            .spacing(10)
            .align_y(Center),
            progress_bar(0.0..=100.0, file.percentage as f32),
            row![
                text(amount).size(12),
                Space::new().width(Fill),
                text(activity).size(12)
            ],
        ]
        .spacing(5),
    )
    .padding([8, 0])
    .into()
}

fn format_eta(locale: Locale, seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return match locale {
            Locale::English => "ETA unknown".into(),
            Locale::Chinese => "剩余时间未知".into(),
        };
    }
    let seconds = seconds.round() as u64;
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        match locale {
            Locale::English => format!("ETA {hours}h {minutes}m"),
            Locale::Chinese => format!("预计剩余 {hours} 小时 {minutes} 分钟"),
        }
    } else if minutes > 0 {
        match locale {
            Locale::English => format!("ETA {minutes}m {seconds}s"),
            Locale::Chinese => format!("预计剩余 {minutes} 分 {seconds} 秒"),
        }
    } else {
        match locale {
            Locale::English => format!("ETA {seconds}s"),
            Locale::Chinese => format!("预计剩余 {seconds} 秒"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(size: u64, uploaded: u64) -> TransferSnapshot {
        TransferSnapshot {
            files: vec![TransferFile {
                id: Default::default(),
                name: "file.bin".into(),
                size,
                bytes: uploaded,
                percentage: uploaded as f64 * 100.0 / size as f64,
                speed: 10.0,
                eta: Some(6.0),
                uploading: true,
                tries: 0,
            }],
            queued: 1,
            uploading: 1,
            queued_bytes: size,
            transferred_bytes: uploaded,
            percentage: uploaded as f64 * 100.0 / size as f64,
            errors: 0,
            out_of_space: false,
            synced: false,
        }
    }

    #[test]
    fn totals_use_real_uploaded_bytes() {
        let first = snapshot(100, 25);
        let second = snapshot(300, 75);
        let totals = totals([Some(&first), Some(&second)]);

        assert_eq!(totals.pending_files, 2);
        assert_eq!(totals.uploading, 2);
        assert_eq!(totals.total_bytes, 400);
        assert_eq!(totals.transferred_bytes, 100);
        assert_eq!(totals.percentage, 25.0);
        assert!(totals.progress_available);
    }

    #[test]
    fn unknown_connection_prevents_false_overall_progress() {
        let known = snapshot(100, 100);
        let totals = totals([Some(&known), None]);

        assert_eq!(totals.unknown_connections, 1);
        assert_eq!(totals.percentage, 0.0);
        assert!(!totals.progress_available);
    }

    #[test]
    fn unknown_file_size_prevents_false_overall_progress() {
        let mut unknown = snapshot(1, 0);
        unknown.files[0].size = 0;
        unknown.queued_bytes = 0;
        let totals = totals([Some(&unknown)]);

        assert_eq!(totals.percentage, 0.0);
        assert!(!totals.progress_available);
    }

    #[test]
    fn empty_confirmed_snapshots_are_fully_synced() {
        let synced = TransferSnapshot {
            files: Vec::new(),
            queued: 0,
            uploading: 0,
            queued_bytes: 0,
            transferred_bytes: 0,
            percentage: 100.0,
            errors: 0,
            out_of_space: false,
            synced: true,
        };
        let totals = totals([Some(&synced)]);

        assert_eq!(totals.percentage, 100.0);
        assert!(totals.progress_available);
    }

    #[test]
    fn exhausted_cache_is_preserved_in_aggregate_state() {
        let exhausted = TransferSnapshot {
            files: Vec::new(),
            queued: 0,
            uploading: 0,
            queued_bytes: 0,
            transferred_bytes: 0,
            percentage: 0.0,
            errors: 0,
            out_of_space: true,
            synced: false,
        };
        let totals = totals([Some(&exhausted)]);

        assert!(totals.out_of_space);
        assert_eq!(totals.percentage, 0.0);
        assert!(!totals.progress_available);
    }

    #[test]
    fn eta_format_is_compact_and_stable() {
        assert_eq!(format_eta(Locale::English, 42.0), "ETA 42s");
        assert_eq!(format_eta(Locale::English, 125.0), "ETA 2m 5s");
        assert_eq!(format_eta(Locale::English, 3_661.0), "ETA 1h 1m");
        assert_eq!(format_eta(Locale::English, f64::NAN), "ETA unknown");
        assert_eq!(format_eta(Locale::Chinese, 42.0), "预计剩余 42 秒");
    }
}
