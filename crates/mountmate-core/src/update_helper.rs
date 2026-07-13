use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use thiserror::Error;
use uuid::Uuid;

use crate::rclone_binary::file_sha256;
use crate::storage::{StorageError, read_json};
use crate::update_install::{
    AppliedUpdate, InstallLayout, PreparedPayload, TransactionPaths, apply_prepared_update,
    commit_applied_update, rename_no_replace, rollback_applied_update, transaction_shape_is_valid,
};

const UPDATE_PLAN_SCHEMA: u32 = 1;
const MAX_UPDATE_PLAN_BYTES: u64 = 1024 * 1024;
const MAX_RELAUNCH_ARGUMENTS: usize = 32;
const MAX_RELAUNCH_ARGUMENT_BYTES: usize = 16 * 1024;
const DEFAULT_PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
const UPDATE_HEALTH_SCHEMA: u32 = 1;

#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentProcessIdentity {
    pub pid: u32,
    pub started_at: u64,
    pub executable: PathBuf,
    pub executable_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateHelperPlan {
    pub schema: u32,
    pub token_sha256: String,
    pub parent: ParentProcessIdentity,
    pub layout: InstallLayout,
    pub prepared: PreparedPayload,
    pub transaction: TransactionPaths,
    pub health_marker: PathBuf,
    pub health_token: String,
    pub relaunch_arguments: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct UpdateHelperAuthorization {
    pub plan_path: PathBuf,
    pub token: String,
}

impl std::fmt::Debug for UpdateHelperAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UpdateHelperAuthorization")
            .field("plan_path", &self.plan_path)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UpdateHealthAuthorization {
    pub marker_path: PathBuf,
    pub token: String,
}

impl std::fmt::Debug for UpdateHealthAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UpdateHealthAuthorization")
            .field("marker_path", &self.marker_path)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateHealthMarker {
    schema: u32,
    token_sha256: String,
}

#[derive(Debug, Error)]
pub enum UpdateHelperError {
    #[error("update helper plan is not a regular private file: {0}")]
    InvalidPlanFile(PathBuf),
    #[error("update helper plan is too large: {0}")]
    PlanTooLarge(PathBuf),
    #[error("update helper plan schema is unsupported")]
    UnsupportedSchema,
    #[error("update helper authorization was rejected")]
    Unauthorized,
    #[error("update helper plan contains an invalid digest or token")]
    InvalidPlan,
    #[error("update helper state path already exists: {0}")]
    StatePathExists(PathBuf),
    #[error("update helper destination contains unexpected content: {0}")]
    HelperCollision(PathBuf),
    #[error("update helper source is not a regular executable file: {0}")]
    InvalidHelperSource(PathBuf),
    #[error("the update helper executable failed SHA-256 verification")]
    HelperDigestMismatch,
    #[error("the update helper must run from outside the installation being replaced")]
    HelperNotDetached,
    #[error("the current process identity could not be verified")]
    CurrentProcessIdentity,
    #[error("timed out waiting for the parent application process to exit")]
    ParentExitTimeout,
    #[error("the authenticated update plan could not be removed before installation")]
    PlanCleanup,
    #[error("the update helper could not start {path}: {source}")]
    Launch {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the new application exited before reporting healthy: {0}")]
    NewApplicationExited(ExitStatus),
    #[error("timed out waiting for the new application to report healthy")]
    HealthTimeout,
    #[error("the new application wrote an invalid update health marker")]
    InvalidHealthMarker,
    #[error("the prepared update could not be installed: {0}")]
    InstallFailed(String),
    #[error("the healthy update was installed but its backup could not be removed: {0}")]
    CommitFailed(String),
    #[error("the update failed and the previous version was restored: {0}")]
    UpdateRolledBack(String),
    #[error("the update failed and rollback also failed: update: {update}; rollback: {rollback}")]
    RollbackFailed { update: String, rollback: String },
    #[error("the previous version was restored but could not be restarted: {0}")]
    RestoredRelaunchFailed(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("update helper I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn capture_current_process_identity(
    executable: &Path,
) -> Result<ParentProcessIdentity, UpdateHelperError> {
    let executable = executable
        .canonicalize()
        .map_err(|source| UpdateHelperError::Io {
            path: executable.to_owned(),
            source,
        })?;
    let mut probe = SystemProcessIdentityProbe;
    let Some(snapshot) = probe.snapshot(std::process::id()) else {
        return Err(UpdateHelperError::CurrentProcessIdentity);
    };
    let Some(process_executable) = snapshot.executable else {
        return Err(UpdateHelperError::CurrentProcessIdentity);
    };
    if process_executable != executable {
        return Err(UpdateHelperError::CurrentProcessIdentity);
    }
    let executable_sha256 =
        file_sha256(&executable).map_err(|_| UpdateHelperError::CurrentProcessIdentity)?;
    Ok(ParentProcessIdentity {
        pid: std::process::id(),
        started_at: snapshot.started_at,
        executable,
        executable_sha256,
    })
}

pub fn wait_for_parent_exit(
    parent: &ParentProcessIdentity,
    timeout: Duration,
) -> Result<(), UpdateHelperError> {
    wait_for_parent_exit_with_probe(
        parent,
        timeout,
        Duration::from_millis(100),
        &mut SystemProcessIdentityProbe,
    )
}

pub fn launch_update_helper(
    helper_executable: &Path,
    authorization: &UpdateHelperAuthorization,
) -> Result<u32, UpdateHelperError> {
    let mut command = detached_command(helper_executable);
    command
        .arg("--run-update-helper")
        .arg(&authorization.plan_path)
        .arg("--update-helper-token")
        .arg(&authorization.token);
    command
        .spawn()
        .map(|child| child.id())
        .map_err(|source| UpdateHelperError::Launch {
            path: helper_executable.to_owned(),
            source,
        })
}

pub fn run_update_helper(
    plan_path: &Path,
    token: &str,
    helper_executable: &Path,
) -> Result<(), UpdateHelperError> {
    let plan = load_authenticated_plan(plan_path, token)?;
    verify_running_helper(&plan, helper_executable)?;
    fs::remove_file(plan_path).map_err(|_| UpdateHelperError::PlanCleanup)?;
    wait_for_parent_exit(&plan.parent, DEFAULT_PARENT_EXIT_TIMEOUT)?;
    verify_authorized_installation(&plan)?;
    let applied = apply_prepared_update(&plan.layout, &plan.prepared, &plan.transaction)
        .map_err(|error| UpdateHelperError::InstallFailed(error.to_string()))?;
    complete_applied_update(
        &plan,
        &applied,
        DEFAULT_HEALTH_TIMEOUT,
        &mut SystemUpdateLauncher,
    )
}

fn verify_authorized_installation(plan: &UpdateHelperPlan) -> Result<(), UpdateHelperError> {
    let current_digest = file_sha256(&plan.layout.executable)
        .map_err(|error| UpdateHelperError::InstallFailed(error.to_string()))?;
    if current_digest == plan.parent.executable_sha256 {
        Ok(())
    } else {
        Err(UpdateHelperError::InstallFailed(
            "the installation changed after the update was authorized".into(),
        ))
    }
}

pub fn write_update_health_marker(
    state_directory: &Path,
    authorization: &UpdateHealthAuthorization,
) -> Result<(), UpdateHelperError> {
    if !valid_token(&authorization.token)
        || authorization.marker_path.parent() != Some(state_directory)
    {
        return Err(UpdateHelperError::InvalidPlan);
    }
    reject_existing_path(&authorization.marker_path)?;
    let marker = UpdateHealthMarker {
        schema: UPDATE_HEALTH_SCHEMA,
        token_sha256: sha256_text(&authorization.token),
    };
    write_private_new_json(&authorization.marker_path, &marker)?;
    Ok(())
}

pub fn materialize_update_helper(
    helper_directory: &Path,
    source_executable: &Path,
) -> Result<PathBuf, UpdateHelperError> {
    let metadata =
        fs::symlink_metadata(source_executable).map_err(|source| UpdateHelperError::Io {
            path: source_executable.to_owned(),
            source,
        })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(UpdateHelperError::InvalidHelperSource(
            source_executable.to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(UpdateHelperError::InvalidHelperSource(
                source_executable.to_owned(),
            ));
        }
    }
    let expected_digest =
        file_sha256(source_executable).map_err(|_| UpdateHelperError::HelperDigestMismatch)?;
    fs::create_dir_all(helper_directory).map_err(|source| UpdateHelperError::Io {
        path: helper_directory.to_owned(),
        source,
    })?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let target = helper_directory.join(format!(
        "SSHMountMate-updater-{}{}",
        &expected_digest[..16],
        suffix
    ));
    if path_entry_exists(&target)? {
        return if regular_file_digest_matches(&target, &expected_digest) {
            Ok(target)
        } else {
            Err(UpdateHelperError::HelperCollision(target))
        };
    }

    let temporary = helper_directory.join(format!(
        ".SSHMountMate-updater-{}{}",
        Uuid::new_v4().simple(),
        suffix
    ));
    let result = (|| {
        copy_new_file(source_executable, &temporary, &metadata)?;
        if !regular_file_digest_matches(&temporary, &expected_digest) {
            return Err(UpdateHelperError::HelperDigestMismatch);
        }
        match rename_no_replace(&temporary, &target) {
            Ok(()) => Ok(target.clone()),
            Err(source) => {
                if path_entry_exists(&target)? {
                    if regular_file_digest_matches(&target, &expected_digest) {
                        Ok(target.clone())
                    } else {
                        Err(UpdateHelperError::HelperCollision(target.clone()))
                    }
                } else {
                    Err(UpdateHelperError::Io {
                        path: target.clone(),
                        source,
                    })
                }
            }
        }
    })();
    let _ = fs::remove_file(&temporary);
    result
}

pub fn verify_running_helper(
    plan: &UpdateHelperPlan,
    helper_executable: &Path,
) -> Result<PathBuf, UpdateHelperError> {
    let helper_executable =
        helper_executable
            .canonicalize()
            .map_err(|source| UpdateHelperError::Io {
                path: helper_executable.to_owned(),
                source,
            })?;
    if helper_executable == plan.parent.executable {
        return Err(UpdateHelperError::HelperNotDetached);
    }
    if !regular_file_digest_matches(&helper_executable, &plan.parent.executable_sha256) {
        return Err(UpdateHelperError::HelperDigestMismatch);
    }
    Ok(helper_executable)
}

pub fn write_update_plan(
    state_directory: &Path,
    parent: ParentProcessIdentity,
    layout: InstallLayout,
    prepared: PreparedPayload,
    transaction: TransactionPaths,
    relaunch_arguments: Vec<String>,
) -> Result<UpdateHelperAuthorization, UpdateHelperError> {
    fs::create_dir_all(state_directory).map_err(|source| UpdateHelperError::Io {
        path: state_directory.to_owned(),
        source,
    })?;
    let id = Uuid::new_v4().simple().to_string();
    let plan_path = state_directory.join(format!("plan-{id}.json"));
    let health_marker = state_directory.join(format!("health-{id}.json"));
    reject_existing_path(&plan_path)?;
    reject_existing_path(&health_marker)?;

    let token = random_token();
    let plan = UpdateHelperPlan {
        schema: UPDATE_PLAN_SCHEMA,
        token_sha256: sha256_text(&token),
        parent,
        layout,
        prepared,
        transaction,
        health_marker,
        health_token: random_token(),
        relaunch_arguments,
    };
    validate_plan_fields(&plan)?;
    write_private_new_json(&plan_path, &plan)?;
    Ok(UpdateHelperAuthorization { plan_path, token })
}

pub fn load_authenticated_plan(
    plan_path: &Path,
    token: &str,
) -> Result<UpdateHelperPlan, UpdateHelperError> {
    let metadata = fs::symlink_metadata(plan_path).map_err(|source| UpdateHelperError::Io {
        path: plan_path.to_owned(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(UpdateHelperError::InvalidPlanFile(plan_path.to_owned()));
    }
    if metadata.len() > MAX_UPDATE_PLAN_BYTES {
        return Err(UpdateHelperError::PlanTooLarge(plan_path.to_owned()));
    }
    let plan: UpdateHelperPlan = read_json(plan_path)?;
    if plan.schema != UPDATE_PLAN_SCHEMA {
        return Err(UpdateHelperError::UnsupportedSchema);
    }
    validate_plan_fields(&plan)?;
    if plan.health_marker.parent() != plan_path.parent()
        || !plan
            .health_marker
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with("health-") && value.ends_with(".json"))
    {
        return Err(UpdateHelperError::InvalidPlan);
    }
    if sha256_text(token) != plan.token_sha256 {
        return Err(UpdateHelperError::Unauthorized);
    }
    Ok(plan)
}

fn validate_plan_fields(plan: &UpdateHelperPlan) -> Result<(), UpdateHelperError> {
    let arguments_size = plan
        .relaunch_arguments
        .iter()
        .map(String::len)
        .sum::<usize>();
    if !valid_sha256(&plan.token_sha256)
        || !valid_sha256(&plan.parent.executable_sha256)
        || !valid_sha256(&plan.prepared.executable_sha256)
        || !valid_token(&plan.health_token)
        || plan.parent.pid == 0
        || plan.parent.executable.as_os_str().is_empty()
        || plan.health_marker.as_os_str().is_empty()
        || plan.parent.executable != plan.layout.executable
        || plan.prepared.replace_path != plan.transaction.prepared
        || !plan
            .prepared
            .executable
            .starts_with(&plan.prepared.replace_path)
        || !transaction_shape_is_valid(&plan.layout, &plan.transaction)
        || plan.relaunch_arguments.len() > MAX_RELAUNCH_ARGUMENTS
        || arguments_size > MAX_RELAUNCH_ARGUMENT_BYTES
        || plan.relaunch_arguments.iter().any(|argument| {
            argument.contains('\0')
                || matches!(
                    argument.as_str(),
                    "--run-update-helper" | "--update-health-token"
                )
        })
    {
        return Err(UpdateHelperError::InvalidPlan);
    }
    Ok(())
}

fn reject_existing_path(path: &Path) -> Result<(), UpdateHelperError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(UpdateHelperError::StatePathExists(path.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(UpdateHelperError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn write_private_new_json<T: Serialize>(path: &Path, value: &T) -> Result<(), UpdateHelperError> {
    let content = serde_json::to_vec_pretty(value).map_err(|source| {
        UpdateHelperError::Storage(StorageError::Json {
            path: path.to_owned(),
            source,
        })
    })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| UpdateHelperError::Io {
        path: parent.to_owned(),
        source,
    })?;
    let mut file = match OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            return Err(UpdateHelperError::StatePathExists(path.to_owned()));
        }
        Err(source) => {
            return Err(UpdateHelperError::Io {
                path: path.to_owned(),
                source,
            });
        }
    };
    let result = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&content)?;
        file.flush()?;
        file.sync_all()
    })();
    if let Err(source) = result {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(UpdateHelperError::Io {
            path: path.to_owned(),
            source,
        });
    }
    Ok(())
}

fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn valid_token(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn path_entry_exists(path: &Path) -> Result<bool, UpdateHelperError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(UpdateHelperError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn regular_file_digest_matches(path: &Path, expected: &str) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && file_sha256(path).is_ok_and(|actual| actual == expected)
    })
}

fn copy_new_file(
    source: &Path,
    destination: &Path,
    metadata: &fs::Metadata,
) -> Result<(), UpdateHelperError> {
    let input = File::open(source).map_err(|source_error| UpdateHelperError::Io {
        path: source.to_owned(),
        source: source_error,
    })?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|source_error| UpdateHelperError::Io {
            path: destination.to_owned(),
            source: source_error,
        })?;
    let result = (|| {
        let mut limited = input.take(metadata.len().saturating_add(1));
        let copied =
            io::copy(&mut limited, &mut output).map_err(|source_error| UpdateHelperError::Io {
                path: destination.to_owned(),
                source: source_error,
            })?;
        if copied != metadata.len() {
            return Err(UpdateHelperError::Io {
                path: source.to_owned(),
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "helper source changed while copying",
                ),
            });
        }
        output
            .flush()
            .map_err(|source_error| UpdateHelperError::Io {
                path: destination.to_owned(),
                source: source_error,
            })?;
        output
            .sync_all()
            .map_err(|source_error| UpdateHelperError::Io {
                path: destination.to_owned(),
                source: source_error,
            })?;
        fs::set_permissions(destination, metadata.permissions()).map_err(|source_error| {
            UpdateHelperError::Io {
                path: destination.to_owned(),
                source: source_error,
            }
        })?;
        Ok(())
    })();
    if result.is_err() {
        drop(output);
        let _ = fs::remove_file(destination);
    }
    result
}

trait UpdateChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    fn terminate(&mut self) -> io::Result<()>;
}

trait UpdateLauncher {
    fn launch(
        &mut self,
        executable: &Path,
        arguments: &[String],
    ) -> Result<Box<dyn UpdateChild>, UpdateHelperError>;
}

struct SystemUpdateLauncher;

impl UpdateLauncher for SystemUpdateLauncher {
    fn launch(
        &mut self,
        executable: &Path,
        arguments: &[String],
    ) -> Result<Box<dyn UpdateChild>, UpdateHelperError> {
        let mut command = detached_command(executable);
        command.args(arguments);
        command
            .spawn()
            .map(|child| Box::new(child) as Box<dyn UpdateChild>)
            .map_err(|source| UpdateHelperError::Launch {
                path: executable.to_owned(),
                source,
            })
    }
}

impl UpdateChild for Child {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }

    fn terminate(&mut self) -> io::Result<()> {
        match self.try_wait()? {
            Some(_) => Ok(()),
            None => {
                self.kill()?;
                self.wait().map(|_| ())
            }
        }
    }
}

fn detached_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    #[cfg(unix)]
    command.process_group(0);
    command
}

fn complete_applied_update(
    plan: &UpdateHelperPlan,
    applied: &AppliedUpdate,
    health_timeout: Duration,
    launcher: &mut dyn UpdateLauncher,
) -> Result<(), UpdateHelperError> {
    let mut arguments = plan.relaunch_arguments.clone();
    arguments.push("--update-health-marker".into());
    arguments.push(plan.health_marker.to_string_lossy().into_owned());
    arguments.push("--update-health-token".into());
    arguments.push(plan.health_token.clone());

    let mut child = match launcher.launch(&applied.executable, &arguments) {
        Ok(child) => child,
        Err(error) => return rollback_and_relaunch(plan, applied, error.to_string(), launcher),
    };
    let deadline = Instant::now() + health_timeout;
    loop {
        let marker_exists = match path_entry_exists(&plan.health_marker) {
            Ok(exists) => exists,
            Err(error) => {
                let _ = child.terminate();
                return rollback_and_relaunch(plan, applied, error.to_string(), launcher);
            }
        };
        let child_status = match child.try_wait() {
            Ok(status) => status,
            Err(source) => {
                let error = UpdateHelperError::Io {
                    path: applied.executable.clone(),
                    source,
                };
                let _ = child.terminate();
                return rollback_and_relaunch(plan, applied, error.to_string(), launcher);
            }
        };
        if let Some(status) = child_status {
            return rollback_and_relaunch(
                plan,
                applied,
                UpdateHelperError::NewApplicationExited(status).to_string(),
                launcher,
            );
        }
        if marker_exists {
            match validate_health_marker(&plan.health_marker, &plan.health_token) {
                Ok(()) => {
                    commit_applied_update(applied)
                        .map_err(|error| UpdateHelperError::CommitFailed(error.to_string()))?;
                    let _ = fs::remove_file(&plan.health_marker);
                    return Ok(());
                }
                Err(error) => {
                    let _ = child.terminate();
                    return rollback_and_relaunch(plan, applied, error.to_string(), launcher);
                }
            }
        }
        if Instant::now() >= deadline {
            let failure = UpdateHelperError::HealthTimeout.to_string();
            let _ = child.terminate();
            return rollback_and_relaunch(plan, applied, failure, launcher);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn validate_health_marker(path: &Path, token: &str) -> Result<(), UpdateHelperError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| UpdateHelperError::InvalidHealthMarker)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 4096 {
        return Err(UpdateHelperError::InvalidHealthMarker);
    }
    let marker: UpdateHealthMarker =
        read_json(path).map_err(|_| UpdateHelperError::InvalidHealthMarker)?;
    if marker.schema != UPDATE_HEALTH_SCHEMA || marker.token_sha256 != sha256_text(token) {
        return Err(UpdateHelperError::InvalidHealthMarker);
    }
    Ok(())
}

fn rollback_and_relaunch(
    plan: &UpdateHelperPlan,
    applied: &AppliedUpdate,
    update_error: String,
    launcher: &mut dyn UpdateLauncher,
) -> Result<(), UpdateHelperError> {
    rollback_applied_update(applied).map_err(|rollback| UpdateHelperError::RollbackFailed {
        update: update_error.clone(),
        rollback: rollback.to_string(),
    })?;
    let _ = remove_path_entry_if_present(&applied.failed_payload);
    launcher
        .launch(&applied.executable, &plan.relaunch_arguments)
        .map_err(|error| UpdateHelperError::RestoredRelaunchFailed(error.to_string()))?;
    Err(UpdateHelperError::UpdateRolledBack(update_error))
}

fn remove_path_entry_if_present(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentitySnapshot {
    started_at: u64,
    executable: Option<PathBuf>,
}

trait ProcessIdentityProbe {
    fn snapshot(&mut self, pid: u32) -> Option<ProcessIdentitySnapshot>;
}

struct SystemProcessIdentityProbe;

impl ProcessIdentityProbe for SystemProcessIdentityProbe {
    fn snapshot(&mut self, pid: u32) -> Option<ProcessIdentitySnapshot> {
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        let process = system.process(pid)?;
        Some(ProcessIdentitySnapshot {
            started_at: process.start_time(),
            executable: process.exe().and_then(|path| path.canonicalize().ok()),
        })
    }
}

fn wait_for_parent_exit_with_probe(
    parent: &ParentProcessIdentity,
    timeout: Duration,
    poll_interval: Duration,
    probe: &mut dyn ProcessIdentityProbe,
) -> Result<(), UpdateHelperError> {
    let deadline = Instant::now() + timeout;
    loop {
        match probe.snapshot(parent.pid) {
            None => return Ok(()),
            Some(snapshot) if snapshot.started_at != parent.started_at => return Ok(()),
            Some(snapshot)
                if snapshot
                    .executable
                    .as_ref()
                    .is_some_and(|executable| executable != &parent.executable) =>
            {
                return Ok(());
            }
            Some(_) => {}
        }
        if Instant::now() >= deadline {
            return Err(UpdateHelperError::ParentExitTimeout);
        }
        thread::sleep(poll_interval.min(deadline.saturating_duration_since(Instant::now())));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;

    use tempfile::tempdir;

    use super::*;
    use crate::update_install::{InstallKind, InstallLayout};

    struct FakeProbe {
        snapshots: VecDeque<Option<ProcessIdentitySnapshot>>,
        fallback: Option<ProcessIdentitySnapshot>,
    }

    struct FakeUpdateChild {
        terminated: Rc<Cell<bool>>,
    }

    type RecordedLaunches = Rc<RefCell<Vec<(PathBuf, Vec<String>)>>>;

    impl UpdateChild for FakeUpdateChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            Ok(None)
        }

        fn terminate(&mut self) -> io::Result<()> {
            self.terminated.set(true);
            Ok(())
        }
    }

    struct FakeUpdateLauncher {
        launches: RecordedLaunches,
        terminated: Rc<Cell<bool>>,
        health: Option<UpdateHealthAuthorization>,
    }

    impl UpdateLauncher for FakeUpdateLauncher {
        fn launch(
            &mut self,
            executable: &Path,
            arguments: &[String],
        ) -> Result<Box<dyn UpdateChild>, UpdateHelperError> {
            self.launches
                .borrow_mut()
                .push((executable.to_owned(), arguments.to_vec()));
            if let Some(health) = self.health.take() {
                let state_directory = health.marker_path.parent().unwrap().to_owned();
                write_update_health_marker(&state_directory, &health)?;
            }
            Ok(Box::new(FakeUpdateChild {
                terminated: self.terminated.clone(),
            }))
        }
    }

    impl ProcessIdentityProbe for FakeProbe {
        fn snapshot(&mut self, _pid: u32) -> Option<ProcessIdentitySnapshot> {
            self.snapshots
                .pop_front()
                .unwrap_or_else(|| self.fallback.clone())
        }
    }

    fn parent() -> ParentProcessIdentity {
        ParentProcessIdentity {
            pid: 42,
            started_at: 123,
            executable: PathBuf::from("/app/SSHMountMate"),
            executable_sha256: "a".repeat(64),
        }
    }

    fn layout() -> InstallLayout {
        InstallLayout {
            kind: InstallKind::StandaloneExecutable,
            executable: PathBuf::from("/app/SSHMountMate"),
            replace_path: PathBuf::from("/app/SSHMountMate"),
        }
    }

    fn prepared() -> PreparedPayload {
        let replace_path = PathBuf::from(format!(
            "/app/.SSHMountMate.ssh-mountmate-prepared-{}",
            "c".repeat(32)
        ));
        PreparedPayload {
            executable: replace_path.clone(),
            replace_path,
            executable_sha256: "b".repeat(64),
        }
    }

    fn transaction() -> TransactionPaths {
        TransactionPaths {
            prepared: prepared().replace_path,
            backup: PathBuf::from("/app/.SSHMountMate.ssh-mountmate-backup"),
        }
    }

    fn create_test_executable(path: &Path, content: &[u8]) {
        fs::write(path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    fn applied_standalone_update() -> (tempfile::TempDir, UpdateHelperPlan, AppliedUpdate, PathBuf)
    {
        #[cfg(windows)]
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        #[cfg(not(windows))]
        let temp = tempdir().unwrap();
        let executable = temp.path().join(if cfg!(windows) {
            "SSHMountMate.exe"
        } else {
            "SSHMountMate"
        });
        create_test_executable(&executable, b"old executable");
        let layout = crate::update_install::detect_install_layout(&executable).unwrap();
        let transaction = crate::update_install::plan_transaction_paths(&layout).unwrap();
        create_test_executable(&transaction.prepared, b"new executable");
        let prepared = PreparedPayload {
            replace_path: transaction.prepared.clone(),
            executable: transaction.prepared.clone(),
            executable_sha256: file_sha256(&transaction.prepared).unwrap(),
        };
        let health_marker = temp.path().join("health-update.json");
        let plan = UpdateHelperPlan {
            schema: UPDATE_PLAN_SCHEMA,
            token_sha256: "a".repeat(64),
            parent: ParentProcessIdentity {
                executable: layout.executable.clone(),
                ..parent()
            },
            layout: layout.clone(),
            prepared: prepared.clone(),
            transaction: transaction.clone(),
            health_marker,
            health_token: "b".repeat(64),
            relaunch_arguments: vec!["--show-main".into()],
        };
        let applied = apply_prepared_update(&layout, &prepared, &transaction).unwrap();
        (temp, plan, applied, executable)
    }

    #[test]
    fn private_plan_round_trip_requires_the_original_random_token() {
        let temp = tempdir().unwrap();
        let authorization = write_update_plan(
            temp.path(),
            parent(),
            layout(),
            prepared(),
            transaction(),
            vec!["--show-main".into()],
        )
        .unwrap();

        let plan = load_authenticated_plan(&authorization.plan_path, &authorization.token).unwrap();
        assert_eq!(plan.parent, parent());
        assert_eq!(plan.relaunch_arguments, vec!["--show-main"]);
        assert_ne!(plan.token_sha256, authorization.token);
        assert!(matches!(
            load_authenticated_plan(&authorization.plan_path, &"0".repeat(64)),
            Err(UpdateHelperError::Unauthorized)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn plan_file_is_private_and_symlink_plans_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temp = tempdir().unwrap();
        let authorization = write_update_plan(
            temp.path(),
            parent(),
            layout(),
            prepared(),
            transaction(),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            fs::metadata(&authorization.plan_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let link = temp.path().join("linked-plan.json");
        symlink(&authorization.plan_path, &link).unwrap();
        assert!(matches!(
            load_authenticated_plan(&link, &authorization.token),
            Err(UpdateHelperError::InvalidPlanFile(_))
        ));
    }

    #[test]
    fn unknown_plan_fields_are_rejected() {
        let temp = tempdir().unwrap();
        let authorization = write_update_plan(
            temp.path(),
            parent(),
            layout(),
            prepared(),
            transaction(),
            Vec::new(),
        )
        .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&authorization.plan_path).unwrap()).unwrap();
        value["unexpected"] = serde_json::Value::Bool(true);
        fs::write(
            &authorization.plan_path,
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            load_authenticated_plan(&authorization.plan_path, &authorization.token),
            Err(UpdateHelperError::Storage(StorageError::Json { .. }))
        ));
    }

    #[test]
    fn plan_paths_and_relaunch_arguments_cannot_escape_the_helper_protocol() {
        let temp = tempdir().unwrap();
        assert!(matches!(
            write_update_plan(
                temp.path(),
                parent(),
                layout(),
                prepared(),
                transaction(),
                vec!["--run-update-helper".into()],
            ),
            Err(UpdateHelperError::InvalidPlan)
        ));

        let authorization = write_update_plan(
            temp.path(),
            parent(),
            layout(),
            prepared(),
            transaction(),
            Vec::new(),
        )
        .unwrap();
        let mut plan: UpdateHelperPlan = read_json(&authorization.plan_path).unwrap();
        plan.health_marker = temp.path().join("../outside-health.json");
        crate::storage::write_private_json(&authorization.plan_path, &plan).unwrap();
        assert!(matches!(
            load_authenticated_plan(&authorization.plan_path, &authorization.token),
            Err(UpdateHelperError::InvalidPlan)
        ));
    }

    #[test]
    fn parent_wait_requires_exit_or_a_changed_process_identity() {
        let parent = parent();
        let same_but_unverifiable = ProcessIdentitySnapshot {
            started_at: parent.started_at,
            executable: None,
        };
        let mut probe = FakeProbe {
            snapshots: VecDeque::from([Some(same_but_unverifiable), None]),
            fallback: None,
        };
        wait_for_parent_exit_with_probe(
            &parent,
            Duration::from_secs(1),
            Duration::ZERO,
            &mut probe,
        )
        .unwrap();

        let mut reused_pid = FakeProbe {
            snapshots: VecDeque::new(),
            fallback: Some(ProcessIdentitySnapshot {
                started_at: parent.started_at + 1,
                executable: Some(parent.executable.clone()),
            }),
        };
        wait_for_parent_exit_with_probe(
            &parent,
            Duration::from_secs(1),
            Duration::ZERO,
            &mut reused_pid,
        )
        .unwrap();
    }

    #[test]
    fn unverifiable_live_parent_times_out_instead_of_allowing_swap() {
        let parent = parent();
        let mut probe = FakeProbe {
            snapshots: VecDeque::new(),
            fallback: Some(ProcessIdentitySnapshot {
                started_at: parent.started_at,
                executable: None,
            }),
        };

        assert!(matches!(
            wait_for_parent_exit_with_probe(&parent, Duration::ZERO, Duration::ZERO, &mut probe,),
            Err(UpdateHelperError::ParentExitTimeout)
        ));
    }

    #[test]
    fn helper_binary_is_content_addressed_verified_and_reused() {
        let temp = tempdir().unwrap();
        let source = temp.path().join(if cfg!(windows) {
            "SSHMountMate.exe"
        } else {
            "SSHMountMate"
        });
        create_test_executable(&source, b"helper executable");
        let helper_directory = temp.path().join("helpers");

        let first = materialize_update_helper(&helper_directory, &source).unwrap();
        let second = materialize_update_helper(&helper_directory, &source).unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"helper executable");
        assert_ne!(first, source);

        fs::write(&first, b"corrupt helper").unwrap();
        assert!(matches!(
            materialize_update_helper(&helper_directory, &source),
            Err(UpdateHelperError::HelperCollision(_))
        ));
        assert_eq!(fs::read(first).unwrap(), b"corrupt helper");
    }

    #[test]
    fn running_helper_must_be_a_detached_copy_of_the_parent_binary() {
        let temp = tempdir().unwrap();
        let source = temp.path().join(if cfg!(windows) {
            "SSHMountMate.exe"
        } else {
            "SSHMountMate"
        });
        create_test_executable(&source, b"helper executable");
        let helper = materialize_update_helper(&temp.path().join("helpers"), &source).unwrap();
        let mut plan_parent = parent();
        plan_parent.executable = source.canonicalize().unwrap();
        plan_parent.executable_sha256 = file_sha256(&source).unwrap();
        let plan = UpdateHelperPlan {
            schema: UPDATE_PLAN_SCHEMA,
            token_sha256: "a".repeat(64),
            parent: plan_parent,
            layout: layout(),
            prepared: prepared(),
            transaction: transaction(),
            health_marker: temp.path().join("health.json"),
            health_token: "b".repeat(64),
            relaunch_arguments: Vec::new(),
        };

        assert_eq!(
            verify_running_helper(&plan, &helper).unwrap(),
            helper.canonicalize().unwrap()
        );
        assert!(matches!(
            verify_running_helper(&plan, &source),
            Err(UpdateHelperError::HelperNotDetached)
        ));
    }

    #[test]
    fn healthy_relaunch_commits_the_new_executable_and_removes_the_backup() {
        let (_temp, plan, applied, executable) = applied_standalone_update();
        let launches = Rc::new(RefCell::new(Vec::new()));
        let terminated = Rc::new(Cell::new(false));
        let mut launcher = FakeUpdateLauncher {
            launches: launches.clone(),
            terminated: terminated.clone(),
            health: Some(UpdateHealthAuthorization {
                marker_path: plan.health_marker.clone(),
                token: plan.health_token.clone(),
            }),
        };

        complete_applied_update(&plan, &applied, Duration::from_secs(1), &mut launcher).unwrap();

        assert_eq!(fs::read(executable).unwrap(), b"new executable");
        assert!(!applied.backup.exists());
        assert!(!plan.health_marker.exists());
        assert!(!terminated.get());
        let launches = launches.borrow();
        assert_eq!(launches.len(), 1);
        assert!(launches[0].1.contains(&"--update-health-marker".into()));
        assert!(launches[0].1.contains(&plan.health_token));
    }

    #[test]
    fn health_timeout_terminates_the_new_version_and_restores_the_old_one() {
        let (_temp, plan, applied, executable) = applied_standalone_update();
        let launches = Rc::new(RefCell::new(Vec::new()));
        let terminated = Rc::new(Cell::new(false));
        let mut launcher = FakeUpdateLauncher {
            launches: launches.clone(),
            terminated: terminated.clone(),
            health: None,
        };

        assert!(matches!(
            complete_applied_update(&plan, &applied, Duration::ZERO, &mut launcher),
            Err(UpdateHelperError::UpdateRolledBack(_))
        ));

        assert!(terminated.get());
        assert_eq!(fs::read(executable).unwrap(), b"old executable");
        assert!(!applied.backup.exists());
        assert!(!applied.failed_payload.exists());
        let launches = launches.borrow();
        assert_eq!(launches.len(), 2);
        assert_eq!(launches[1].1, plan.relaunch_arguments);
    }

    #[test]
    fn health_markers_are_authenticated_and_cannot_be_reused() {
        let temp = tempdir().unwrap();
        let authorization = UpdateHealthAuthorization {
            marker_path: temp.path().join("health.json"),
            token: "c".repeat(64),
        };
        write_update_health_marker(temp.path(), &authorization).unwrap();
        validate_health_marker(&authorization.marker_path, &authorization.token).unwrap();
        assert!(matches!(
            validate_health_marker(&authorization.marker_path, &"d".repeat(64)),
            Err(UpdateHelperError::InvalidHealthMarker)
        ));
        assert!(matches!(
            write_update_health_marker(temp.path(), &authorization),
            Err(UpdateHelperError::StatePathExists(_))
        ));
        let outside = temp.path().join("outside");
        assert!(matches!(
            write_update_health_marker(
                &outside,
                &UpdateHealthAuthorization {
                    marker_path: authorization.marker_path.clone(),
                    token: authorization.token.clone(),
                }
            ),
            Err(UpdateHelperError::InvalidPlan)
        ));
    }

    #[test]
    fn installation_digest_must_still_match_the_authorized_parent() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("SSHMountMate");
        create_test_executable(&executable, b"authorized executable");
        let executable = executable.canonicalize().unwrap();
        let mut plan_parent = parent();
        plan_parent.executable = executable.clone();
        plan_parent.executable_sha256 = file_sha256(&executable).unwrap();
        let plan = UpdateHelperPlan {
            schema: UPDATE_PLAN_SCHEMA,
            token_sha256: "a".repeat(64),
            parent: plan_parent,
            layout: InstallLayout {
                kind: InstallKind::StandaloneExecutable,
                executable: executable.clone(),
                replace_path: executable.clone(),
            },
            prepared: prepared(),
            transaction: transaction(),
            health_marker: temp.path().join("health.json"),
            health_token: "b".repeat(64),
            relaunch_arguments: Vec::new(),
        };
        verify_authorized_installation(&plan).unwrap();

        fs::write(&executable, b"externally replaced").unwrap();
        assert!(matches!(
            verify_authorized_installation(&plan),
            Err(UpdateHelperError::InstallFailed(_))
        ));
    }
}
