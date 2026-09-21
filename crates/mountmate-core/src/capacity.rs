use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::Deserialize;
use thiserror::Error;
use wait_timeout::ChildExt;

use crate::rc::{HttpRcClient, RcApi};
use crate::rclone::openssh_target_arguments;
use crate::{AuthMethod, MountState, ServerConfig};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const LUSTRE_CAPACITY_SCRIPT: &str = r#"set -eu
target=${1:-.}
if [ -z "$target" ]; then target=.; fi
case "$target" in
  '~') target=$HOME ;;
  '~/'*) target=$HOME/${target#\~/} ;;
esac
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
if ! printf '%s\n' "$quota_out" | awk 'NF >= 4 && $2 ~ /^[0-9]+[*]?$/ {found=1} END {exit !found}'; then
  quota_out=$(lfs quota -p "$project_id" "$mountpoint" 2>/dev/null || true)
fi
printf '%s\n' "$quota_out"
"#;

const FILESYSTEM_CAPACITY_SCRIPT: &str = r#"set -eu
target=${1:-.}
if [ -z "$target" ]; then target=.; fi
case "$target" in
  '~') target=$HOME ;;
  '~/'*) target=$HOME/${target#\~/} ;;
esac
if [ -d "$target" ]; then
  resolved=$(cd "$target" 2>/dev/null && pwd -P) || exit 0
else
  resolved=$(readlink -f -- "$target" 2>/dev/null || printf '%s' "$target")
fi
df -Pk "$resolved" 2>/dev/null | awk '
  NR == 2 && $2 ~ /^[0-9]+$/ && $3 ~ /^[0-9]+$/ && $4 ~ /^[0-9]+$/ {
    print $2 "\t" $3 "\t" $4
  }
'
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacitySource {
    LocalMountpoint,
    LustreProjectQuota,
    RcloneAbout,
    RemoteFilesystem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityInfo {
    pub used: u64,
    pub total: u64,
    pub percent: u8,
    pub source: CapacitySource,
    /// The Lustre block soft limit, when the capacity came from a project
    /// quota. The hard limit remains `total` and is the primary capacity.
    pub soft_total: Option<u64>,
    /// Optional inode usage for filesystems (notably Lustre) that expose it.
    pub inode: Option<InodeInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InodeInfo {
    pub used: u64,
    pub total: u64,
    pub percent: u8,
    /// The inode soft limit, when the filesystem exposes one.
    pub soft_total: Option<u64>,
}

impl CapacityInfo {
    /// Returns the soft-limit position on the hard-limit capacity bar.
    pub fn soft_limit_percent(&self) -> Option<f32> {
        let soft = self.soft_total?;
        if self.total == 0 || soft >= self.total {
            return None;
        }
        Some((soft as f64 * 100.0 / self.total as f64).clamp(0.0, 100.0) as f32)
    }

    pub fn soft_limit_exceeded(&self) -> bool {
        self.soft_total
            .is_some_and(|soft| soft < self.total && self.used > soft)
    }
}

impl InodeInfo {
    /// Returns the inode soft-limit position on the hard-limit bar.
    pub fn soft_limit_percent(&self) -> Option<f32> {
        let soft = self.soft_total?;
        if self.total == 0 || soft >= self.total {
            return None;
        }
        Some((soft as f64 * 100.0 / self.total as f64).clamp(0.0, 100.0) as f32)
    }

    pub fn soft_limit_exceeded(&self) -> bool {
        self.soft_total
            .is_some_and(|soft| soft < self.total && self.used > soft)
    }
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
    #[serde(rename = "lustreQuota")]
    lustre_quota: Option<LustreQuotaDetails>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LustreQuotaDetails {
    soft_total: Option<u64>,
    inodes_used: Option<u64>,
    inodes_total: Option<u64>,
    inodes_soft_total: Option<u64>,
}

/// Query the backend already owned by the mount. Its SSH session carries the
/// verified host key and login credentials, including passwords/passphrases.
pub(crate) fn session_capacity(state: &MountState) -> Result<Option<CapacityInfo>, CapacityError> {
    let client = HttpRcClient::with_credentials(
        &state.rc_addr,
        &state.rc_user,
        &state.rc_pass,
        Duration::from_secs(12),
    )
    .map_err(|error| CapacityError::Command(error.to_string()))?;
    capacity_from_session(&client, &state.remote)
}

fn capacity_from_session(
    client: &impl RcApi,
    remote: &str,
) -> Result<Option<CapacityInfo>, CapacityError> {
    let response = client
        .call("operations/about", serde_json::json!({ "fs": remote }))
        .map_err(|error| CapacityError::Command(error.to_string()))?;
    let about: RcloneAbout = serde_json::from_value(response)
        .map_err(|error| CapacityError::InvalidResponse(error.to_string()))?;
    Ok(capacity_from_about(about))
}

pub fn mounted_capacity(
    server: &ServerConfig,
    state: &MountState,
    rclone_config: &Path,
    external_ssh: Option<&[String]>,
) -> Result<Option<CapacityInfo>, CapacityError> {
    // Project quotas describe the directory's actual allowance; a successful
    // statfs on the mount usually describes the entire backing filesystem.
    let project_result = lustre_project_capacity(server, external_ssh);
    if let Ok(Some(capacity)) = &project_result {
        return Ok(Some(*capacity));
    }
    if let Some(capacity) = local_mount_capacity(&state.mountpoint) {
        return Ok(Some(capacity));
    }
    let rclone_result = rclone_about_capacity(&state.rclone, rclone_config, &state.remote);
    if let Ok(Some(capacity)) = &rclone_result {
        return Ok(Some(*capacity));
    }
    let remote_result = remote_filesystem_capacity(server, external_ssh);
    if let Ok(Some(capacity)) = &remote_result {
        return Ok(Some(*capacity));
    }
    match (rclone_result, remote_result, project_result) {
        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
        _ => Ok(None),
    }
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
    let mut capacity = capacity_from_usage(
        total.unwrap_or_default(),
        used.unwrap_or_default(),
        CapacitySource::RcloneAbout,
    )?;
    if let Some(quota) = about.lustre_quota {
        capacity.source = CapacitySource::LustreProjectQuota;
        capacity.soft_total = quota
            .soft_total
            .filter(|soft| *soft > 0 && *soft <= capacity.total);
        if let (Some(used), Some(total)) = (quota.inodes_used, quota.inodes_total) {
            capacity.inode = inode_from_usage(total, used).map(|mut inode| {
                inode.soft_total = quota
                    .inodes_soft_total
                    .filter(|soft| *soft > 0 && *soft <= total);
                inode
            });
        }
    }
    Some(capacity)
}

fn lustre_project_capacity(
    server: &ServerConfig,
    external_ssh: Option<&[String]>,
) -> Result<Option<CapacityInfo>, CapacityError> {
    let Some(output) = ssh_capacity_output(server, LUSTRE_CAPACITY_SCRIPT, external_ssh)? else {
        return Ok(None);
    };
    Ok(parse_lustre_quota(&output))
}

fn remote_filesystem_capacity(
    server: &ServerConfig,
    external_ssh: Option<&[String]>,
) -> Result<Option<CapacityInfo>, CapacityError> {
    let Some(output) = ssh_capacity_output(server, FILESYSTEM_CAPACITY_SCRIPT, external_ssh)?
    else {
        return Ok(None);
    };
    Ok(parse_filesystem_capacity(&output))
}

fn ssh_capacity_output(
    server: &ServerConfig,
    script: &str,
    external_ssh: Option<&[String]>,
) -> Result<Option<String>, CapacityError> {
    if external_ssh.is_none() && !supports_system_ssh_capacity(server) {
        return Ok(None);
    }
    let (ssh, mut arguments) = if let Some(connector) = external_ssh {
        let Some((program, arguments)) = connector.split_first() else {
            return Err(CapacityError::Command("empty shared SSH connector".into()));
        };
        (std::path::PathBuf::from(program), arguments.to_vec())
    } else {
        let Some(ssh) = crate::rclone_binary::find_system_executable(if cfg!(windows) {
            "ssh.exe"
        } else {
            "ssh"
        }) else {
            return Ok(None);
        };
        let mut arguments = vec![
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=8".into(),
        ];
        arguments.extend(
            openssh_target_arguments(server)
                .map_err(|error| CapacityError::Command(error.to_string()))?,
        );
        (ssh, arguments)
    };
    arguments.extend([
        "sh".into(),
        "-s".into(),
        "--".into(),
        quote_remote_shell_argument(&remote_path_for_capacity(server)),
    ]);
    let mut command = Command::new(ssh);
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = run_with_timeout(command, Some(script.as_bytes()), Duration::from_secs(12))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

fn supports_system_ssh_capacity(server: &ServerConfig) -> bool {
    server.auth != AuthMethod::Password
        || server.mode == "ssh_config"
        || matches!(server.source.as_str(), "ssh_config" | "ssh_config_batch")
        || server.ssh_config_managed
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

fn quote_remote_shell_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
        let row: Vec<_> = line.split_whitespace().collect();
        // lfs places long filesystem paths on their own line. The following
        // numeric row then starts directly with used blocks.
        let fields = if row
            .first()
            .and_then(|value| parse_quota_used_token(value))
            .is_some()
        {
            &row[..]
        } else if row.len() > 1 {
            &row[1..]
        } else {
            continue;
        };
        if fields.len() < 3 {
            continue;
        }
        let Some(used_kib) = parse_quota_used_token(fields[0]) else {
            continue;
        };
        let Some(limit_kib) = parse_quota_limit_token(fields[2]) else {
            continue;
        };
        let mut capacity = capacity_from_usage(
            limit_kib.saturating_mul(1024),
            used_kib.saturating_mul(1024),
            CapacitySource::LustreProjectQuota,
        )?;
        if let Some(soft_kib) = parse_quota_limit_token(fields[1])
            && soft_kib <= limit_kib
        {
            capacity.soft_total = Some(soft_kib.saturating_mul(1024));
        }
        // lfs quota columns after the byte quota are: files used, quota, limit, grace.
        // A zero limit means unlimited, so leave inode information unavailable.
        if fields.len() >= 7
            && let (Some(inode_used), Some(inode_limit)) = (
                parse_quota_used_token(fields[4]),
                parse_quota_limit_token(fields[6]),
            )
        {
            capacity.inode = inode_from_usage(inode_limit, inode_used).map(|mut inode| {
                if let Some(inode_soft) = parse_quota_limit_token(fields[5])
                    && inode_soft <= inode_limit
                {
                    inode.soft_total = Some(inode_soft);
                }
                inode
            });
        }
        return Some(capacity);
    }
    None
}

/// Parse a Lustre quota usage column. Lustre appends `*` when a quota is over
/// its soft or hard threshold; the marker is presentation metadata rather than
/// part of the numeric value.
fn parse_quota_used_token(value: &str) -> Option<u64> {
    value.strip_suffix('*').unwrap_or(value).parse::<u64>().ok()
}

/// Parse a Lustre quota limit column. `-`, `--`, and zero represent an
/// unlimited limit. The caller can distinguish an unavailable hard limit from
/// a real numeric capacity by checking the returned `Option`.
fn parse_quota_limit_token(value: &str) -> Option<u64> {
    let value = value.strip_suffix('*').unwrap_or(value);
    if matches!(value, "-" | "--") {
        return None;
    }
    let value = value.parse::<u64>().ok()?;
    (value > 0).then_some(value)
}

pub fn parse_filesystem_capacity(output: &str) -> Option<CapacityInfo> {
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        let (Ok(total_kib), Ok(used_kib), Ok(available_kib)) = (
            fields[0].parse::<u64>(),
            fields[1].parse::<u64>(),
            fields[2].parse::<u64>(),
        ) else {
            continue;
        };
        let total_kib = total_kib.max(used_kib.saturating_add(available_kib));
        return capacity_from_usage(
            total_kib.saturating_mul(1024),
            used_kib.saturating_mul(1024),
            CapacitySource::RemoteFilesystem,
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
        soft_total: None,
        inode: None,
    })
}

fn inode_from_usage(total: u64, used: u64) -> Option<InodeInfo> {
    if total == 0 {
        return None;
    }
    let used = used.min(total);
    let percent = ((used as u128 * 100 + total as u128 / 2) / total as u128).min(100) as u8;
    Some(InodeInfo {
        used,
        total,
        percent,
        soft_total: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_password_capacity_probe_accepts_single_batch_and_legacy_imports() {
        let mut server = ServerConfig {
            auth: AuthMethod::Password,
            connection_method: crate::ConnectionMethod::Native,
            ..ServerConfig::default()
        };
        assert!(!supports_system_ssh_capacity(&server));

        for source in ["ssh_config", "ssh_config_batch"] {
            server.source = source.into();
            assert!(supports_system_ssh_capacity(&server), "source {source}");
        }

        // Legacy records may identify the import only through their mode.
        server.source = "manual".into();
        server.mode = "ssh_config".into();
        assert!(supports_system_ssh_capacity(&server));

        server.mode = "manual".into();
        server.ssh_config_managed = true;
        assert!(supports_system_ssh_capacity(&server));

        server.ssh_config_managed = false;
        assert!(!supports_system_ssh_capacity(&server));
        server.auth = AuthMethod::Key;
        assert!(supports_system_ssh_capacity(&server));
    }

    #[cfg(unix)]
    fn mounted_state(path: &Path) -> MountState {
        serde_json::from_value(serde_json::json!({
            "pid": 1, "server_id": "quota-test", "remote": "quota-test:",
            "mountpoint": path, "log": path.join("log"), "rc_addr": "127.0.0.1:1",
            "rclone": path.join("unused-rclone")
        }))
        .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn project_quota_precedes_available_mount_capacity_for_every_import_source() {
        let temp = tempfile::tempdir().unwrap();
        let state = mounted_state(temp.path());
        assert!(local_mount_capacity(temp.path()).is_some());
        // The already-verified connector stands in for an MFA-only session:
        // no separate system SSH login is needed for the quota command.
        let connector = vec![
            "/bin/sh".into(),
            "-c".into(),
            "cat >/dev/null; printf '/lustre 10 0 100 - 0 0 0\\n'".into(),
        ];
        for source in ["manual", "ssh_config", "sai_cluster"] {
            let server = ServerConfig {
                source: source.into(),
                connection_method: crate::ConnectionMethod::Interactive,
                ..ServerConfig::default()
            };
            let capacity = mounted_capacity(
                &server,
                &state,
                &temp.path().join("rclone.conf"),
                Some(&connector),
            )
            .unwrap()
            .unwrap();
            assert_eq!(capacity.source, CapacitySource::LustreProjectQuota);
            assert_eq!((capacity.used, capacity.total), (10 * 1024, 100 * 1024));
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_project_query_falls_back_to_available_mount_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let server = ServerConfig {
            source: "sai_cluster".into(),
            ..ServerConfig::default()
        };
        let connector = vec![temp.path().join("missing-ssh").display().to_string()];
        let result = mounted_capacity(
            &server,
            &mounted_state(temp.path()),
            &temp.path().join("rclone.conf"),
            Some(&connector),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.source, CapacitySource::LocalMountpoint);
    }

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
        assert_eq!(capacity.soft_total, None);
        assert_eq!(capacity.inode, None);
    }

    #[test]
    fn lustre_quota_keeps_the_soft_limit_as_a_marker_but_uses_hard_limit_for_capacity() {
        let capacity = parse_lustre_quota("/lustre 40000 30000 100000 - 3 0 0 -\n").unwrap();
        assert_eq!(capacity.used, 40000 * 1024);
        assert_eq!(capacity.total, 100000 * 1024);
        assert_eq!(capacity.soft_total, Some(30000 * 1024));
        assert_eq!(capacity.percent, 40);
        assert_eq!(capacity.soft_limit_percent(), Some(30.0));
        assert!(capacity.soft_limit_exceeded());

        let below_soft = parse_lustre_quota("/lustre 20000 30000 100000 - 3 0 0 -\n").unwrap();
        assert!(!below_soft.soft_limit_exceeded());
        assert_eq!(below_soft.soft_limit_percent(), Some(30.0));
        let at_soft = parse_lustre_quota("/lustre 30000 30000 100000 - 3 0 0 -\n").unwrap();
        assert!(!at_soft.soft_limit_exceeded());

        let equal_limits = parse_lustre_quota("/lustre 30000 100000 100000 - 3 0 0 -\n").unwrap();
        assert_eq!(equal_limits.soft_limit_percent(), None);
        assert!(!equal_limits.soft_limit_exceeded());
    }

    #[test]
    fn lustre_quota_ignores_unlimited_or_invalid_soft_limits() {
        let unlimited = parse_lustre_quota("/lustre 1200 0 1000 - 3 0 0 -\n").unwrap();
        assert_eq!(unlimited.soft_total, None);

        let invalid = parse_lustre_quota("/lustre 1200 2000 1000 - 3 0 0 -\n").unwrap();
        assert_eq!(invalid.soft_total, None);

        let non_numeric = parse_lustre_quota("/lustre 1200 - 1000  - 3 0 0 -\n").unwrap();
        assert_eq!(non_numeric.soft_total, None);
    }

    #[test]
    fn lustre_quota_accepts_over_limit_markers() {
        let capacity = parse_lustre_quota("/lustre 1200* 1000* 2000* - 300* 0 1000* -\n").unwrap();
        assert_eq!(capacity.used, 1200 * 1024);
        assert_eq!(capacity.total, 2000 * 1024);
        assert_eq!(capacity.soft_total, Some(1000 * 1024));
        assert_eq!(
            capacity.inode,
            Some(InodeInfo {
                used: 300,
                total: 1000,
                percent: 30,
                soft_total: None,
            })
        );
    }

    #[test]
    fn lustre_quota_skips_rows_without_a_hard_limit() {
        assert!(parse_lustre_quota("/lustre 1200 1000 - -\n").is_none());
        assert!(parse_lustre_quota("/lustre 1200 1000 -- -\n").is_none());
    }

    #[test]
    fn empty_or_unlimited_lustre_quotas_are_not_presented_as_capacity() {
        assert!(parse_lustre_quota("/lustre 1200 0 0 -").is_none());
        assert!(parse_lustre_quota("no project quota").is_none());
    }

    #[test]
    fn lustre_quota_parses_inode_limit_when_present() {
        let capacity = parse_lustre_quota("/lustre 1200 0 2000 - 300 500 1000 -\n").unwrap();
        assert_eq!(
            capacity.inode,
            Some(InodeInfo {
                used: 300,
                total: 1000,
                percent: 30,
                soft_total: Some(500),
            })
        );
        let inode = capacity.inode.unwrap();
        assert_eq!(inode.soft_limit_percent(), Some(50.0));
        assert!(!inode.soft_limit_exceeded());

        let exceeded = parse_lustre_quota("/lustre 1200 0 2000 - 700 500 1000 -\n")
            .unwrap()
            .inode
            .unwrap();
        assert!(exceeded.soft_limit_exceeded());

        let equal_limits = parse_lustre_quota("/lustre 1200 0 2000 - 700 1000 1000 -\n")
            .unwrap()
            .inode
            .unwrap();
        assert_eq!(equal_limits.soft_limit_percent(), None);
        assert!(!equal_limits.soft_limit_exceeded());
    }

    #[test]
    fn lustre_inode_usage_is_clamped_and_unlimited_inode_quota_is_ignored() {
        let over_limit = parse_lustre_quota("/lustre 10 0 20 - 12 0 10 -\n").unwrap();
        assert_eq!(
            over_limit.inode,
            Some(InodeInfo {
                used: 10,
                total: 10,
                percent: 100,
                soft_total: None,
            })
        );

        let unlimited = parse_lustre_quota("/lustre 10 0 20 - 12 0 0 -\n").unwrap();
        assert_eq!(unlimited.inode, None);
    }

    #[test]
    fn rclone_about_derives_missing_used_or_total_values() {
        let from_free = capacity_from_about(RcloneAbout {
            total: Some(100),
            used: None,
            free: Some(40),
            ..Default::default()
        })
        .unwrap();
        assert_eq!((from_free.used, from_free.total), (60, 100));

        let from_parts = capacity_from_about(RcloneAbout {
            total: None,
            used: Some(25),
            free: Some(75),
            ..Default::default()
        })
        .unwrap();
        assert_eq!((from_parts.used, from_parts.total), (25, 100));
    }

    #[test]
    fn session_quota_preserves_both_soft_limits_and_uses_mounted_path() {
        struct MountedSession;
        impl RcApi for MountedSession {
            fn call(
                &self,
                method: &str,
                params: serde_json::Value,
            ) -> Result<serde_json::Value, crate::rc::RcError> {
                assert_eq!(method, "operations/about");
                assert_eq!(
                    params,
                    serde_json::json!({"fs": "sai:/project with spaces"})
                );
                Ok(serde_json::json!({
                    "used": 40960000, "total": 102400000, "free": 61440000,
                    "lustreQuota": {
                        "softTotal": 30720000,
                        "inodesUsed": 700, "inodesTotal": 1000, "inodesSoftTotal": 500
                    }
                }))
            }
        }
        let capacity = capacity_from_session(&MountedSession, "sai:/project with spaces")
            .unwrap()
            .unwrap();
        assert_eq!(capacity.source, CapacitySource::LustreProjectQuota);
        assert_eq!((capacity.used, capacity.total), (40960000, 102400000));
        assert_eq!(capacity.soft_limit_percent(), Some(30.0));
        assert!(capacity.soft_limit_exceeded());
        let inode = capacity.inode.unwrap();
        assert_eq!((inode.used, inode.total), (700, 1000));
        assert_eq!(inode.soft_limit_percent(), Some(50.0));
        assert!(inode.soft_limit_exceeded());
    }

    #[test]
    fn quota_details_ignore_unlimited_or_inconsistent_limits() {
        for soft in [0, 101] {
            let about: RcloneAbout = serde_json::from_value(serde_json::json!({
                "total": 100, "used": 25,
                "lustreQuota": {"softTotal": soft, "inodesTotal": 0, "inodesUsed": 12}
            }))
            .unwrap();
            let capacity = capacity_from_about(about).unwrap();
            assert_eq!(capacity.soft_total, None);
            assert_eq!(capacity.inode, None);
        }
    }

    #[test]
    fn wrapped_quota_row_keeps_blocks_and_inodes_in_their_columns() {
        let wrapped = parse_lustre_quota("Disk quotas for prj 42 (pid 42):\nFilesystem kbytes quota limit grace files quota limit grace\n/very/long/filesystem/name\n 40000* 30000 100000 - 700* 500 1000 6d\n").unwrap();
        let normal =
            parse_lustre_quota("/lustre 40000* 30000 100000 - 700* 500 1000 6d\n").unwrap();
        assert_eq!(wrapped, normal);
        assert_eq!(wrapped.soft_total, Some(30000 * 1024));
        assert_eq!(wrapped.inode.unwrap().soft_total, Some(500));
    }

    #[test]
    fn remote_df_capacity_uses_kib_blocks_and_tolerates_rounding() {
        let capacity = parse_filesystem_capacity("1048576 262144 786400\n").unwrap();
        assert_eq!(capacity.total, 1_048_576 * 1024);
        assert_eq!(capacity.used, 262_144 * 1024);
        assert_eq!(capacity.percent, 25);
        assert_eq!(capacity.source, CapacitySource::RemoteFilesystem);
    }

    #[test]
    fn malformed_remote_df_is_ignored() {
        assert!(parse_filesystem_capacity("capacity unavailable\n").is_none());
        assert!(parse_filesystem_capacity("0 0 0\n").is_none());
    }

    #[test]
    fn remote_capacity_path_is_one_shell_argument() {
        assert_eq!(
            quote_remote_shell_argument("~/folder with 'quotes'"),
            "'~/folder with '\\''quotes'\\'''"
        );
    }
}
