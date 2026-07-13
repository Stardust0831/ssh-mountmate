use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::Deserialize;
use thiserror::Error;
use wait_timeout::ChildExt;

use crate::{AuthMethod, MountState, ServerConfig};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const LUSTRE_CAPACITY_SCRIPT: &str = r#"set -eu
target=${1:-.}
if [ -z "$target" ]; then target=.; fi
if ! command -v lfs >/dev/null 2>&1; then exit 0; fi
if [ -d "$target" ]; then
  resolved=$(cd "$target" 2>/dev/null && pwd -P) || exit 0
else
  resolved=$(readlink -f -- "$target" 2>/dev/null || printf '%s' "$target")
fi
df_out=$(df -P -T "$resolved" 2>/dev/null | awk 'NR==2 {print $2 "\t" $7}')
fstype=${df_out%%	*}
mountpoint=${df_out#*	}
if [ "$fstype" != "lustre" ] || [ -z "$mountpoint" ]; then exit 0; fi
project_out=$(lfs project -d "$resolved" 2>/dev/null || true)
project_id=$(printf '%s\n' "$project_out" | awk 'NF >= 3 && $1 ~ /^[0-9]+$/ {print $1; exit}')
if [ -z "$project_id" ]; then exit 0; fi
quota_out=$(lfs quota -p "$project_id" "$resolved" 2>/dev/null || true)
if ! printf '%s\n' "$quota_out" | awk 'NF >= 4 && $2 ~ /^[0-9]+$/ {found=1} END {exit !found}'; then
  quota_out=$(lfs quota -p "$project_id" "$mountpoint" 2>/dev/null || true)
fi
printf '%s\n' "$quota_out"
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacitySource {
    LocalMountpoint,
    LustreProjectQuota,
    RcloneAbout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityInfo {
    pub used: u64,
    pub total: u64,
    pub percent: u8,
    pub source: CapacitySource,
}

#[derive(Debug, Error)]
pub enum CapacityError {
    #[error("capacity I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("capacity command timed out")]
    Timeout,
    #[error("capacity command failed: {0}")]
    Command(String),
    #[error("capacity response was invalid: {0}")]
    InvalidResponse(String),
}

#[derive(Debug, Default, Deserialize)]
struct RcloneAbout {
    total: Option<u64>,
    used: Option<u64>,
    free: Option<u64>,
}

pub fn mounted_capacity(
    server: &ServerConfig,
    state: &MountState,
    rclone_config: &Path,
) -> Result<Option<CapacityInfo>, CapacityError> {
    if server.source == "sai_cluster"
        && let Some(capacity) = lustre_project_capacity(server)?
    {
        return Ok(Some(capacity));
    }
    if let Some(capacity) = local_mount_capacity(&state.mountpoint) {
        return Ok(Some(capacity));
    }
    if let Some(capacity) = lustre_project_capacity(server)? {
        return Ok(Some(capacity));
    }
    rclone_about_capacity(&state.rclone, rclone_config, &state.remote)
}

pub fn local_mount_capacity(mountpoint: &Path) -> Option<CapacityInfo> {
    let total = fs2::total_space(mountpoint).ok()?;
    let available = fs2::available_space(mountpoint).ok()?;
    capacity_from_usage(
        total,
        total.saturating_sub(available),
        CapacitySource::LocalMountpoint,
    )
}

fn rclone_about_capacity(
    rclone: &Path,
    config: &Path,
    remote: &str,
) -> Result<Option<CapacityInfo>, CapacityError> {
    let mut command = Command::new(rclone);
    command
        .args(["--config"])
        .arg(config)
        .args(["about", remote, "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = run_with_timeout(command, None, Duration::from_secs(12))?;
    if !output.status.success() {
        return Err(CapacityError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let about: RcloneAbout = serde_json::from_slice(&output.stdout)
        .map_err(|error| CapacityError::InvalidResponse(error.to_string()))?;
    Ok(capacity_from_about(about))
}

fn capacity_from_about(about: RcloneAbout) -> Option<CapacityInfo> {
    let total = about
        .total
        .or_else(|| Some(about.used?.saturating_add(about.free?)));
    let used = about
        .used
        .or_else(|| Some(total?.saturating_sub(about.free?)));
    capacity_from_usage(
        total.unwrap_or_default(),
        used.unwrap_or_default(),
        CapacitySource::RcloneAbout,
    )
}

fn lustre_project_capacity(server: &ServerConfig) -> Result<Option<CapacityInfo>, CapacityError> {
    if server.auth == AuthMethod::Password
        && server.source != "ssh_config"
        && !server.ssh_config_managed
    {
        return Ok(None);
    }
    let Some(ssh) =
        crate::rclone_binary::find_system_executable(if cfg!(windows) { "ssh.exe" } else { "ssh" })
    else {
        return Ok(None);
    };
    let mut arguments = vec![
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ConnectTimeout=8".to_owned(),
    ];
    if (server.source == "ssh_config" || server.ssh_config_managed)
        && !server.host_alias.trim().is_empty()
    {
        let config = if !server.managed_ssh_config_path.trim().is_empty() {
            &server.managed_ssh_config_path
        } else {
            &server.ssh_config_path
        };
        if !config.trim().is_empty() {
            arguments.extend(["-F".into(), config.clone()]);
        }
        arguments.push(server.host_alias.clone());
    } else {
        if !server.user.trim().is_empty() {
            arguments.extend(["-l".into(), server.user.clone()]);
        }
        arguments.extend(["-p".into(), server.port.clone()]);
        if !server.key_file.trim().is_empty() {
            arguments.extend([
                "-i".into(),
                server.key_file.clone(),
                "-o".into(),
                "IdentitiesOnly=yes".into(),
            ]);
        }
        arguments.push(server.host.clone());
    }
    arguments.extend([
        "sh".into(),
        "-s".into(),
        "--".into(),
        remote_path_for_capacity(server),
    ]);
    let mut command = Command::new(ssh);
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = run_with_timeout(
        command,
        Some(LUSTRE_CAPACITY_SCRIPT.as_bytes()),
        Duration::from_secs(12),
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(parse_lustre_quota(&String::from_utf8_lossy(&output.stdout)))
}

fn run_with_timeout(
    mut command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<std::process::Output, CapacityError> {
    let mut child = command.spawn()?;
    if let Some(input) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin.write_all(input)?;
    }
    if child.wait_timeout(timeout)?.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(CapacityError::Timeout);
    }
    child.wait_with_output().map_err(CapacityError::from)
}

fn remote_path_for_capacity(server: &ServerConfig) -> String {
    let path = server.remote_path.trim();
    if path.is_empty() {
        ".".into()
    } else {
        path.into()
    }
}

pub fn parse_lustre_quota(output: &str) -> Option<CapacityInfo> {
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("Disk ")
            || line.to_ascii_lowercase().starts_with("filesystem")
        {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let (Ok(used_kib), Ok(limit_kib)) = (fields[1].parse::<u64>(), fields[3].parse::<u64>())
        else {
            continue;
        };
        if limit_kib == 0 {
            return None;
        }
        return capacity_from_usage(
            limit_kib.saturating_mul(1024),
            used_kib.saturating_mul(1024),
            CapacitySource::LustreProjectQuota,
        );
    }
    None
}

fn capacity_from_usage(total: u64, used: u64, source: CapacitySource) -> Option<CapacityInfo> {
    if total == 0 {
        return None;
    }
    let used = used.min(total);
    let percent = ((used as u128 * 100 + total as u128 / 2) / total as u128).min(100) as u8;
    Some(CapacityInfo {
        used,
        total,
        percent,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lustre_quota_uses_the_hard_limit_and_clamps_percentage() {
        let capacity = parse_lustre_quota(
            "Disk quotas for prj 42 (pid 42):\nFilesystem  kbytes quota limit grace files quota limit grace\n/lustre 1200 0 1000 - 3 0 0 -\n",
        )
        .unwrap();
        assert_eq!(capacity.used, 1000 * 1024);
        assert_eq!(capacity.total, 1000 * 1024);
        assert_eq!(capacity.percent, 100);
        assert_eq!(capacity.source, CapacitySource::LustreProjectQuota);
    }

    #[test]
    fn empty_or_unlimited_lustre_quotas_are_not_presented_as_capacity() {
        assert!(parse_lustre_quota("/lustre 1200 0 0 -").is_none());
        assert!(parse_lustre_quota("no project quota").is_none());
    }

    #[test]
    fn rclone_about_derives_missing_used_or_total_values() {
        let from_free = capacity_from_about(RcloneAbout {
            total: Some(100),
            used: None,
            free: Some(40),
        })
        .unwrap();
        assert_eq!((from_free.used, from_free.total), (60, 100));

        let from_parts = capacity_from_about(RcloneAbout {
            total: None,
            used: Some(25),
            free: Some(75),
        })
        .unwrap();
        assert_eq!((from_parts.used, from_parts.total), (25, 100));
    }
}
