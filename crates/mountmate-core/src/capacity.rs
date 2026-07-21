use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::Deserialize;
use thiserror::Error;
use wait_timeout::ChildExt;

use crate::{AuthMethod, ConnectionMethod, MountState, ServerConfig};

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
if ! printf '%s\n' "$quota_out" | awk 'NF >= 4 && $2 ~ /^[0-9]+$/ {found=1} END {exit !found}'; then
  quota_out=$(lfs quota -p "$project_id" "$mountpoint" 2>/dev/null || true)
fi
printf '%s\n' "$quota_out"
"#;

// Keep the complete Lustre probe in one remote shell invocation.  The markers
// are deliberately line-oriented so arbitrary lfs warnings can remain in the
// response without making the parser depend on locale-specific wording.
const LUSTRE_QUOTA_SCRIPT: &str = r#"set -u
export LC_ALL=C
export LANG=C

target=${1:-.}
if [ -z "$target" ]; then target=.; fi
case "$target" in
  '~') target=$HOME ;;
  '~/'*) target=$HOME/${target#\~/} ;;
esac

if ! command -v lfs >/dev/null 2>&1; then
  printf '@@MMQ|STATUS|UNAVAILABLE|lfs-missing\n'
  exit 0
fi

if [ -d "$target" ]; then
  resolved=$(cd "$target" 2>/dev/null && pwd -P) || {
    printf '@@MMQ|STATUS|UNAVAILABLE|path-unresolved\n'
    exit 0
  }
else
  resolved=$(readlink -f -- "$target" 2>/dev/null || printf '%s' "$target")
fi
if [ -z "$resolved" ]; then
  printf '@@MMQ|STATUS|UNAVAILABLE|path-unresolved\n'
  exit 0
fi

df_out=$(df -P -T "$resolved" 2>/dev/null || true)
df_line=$(printf '%s\n' "$df_out" | awk 'NR == 2 {print $2 "\t" $NF; exit}')
fstype=${df_line%%	*}
mountpoint=${df_line#*	}
if [ -z "$fstype" ] || [ -z "$mountpoint" ]; then
  printf '@@MMQ|STATUS|UNAVAILABLE|filesystem-unresolved\n'
  exit 0
fi
if [ "$fstype" != "lustre" ]; then
  printf '@@MMQ|STATUS|NOT_LUSTRE|%s\n' "$fstype"
  exit 0
fi

project_out=$(lfs project -d "$resolved" 2>&1 || true)
project_id=$(printf '%s\n' "$project_out" | awk '$1 ~ /^[0-9]+$/ {print $1; exit}')

uid=$(id -u 2>/dev/null || true)
gid=$(id -g 2>/dev/null || true)
user_name=$(id -un 2>/dev/null || true)
group_name=$(id -gn 2>/dev/null || true)
printf '@@MMQ|STATUS|LUSTRE\n'
printf '@@MMQ|PATH|%s|%s\n' "$resolved" "$mountpoint"
printf '@@MMQ|PROJECT|%s\n' "$project_id"
printf '@@MMQ|IDENTITY|%s|%s|%s|%s\n' "$uid" "$gid" "$user_name" "$group_name"

run_scope() {
  scope=$1
  shift
  printf '@@MMQ|BEGIN|%s\n' "$scope"
  quota_output=$(lfs quota "$@" "$mountpoint" 2>&1)
  quota_status=$?
  printf '%s\n' "$quota_output"
  printf '@@MMQ|END|%s|%s\n' "$scope" "$quota_status"
}

if [ -n "$project_id" ]; then
  run_scope project -p "$project_id"
else
  printf '@@MMQ|BEGIN|project\nproject ID is unavailable\n@@MMQ|END|project|65\n'
fi
if [ -n "$uid" ]; then
  run_scope user -u "$uid"
else
  printf '@@MMQ|BEGIN|user\n@@MMQ|END|user|64\n'
fi
if [ -n "$gid" ]; then
  run_scope group -g "$gid"
else
  printf '@@MMQ|BEGIN|group\n@@MMQ|END|group|64\n'
fi
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
}

/// The reason a Lustre probe did not provide quota details.  The payload is
/// intentionally textual because remote tools return installation-specific
/// diagnostics; callers can display it without losing useful context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LustreStatusReason {
    AuthUnavailable,
    NotLustre(String),
    LfsMissing,
    PathUnresolved,
    FilesystemUnresolved,
    ProjectIdMissing,
    IdentityUnavailable,
    QuotaUnavailable(String),
    InvalidOutput(String),
    Other(String),
}

impl std::fmt::Display for LustreStatusReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AuthUnavailable => formatter.write_str("authentication unavailable"),
            Self::NotLustre(filesystem) => {
                write!(formatter, "remote filesystem is not Lustre ({filesystem})")
            }
            Self::LfsMissing => formatter.write_str("lfs is unavailable"),
            Self::PathUnresolved => formatter.write_str("remote path could not be resolved"),
            Self::FilesystemUnresolved => formatter.write_str("filesystem could not be resolved"),
            Self::ProjectIdMissing => formatter.write_str("Lustre project ID is unavailable"),
            Self::IdentityUnavailable => {
                formatter.write_str("remote user or group identity is unavailable")
            }
            Self::QuotaUnavailable(message)
            | Self::InvalidOutput(message)
            | Self::Other(message) => formatter.write_str(message),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LustreGraceState {
    None,
    Active,
    Expired,
    Unlimited,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LustreGrace {
    /// The exact grace token emitted by `lfs quota` (for example `1d` or `-`).
    pub raw: String,
    pub state: LustreGraceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LustreQuotaSeverity {
    Normal,
    SoftExceeded,
    HardExceeded,
    Grace,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LustreQuotaMetric {
    /// Used blocks are KiB; used inodes are counts.  Values are never clamped.
    pub used: u64,
    pub soft: Option<u64>,
    pub hard: Option<u64>,
    pub grace: LustreGrace,
    pub severity: LustreQuotaSeverity,
    /// Whether the source row marked the used value with a trailing `*`.
    pub marked: bool,
    pub soft_marked: bool,
    pub hard_marked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LustreQuotaScopeDetails {
    pub blocks: LustreQuotaMetric,
    pub inodes: LustreQuotaMetric,
}

impl LustreQuotaScopeDetails {
    pub fn block(&self) -> &LustreQuotaMetric {
        &self.blocks
    }

    pub fn inode(&self) -> &LustreQuotaMetric {
        &self.inodes
    }

    pub fn block_capacity(&self) -> Option<CapacityInfo> {
        let hard = self.blocks.hard?;
        capacity_from_usage(
            hard.saturating_mul(1024),
            self.blocks.used.saturating_mul(1024),
            CapacitySource::LustreProjectQuota,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LustreQuotaScopeStatus {
    Available(LustreQuotaScopeDetails),
    Unavailable { reason: LustreStatusReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LustreQuotaDetails {
    pub resolved_path: String,
    pub mountpoint: String,
    pub project_id: Option<u64>,
    pub uid: Option<u64>,
    pub gid: Option<u64>,
    pub user_name: Option<String>,
    pub group_name: Option<String>,
    pub project: LustreQuotaScopeStatus,
    pub current_user: LustreQuotaScopeStatus,
    pub primary_group: LustreQuotaScopeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LustreQuotaStatus {
    NotLustre { reason: LustreStatusReason },
    Unavailable { reason: LustreStatusReason },
    Available(Box<LustreQuotaDetails>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacitySnapshot {
    pub capacity: Option<CapacityInfo>,
    pub lustre: LustreQuotaStatus,
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
    mounted_capacity_with_connector(server, state, rclone_config, None)
}

pub fn mounted_capacity_with_connector(
    server: &ServerConfig,
    state: &MountState,
    rclone_config: &Path,
    connector: Option<&[String]>,
) -> Result<Option<CapacityInfo>, CapacityError> {
    if server.source == "sai_cluster"
        && let Some(capacity) = lustre_project_capacity_with_connector(server, connector)?
    {
        return Ok(Some(capacity));
    }
    if let Some(capacity) = local_mount_capacity(&state.mountpoint) {
        return Ok(Some(capacity));
    }
    if server.source != "sai_cluster"
        && let Some(capacity) = lustre_project_capacity_with_connector(server, connector)?
    {
        return Ok(Some(capacity));
    }
    let rclone_result = rclone_about_capacity(&state.rclone, rclone_config, &state.remote);
    if let Ok(Some(capacity)) = &rclone_result {
        return Ok(Some(*capacity));
    }
    let remote_result = remote_filesystem_capacity_with_connector(server, connector);
    if let Ok(Some(capacity)) = &remote_result {
        return Ok(Some(*capacity));
    }
    match (rclone_result, remote_result) {
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(None),
    }
}

/// Return display capacity and the full Lustre probe result in one snapshot.
/// The existing fallback order is retained: SAI profiles prefer Lustre before
/// local statistics, while other profiles prefer local statistics first.
pub fn capacity_snapshot(
    server: &ServerConfig,
    state: &MountState,
    rclone_config: &Path,
) -> Result<CapacitySnapshot, CapacityError> {
    capacity_snapshot_with_connector(server, state, rclone_config, None)
}

pub fn capacity_snapshot_with_connector(
    server: &ServerConfig,
    state: &MountState,
    rclone_config: &Path,
    connector: Option<&[String]>,
) -> Result<CapacitySnapshot, CapacityError> {
    let lustre = match lustre_quota_status_with_connector(server, connector) {
        Ok(status) => status,
        Err(error) => LustreQuotaStatus::Unavailable {
            reason: LustreStatusReason::QuotaUnavailable(error.to_string()),
        },
    };
    let lustre_capacity = lustre_project_snapshot_capacity(&lustre);

    if server.source == "sai_cluster"
        && let Some(capacity) = lustre_capacity
    {
        return Ok(CapacitySnapshot {
            capacity: Some(capacity),
            lustre,
        });
    }
    let local = local_mount_capacity(&state.mountpoint);
    if let Some(capacity) = local {
        return Ok(CapacitySnapshot {
            capacity: Some(capacity),
            lustre,
        });
    }
    if server.source != "sai_cluster"
        && let Some(capacity) = lustre_capacity
    {
        return Ok(CapacitySnapshot {
            capacity: Some(capacity),
            lustre,
        });
    }

    let rclone_result = rclone_about_capacity(&state.rclone, rclone_config, &state.remote);
    if let Ok(Some(capacity)) = &rclone_result {
        return Ok(CapacitySnapshot {
            capacity: Some(*capacity),
            lustre,
        });
    }
    let remote_result = remote_filesystem_capacity_with_connector(server, connector);
    if let Ok(Some(capacity)) = &remote_result {
        return Ok(CapacitySnapshot {
            capacity: Some(*capacity),
            lustre,
        });
    }
    match (rclone_result, remote_result) {
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(CapacitySnapshot {
            capacity: None,
            lustre,
        }),
    }
}

/// Compatibility spelling for callers that use a mounted-capacity prefix.
pub fn mounted_capacity_snapshot(
    server: &ServerConfig,
    state: &MountState,
    rclone_config: &Path,
) -> Result<CapacitySnapshot, CapacityError> {
    capacity_snapshot(server, state, rclone_config)
}

/// Probe Lustre quota details without applying display-capacity fallbacks.
pub fn lustre_quota_status(server: &ServerConfig) -> Result<LustreQuotaStatus, CapacityError> {
    lustre_quota_status_with_connector(server, None)
}

pub fn lustre_quota_status_with_connector(
    server: &ServerConfig,
    connector: Option<&[String]>,
) -> Result<LustreQuotaStatus, CapacityError> {
    // The interactive/shared transport owns authentication and must never be
    // bypassed by starting a second SSH process that could prompt.
    if server.connection_method == ConnectionMethod::Interactive && connector.is_none() {
        return Ok(LustreQuotaStatus::Unavailable {
            reason: LustreStatusReason::AuthUnavailable,
        });
    }
    if server.auth == AuthMethod::Password
        && server.source != "ssh_config"
        && !server.ssh_config_managed
    {
        return Ok(LustreQuotaStatus::Unavailable {
            reason: LustreStatusReason::AuthUnavailable,
        });
    }
    let Some(output) = ssh_capacity_output_with_connector(server, LUSTRE_QUOTA_SCRIPT, connector)?
    else {
        return Ok(LustreQuotaStatus::Unavailable {
            reason: LustreStatusReason::Other("non-interactive SSH is unavailable".into()),
        });
    };
    Ok(parse_lustre_quota_snapshot(&output))
}

fn lustre_project_snapshot_capacity(status: &LustreQuotaStatus) -> Option<CapacityInfo> {
    let LustreQuotaStatus::Available(details) = status else {
        return None;
    };
    let LustreQuotaScopeStatus::Available(project) = &details.project else {
        return None;
    };
    project.block_capacity()
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

fn lustre_project_capacity_with_connector(
    server: &ServerConfig,
    connector: Option<&[String]>,
) -> Result<Option<CapacityInfo>, CapacityError> {
    let Some(output) =
        ssh_capacity_output_with_connector(server, LUSTRE_CAPACITY_SCRIPT, connector)?
    else {
        return Ok(None);
    };
    Ok(parse_lustre_quota(&output))
}

fn remote_filesystem_capacity_with_connector(
    server: &ServerConfig,
    connector: Option<&[String]>,
) -> Result<Option<CapacityInfo>, CapacityError> {
    let Some(output) =
        ssh_capacity_output_with_connector(server, FILESYSTEM_CAPACITY_SCRIPT, connector)?
    else {
        return Ok(None);
    };
    Ok(parse_filesystem_capacity(&output))
}

fn ssh_capacity_output_with_connector(
    server: &ServerConfig,
    script: &str,
    connector: Option<&[String]>,
) -> Result<Option<String>, CapacityError> {
    if server.connection_method == ConnectionMethod::Interactive && connector.is_none() {
        return Ok(None);
    }
    if server.auth == AuthMethod::Password
        && server.source != "ssh_config"
        && !server.ssh_config_managed
    {
        return Ok(None);
    }
    let (program, mut arguments) = if let Some(connector) = connector {
        let Some((program, arguments)) = connector.split_first() else {
            return Ok(None);
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
        (ssh, arguments)
    };
    arguments.extend([
        "sh".into(),
        "-s".into(),
        "--".into(),
        quote_remote_shell_argument(&remote_path_for_capacity(server)),
    ]);
    let mut command = Command::new(program);
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
    if let Some(details) = parse_lustre_scope_row(output) {
        return details.block_capacity();
    }
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

/// Parse a complete framed Lustre probe response.  Raw quota command output
/// is parsed independently for each scope, so one failed scope does not erase
/// details obtained for the other scopes.
pub fn parse_lustre_quota_snapshot(output: &str) -> LustreQuotaStatus {
    let mut status: Option<&str> = None;
    let mut status_reason = String::new();
    let mut resolved_path = String::new();
    let mut mountpoint = String::new();
    let mut project_id = None;
    let mut uid = None;
    let mut gid = None;
    let mut user_name = None;
    let mut group_name = None;
    let mut scope = None::<&str>;
    let mut scope_lines = [String::new(), String::new(), String::new()];
    let mut scope_exit = [None::<i32>, None::<i32>, None::<i32>];

    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("@@MMQ|STATUS|") {
            let mut fields = value.splitn(2, '|');
            status = fields.next();
            status_reason = fields.next().unwrap_or_default().trim().to_owned();
            continue;
        }
        if let Some(value) = line.strip_prefix("@@MMQ|PATH|") {
            let mut fields = value.splitn(2, '|');
            resolved_path = fields.next().unwrap_or_default().to_owned();
            mountpoint = fields.next().unwrap_or_default().to_owned();
            continue;
        }
        if let Some(value) = line.strip_prefix("@@MMQ|PROJECT|") {
            project_id = value.trim().parse::<u64>().ok();
            continue;
        }
        if let Some(value) = line.strip_prefix("@@MMQ|IDENTITY|") {
            let mut fields = value.splitn(4, '|');
            uid = fields.next().and_then(|value| value.parse::<u64>().ok());
            gid = fields.next().and_then(|value| value.parse::<u64>().ok());
            user_name = nonempty_string(fields.next().unwrap_or_default());
            group_name = nonempty_string(fields.next().unwrap_or_default());
            continue;
        }
        if let Some(value) = line.strip_prefix("@@MMQ|BEGIN|") {
            scope = scope_index(value.trim()).map(|index| match index {
                0 => "project",
                1 => "user",
                _ => "group",
            });
            continue;
        }
        if let Some(value) = line.strip_prefix("@@MMQ|END|") {
            let mut fields = value.split('|');
            let ended_scope = fields.next().unwrap_or_default();
            let code = fields.next().and_then(|value| value.parse::<i32>().ok());
            if let Some(index) = scope_index(ended_scope) {
                scope_exit[index] = code;
            }
            scope = None;
            continue;
        }
        if let Some(current) = scope.and_then(scope_index) {
            scope_lines[current].push_str(line);
            scope_lines[current].push('\n');
        }
    }

    match status {
        Some("NOT_LUSTRE") => LustreQuotaStatus::NotLustre {
            reason: LustreStatusReason::NotLustre(if status_reason.is_empty() {
                "remote filesystem is not Lustre".into()
            } else {
                status_reason
            }),
        },
        Some("UNAVAILABLE") => LustreQuotaStatus::Unavailable {
            reason: unavailable_reason(&status_reason),
        },
        Some("LUSTRE") => {
            if resolved_path.is_empty() || mountpoint.is_empty() {
                return LustreQuotaStatus::Unavailable {
                    reason: LustreStatusReason::InvalidOutput("missing path frame".into()),
                };
            }
            let project = if project_id.is_some() {
                scope_status(&scope_lines[0], scope_exit[0])
            } else {
                LustreQuotaScopeStatus::Unavailable {
                    reason: LustreStatusReason::ProjectIdMissing,
                }
            };
            let current_user = scope_status(&scope_lines[1], scope_exit[1]);
            let primary_group = scope_status(&scope_lines[2], scope_exit[2]);
            LustreQuotaStatus::Available(Box::new(LustreQuotaDetails {
                resolved_path,
                mountpoint,
                project_id,
                uid,
                gid,
                user_name,
                group_name,
                project,
                current_user,
                primary_group,
            }))
        }
        _ => LustreQuotaStatus::Unavailable {
            reason: LustreStatusReason::InvalidOutput("missing Lustre status frame".into()),
        },
    }
}

/// Parse one unframed `lfs quota` response.  This is useful for diagnostics and
/// keeps the parser independently testable from the remote shell framing.
pub fn parse_lustre_quota_scope(output: &str) -> Option<LustreQuotaScopeDetails> {
    parse_lustre_scope_row(output)
}

fn scope_status(output: &str, exit_code: Option<i32>) -> LustreQuotaScopeStatus {
    if exit_code != Some(0) {
        let message = output
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("lfs quota command failed")
            .to_owned();
        return LustreQuotaScopeStatus::Unavailable {
            reason: LustreStatusReason::QuotaUnavailable(message),
        };
    }
    parse_lustre_scope_row(output).map_or_else(
        || LustreQuotaScopeStatus::Unavailable {
            reason: LustreStatusReason::InvalidOutput("quota row was not recognized".into()),
        },
        LustreQuotaScopeStatus::Available,
    )
}

fn scope_index(scope: &str) -> Option<usize> {
    match scope.trim().to_ascii_lowercase().as_str() {
        "project" => Some(0),
        "user" | "current-user" | "current_user" => Some(1),
        "group" | "primary-group" | "primary_group" => Some(2),
        _ => None,
    }
}

fn nonempty_string(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

fn unavailable_reason(value: &str) -> LustreStatusReason {
    match value.trim().to_ascii_lowercase().as_str() {
        "lfs-missing" => LustreStatusReason::LfsMissing,
        "path-unresolved" => LustreStatusReason::PathUnresolved,
        "filesystem-unresolved" => LustreStatusReason::FilesystemUnresolved,
        "project-id-missing" => LustreStatusReason::ProjectIdMissing,
        "identity-unavailable" => LustreStatusReason::IdentityUnavailable,
        "" => LustreStatusReason::Other("Lustre probe unavailable".into()),
        _ => LustreStatusReason::Other(value.trim().to_owned()),
    }
}

fn parse_lustre_scope_row(output: &str) -> Option<LustreQuotaScopeDetails> {
    let mut pending = Vec::<String>::new();
    for line in output.lines() {
        let fields: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        if fields.is_empty() {
            continue;
        }
        if let Some(row) = parse_lustre_row_fields(&fields) {
            return Some(row);
        }
        // lfs can wrap a filesystem row after the filesystem column.  Carry a
        // path-like line into the next line, but never carry warning/header
        // text into numeric data.
        if pending.is_empty() && fields.len() <= 2 && fields[0].starts_with('/') {
            pending.extend(fields.iter().cloned());
            continue;
        }
        if !pending.is_empty() {
            let mut combined = pending.clone();
            combined.extend(fields.iter().map(|field| (*field).to_owned()));
            if let Some(row) = parse_lustre_row_fields(&combined) {
                return Some(row);
            }
            pending.clear();
        }
    }
    None
}

fn parse_lustre_row_fields(fields: &[String]) -> Option<LustreQuotaScopeDetails> {
    // Normal rows have a filesystem field followed by eight quota fields;
    // pathless wrapped rows have just the eight quota fields.
    for start in 0..fields.len() {
        if start + 8 < fields.len()
            && let (Some((block_used, block_marked)), Some((inode_used, inode_marked))) = (
                parse_used_token(&fields[start + 1]),
                parse_used_token(&fields[start + 5]),
            )
        {
            let Some((block_soft, block_soft_marked)) = parse_limit_token(&fields[start + 2])
            else {
                continue;
            };
            let Some((block_hard, block_hard_marked)) = parse_limit_token(&fields[start + 3])
            else {
                continue;
            };
            let Some((inode_soft, inode_soft_marked)) = parse_limit_token(&fields[start + 6])
            else {
                continue;
            };
            let Some((inode_hard, inode_hard_marked)) = parse_limit_token(&fields[start + 7])
            else {
                continue;
            };
            return Some(LustreQuotaScopeDetails {
                blocks: quota_metric(
                    block_used,
                    block_soft,
                    block_hard,
                    &fields[start + 4],
                    block_marked,
                    block_soft_marked,
                    block_hard_marked,
                ),
                inodes: quota_metric(
                    inode_used,
                    inode_soft,
                    inode_hard,
                    &fields[start + 8],
                    inode_marked,
                    inode_soft_marked,
                    inode_hard_marked,
                ),
            });
        }
    }
    if fields.len() >= 8
        && let (Some((block_used, block_marked)), Some((inode_used, inode_marked))) =
            (parse_used_token(&fields[0]), parse_used_token(&fields[4]))
    {
        let (block_soft, block_soft_marked) = parse_limit_token(&fields[1])?;
        let (block_hard, block_hard_marked) = parse_limit_token(&fields[2])?;
        let (inode_soft, inode_soft_marked) = parse_limit_token(&fields[5])?;
        let (inode_hard, inode_hard_marked) = parse_limit_token(&fields[6])?;
        return Some(LustreQuotaScopeDetails {
            blocks: quota_metric(
                block_used,
                block_soft,
                block_hard,
                &fields[3],
                block_marked,
                block_soft_marked,
                block_hard_marked,
            ),
            inodes: quota_metric(
                inode_used,
                inode_soft,
                inode_hard,
                &fields[7],
                inode_marked,
                inode_soft_marked,
                inode_hard_marked,
            ),
        });
    }
    None
}

fn parse_used_token(value: &str) -> Option<(u64, bool)> {
    let (number, marked) = value
        .strip_suffix('*')
        .map_or((value, false), |value| (value, true));
    Some((number.parse().ok()?, marked))
}

fn parse_limit_token(value: &str) -> Option<(Option<u64>, bool)> {
    if value == "-" || value == "--" {
        return Some((None, false));
    }
    let (number, marked) = value
        .strip_suffix('*')
        .map_or((value, false), |value| (value, true));
    let parsed = number.parse::<u64>().ok()?;
    Some((if parsed == 0 { None } else { Some(parsed) }, marked))
}

fn quota_metric(
    used: u64,
    soft: Option<u64>,
    hard: Option<u64>,
    grace: &str,
    used_marked: bool,
    soft_marked: bool,
    hard_marked: bool,
) -> LustreQuotaMetric {
    let grace = parse_grace(grace, soft.is_none() && hard.is_none());
    let severity = if hard.is_some_and(|hard| used >= hard) || hard_marked {
        LustreQuotaSeverity::HardExceeded
    } else if soft.is_some_and(|soft| used > soft) || soft_marked || (used_marked && soft.is_some())
    {
        LustreQuotaSeverity::SoftExceeded
    } else if grace.state == LustreGraceState::Active {
        LustreQuotaSeverity::Grace
    } else {
        LustreQuotaSeverity::Normal
    };
    LustreQuotaMetric {
        used,
        soft,
        hard,
        grace,
        severity,
        marked: used_marked || soft_marked || hard_marked,
        soft_marked,
        hard_marked,
    }
}

fn parse_grace(value: &str, unlimited: bool) -> LustreGrace {
    let raw = value.to_owned();
    let normalized = value.trim().to_ascii_lowercase();
    let state = if unlimited {
        LustreGraceState::Unlimited
    } else if normalized.is_empty() || matches!(normalized.as_str(), "-" | "none" | "no") {
        LustreGraceState::None
    } else if normalized.contains("expired")
        || matches!(normalized.as_str(), "0" | "0s" | "00:00" | "00:00:00")
    {
        LustreGraceState::Expired
    } else if normalized.chars().any(|ch| ch.is_ascii_digit()) {
        LustreGraceState::Active
    } else {
        LustreGraceState::Unknown
    };
    LustreGrace { raw, state }
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

        let at_limit = parse_lustre_quota_scope("/lustre 1000 0 1000 - 1 0 0 -").unwrap();
        assert_eq!(at_limit.blocks.severity, LustreQuotaSeverity::HardExceeded);
    }

    #[test]
    fn interactive_capacity_never_starts_unverified_ssh() {
        let server = ServerConfig {
            connection_method: ConnectionMethod::Interactive,
            ..ServerConfig::default()
        };
        assert_eq!(
            ssh_capacity_output_with_connector(&server, "exit 99", None).unwrap(),
            None
        );
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

    #[test]
    fn framed_lustre_snapshot_preserves_all_scopes_and_units() {
        let status = parse_lustre_quota_snapshot(
            "@@MMQ|STATUS|LUSTRE
@@MMQ|PATH|/data/project|/data
@@MMQ|PROJECT|42
@@MMQ|IDENTITY|1000|100|alice|users
@@MMQ|BEGIN|project
warning from lfs
Filesystem kbytes quota limit grace files quota limit grace
/data 1200 500 1000 1d 3 4 5 -
@@MMQ|END|project|0
@@MMQ|BEGIN|user
/data 12 0 0 - 7 0 0 -
@@MMQ|END|user|0
@@MMQ|BEGIN|group
/data 9 8 16 - 9 10 20 00:00:00
@@MMQ|END|group|0
",
        );
        let LustreQuotaStatus::Available(details) = status else {
            panic!("expected available Lustre status");
        };
        assert_eq!(details.project_id, Some(42));
        assert_eq!(details.uid, Some(1000));
        assert_eq!(details.gid, Some(100));
        assert_eq!(details.user_name.as_deref(), Some("alice"));
        assert_eq!(details.group_name.as_deref(), Some("users"));
        let LustreQuotaScopeStatus::Available(project) = details.project else {
            panic!("project quota missing");
        };
        assert_eq!(project.blocks.used, 1200);
        assert_eq!(project.blocks.soft, Some(500));
        assert_eq!(project.blocks.hard, Some(1000));
        assert_eq!(project.blocks.grace.raw, "1d");
        assert_eq!(project.blocks.grace.state, LustreGraceState::Active);
        assert_eq!(project.blocks.severity, LustreQuotaSeverity::HardExceeded);
        assert_eq!(project.inodes.used, 3);
        assert_eq!(project.inodes.soft, Some(4));
        assert_eq!(project.inodes.hard, Some(5));
        let LustreQuotaScopeStatus::Available(user) = details.current_user else {
            panic!("user quota missing");
        };
        assert_eq!(user.blocks.hard, None);
        assert_eq!(user.blocks.grace.state, LustreGraceState::Unlimited);
        assert_eq!(user.inodes.used, 7);
        let LustreQuotaScopeStatus::Available(group) = details.primary_group else {
            panic!("group quota missing");
        };
        assert_eq!(group.blocks.severity, LustreQuotaSeverity::SoftExceeded);
        assert_eq!(group.inodes.grace.state, LustreGraceState::Expired);
    }

    #[test]
    fn quota_parser_handles_trailing_markers_wrapped_rows_and_overflow() {
        let details = parse_lustre_quota_scope(
            "lfs warning: ignored
/lustre
1200* 900 1000 - 3* 4 5 -
",
        )
        .unwrap();
        assert_eq!(details.blocks.used, 1200);
        assert!(details.blocks.marked);
        assert_eq!(details.blocks.severity, LustreQuotaSeverity::HardExceeded);
        assert!(parse_lustre_quota_scope("/lustre 184467440737095516160 0 0 - 1 0 0 -").is_none());
    }

    #[test]
    fn framed_lustre_statuses_and_partial_scope_errors_are_distinct() {
        assert!(matches!(
            parse_lustre_quota_snapshot("@@MMQ|STATUS|NOT_LUSTRE|nfs\n"),
            LustreQuotaStatus::NotLustre { .. }
        ));
        assert!(matches!(
            parse_lustre_quota_snapshot("@@MMQ|STATUS|UNAVAILABLE|lfs-missing\n"),
            LustreQuotaStatus::Unavailable {
                reason: LustreStatusReason::LfsMissing
            }
        ));
        let status = parse_lustre_quota_snapshot(
            "@@MMQ|STATUS|LUSTRE
@@MMQ|PATH|/data|/data
@@MMQ|PROJECT|1
@@MMQ|IDENTITY|100|100|u|g
@@MMQ|BEGIN|project
/data 1 2 3 - 1 2 3 -
@@MMQ|END|project|0
@@MMQ|BEGIN|user
permission denied
@@MMQ|END|user|1
@@MMQ|BEGIN|group
/data 1 0 0 - 1 0 0 -
@@MMQ|END|group|0
",
        );
        let LustreQuotaStatus::Available(details) = status else {
            panic!("expected available status");
        };
        assert!(matches!(
            details.current_user,
            LustreQuotaScopeStatus::Unavailable {
                reason: LustreStatusReason::QuotaUnavailable(_)
            }
        ));
        assert!(matches!(
            details.project,
            LustreQuotaScopeStatus::Available(_)
        ));
        assert!(matches!(
            details.primary_group,
            LustreQuotaScopeStatus::Available(_)
        ));
        assert!(matches!(
            parse_lustre_quota_snapshot("garbage output\n"),
            LustreQuotaStatus::Unavailable {
                reason: LustreStatusReason::InvalidOutput(_)
            }
        ));
    }

    #[test]
    fn missing_project_id_does_not_hide_user_or_group_quota() {
        let status = parse_lustre_quota_snapshot(
            "@@MMQ|STATUS|LUSTRE
@@MMQ|PATH|/data/home/alice|/data
@@MMQ|PROJECT|
@@MMQ|IDENTITY|1000|100|alice|users
@@MMQ|BEGIN|project
project ID is unavailable
@@MMQ|END|project|65
@@MMQ|BEGIN|user
/data 10 20 30 - 4 5 6 -
@@MMQ|END|user|0
@@MMQ|BEGIN|group
/data 7 8 9 - 1 2 3 -
@@MMQ|END|group|0
",
        );
        let LustreQuotaStatus::Available(details) = status else {
            panic!("expected available Lustre status");
        };
        assert_eq!(details.project_id, None);
        assert!(matches!(
            details.project,
            LustreQuotaScopeStatus::Unavailable {
                reason: LustreStatusReason::ProjectIdMissing
            }
        ));
        assert!(matches!(
            details.current_user,
            LustreQuotaScopeStatus::Available(_)
        ));
        assert!(matches!(
            details.primary_group,
            LustreQuotaScopeStatus::Available(_)
        ));
    }
}
