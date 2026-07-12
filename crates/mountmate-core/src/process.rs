use std::path::Path;

use crate::MountState;

pub fn normalize_command(value: &str) -> String {
    value.replace('\\', "/").to_lowercase()
}

pub fn command_looks_like_rclone_mount(command: &str) -> bool {
    let command = normalize_command(command);
    command.contains("rclone") && command.split_whitespace().any(|part| part == "mount")
}

pub fn command_matches_state(command: &str, state: &MountState, require_log: bool) -> bool {
    let command = normalize_command(command);
    if !command_looks_like_rclone_mount(&command) {
        return false;
    }
    let remote = normalize_command(&state.remote);
    let mountpoint = normalize_command(&state.mountpoint.to_string_lossy());
    let log = normalize_command(&state.log.to_string_lossy());
    command.contains(&remote)
        && command.contains(mountpoint.trim_end_matches('/'))
        && (!require_log || command.contains(&log))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountStatus {
    Mounted,
    Unmounted,
    Starting,
    Stale,
}

pub fn status_from_evidence(
    state: Option<&MountState>,
    process_running: bool,
    command: Option<&str>,
    mountpoint_ready: bool,
    rc_verified: bool,
) -> MountStatus {
    let Some(state) = state else {
        return MountStatus::Unmounted;
    };
    if !process_running {
        return MountStatus::Stale;
    }
    if let Some(command) = command
        && !command_matches_state(command, state, false)
    {
        return MountStatus::Stale;
    }
    if rc_verified || mountpoint_ready {
        MountStatus::Mounted
    } else {
        MountStatus::Starting
    }
}

pub fn path_matches_command(command: &str, path: &Path) -> bool {
    let command = normalize_command(command);
    let path = normalize_command(&path.to_string_lossy());
    command.contains(path.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn state() -> MountState {
        MountState {
            pid: 42,
            server_id: "alpha".into(),
            remote: "alpha:folder".into(),
            mountpoint: PathBuf::from("R:"),
            log: PathBuf::from("C:/State/alpha.log"),
            rc_addr: "127.0.0.1:1234".into(),
        }
    }

    #[test]
    fn strict_process_match_requires_mount_identity() {
        let state = state();
        let command = "rclone.exe --rc mount alpha:folder R: --log-file C:/State/alpha.log";
        assert!(command_matches_state(command, &state, true));
        assert!(!command_matches_state(
            "rclone.exe mount other: R: --log-file C:/State/alpha.log",
            &state,
            true
        ));
    }

    #[test]
    fn pid_reuse_is_stale_and_never_reported_as_mounted() {
        let state = state();
        assert_eq!(
            status_from_evidence(
                Some(&state),
                true,
                Some("unrelated.exe --serve"),
                true,
                false
            ),
            MountStatus::Stale
        );
    }

    #[test]
    fn rc_or_ready_mountpoint_can_confirm_a_matching_process() {
        let state = state();
        let command = "rclone mount alpha:folder R:";
        assert_eq!(
            status_from_evidence(Some(&state), true, Some(command), false, true),
            MountStatus::Mounted
        );
        assert_eq!(
            status_from_evidence(Some(&state), true, Some(command), true, false),
            MountStatus::Mounted
        );
    }
}
