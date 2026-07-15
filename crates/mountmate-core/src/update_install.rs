use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::rclone_binary::{file_sha256, verify_bundled};

const DIRECTORY_BUNDLE_MARKER_NAME: &str = "SSHMountMate.install-layout";
const DIRECTORY_BUNDLE_MARKER: &[u8] = b"ssh-mountmate-directory-bundle-v1\n";
const MAX_PREPARED_ENTRIES: usize = 20_000;
const MAX_PREPARED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    StandaloneExecutable,
    DirectoryBundle,
    MacApplicationBundle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallLayout {
    pub kind: InstallKind,
    pub executable: PathBuf,
    /// The file or directory replaced as one unit during an update.
    pub replace_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPayload {
    pub root: PathBuf,
    pub executable: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionPaths {
    pub prepared: PathBuf,
    pub backup: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedPayload {
    pub replace_path: PathBuf,
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub tree_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedUpdate {
    pub executable: PathBuf,
    pub target: PathBuf,
    pub failed_payload: PathBuf,
    pub backup: PathBuf,
}

#[derive(Debug, Error)]
pub enum InstallLayoutError {
    #[error("could not inspect the running executable at {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the running executable is a symbolic link, which cannot be updated safely: {0}")]
    SymlinkExecutable(PathBuf),
    #[error("the running executable is not a regular file: {0}")]
    NotExecutableFile(PathBuf),
    #[error(
        "automatic updates are disabled when running from the Windows temporary directory: {0}"
    )]
    TemporaryWindowsLocation(PathBuf),
    #[error("the executable is inside a malformed macOS application bundle: {0}")]
    MalformedMacApplication(PathBuf),
    #[error("the SSH MountMate directory-bundle marker is invalid: {0}")]
    InvalidDirectoryBundleMarker(PathBuf),
    #[error("the SSH MountMate directory bundle is incomplete: {0}")]
    IncompleteDirectoryBundle(PathBuf),
    #[error("the bundled rclone failed verification at {path}: {message}")]
    CorruptDirectoryBundle { path: PathBuf, message: String },
}

#[derive(Debug, Error)]
pub enum PayloadError {
    #[error("could not inspect the extracted update at {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the extracted update root is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("the extracted update did not contain a complete SSH MountMate directory bundle")]
    MissingDirectoryBundle,
    #[error("the extracted update contained more than one SSH MountMate directory bundle")]
    AmbiguousDirectoryBundle,
    #[error("the extracted update did not contain a standalone SSH MountMate executable")]
    MissingStandaloneExecutable,
    #[error("the extracted update contained more than one standalone SSH MountMate executable")]
    AmbiguousStandaloneExecutable,
    #[error("the update bundle does not contain the expected executable: {0}")]
    MissingExecutable(PathBuf),
    #[error("the macOS update bundle is incomplete: {0}")]
    IncompleteMacApplication(PathBuf),
    #[error("the update bundle contains a corrupt rclone at {path}: {message}")]
    CorruptRclone { path: PathBuf, message: String },
    #[error(transparent)]
    Layout(#[from] InstallLayoutError),
}

#[derive(Debug, Error)]
pub enum TransactionPlanError {
    #[error("the installation does not have a replaceable parent directory: {0}")]
    MissingParent(PathBuf),
    #[error("the installation path does not have a usable file name: {0}")]
    MissingName(PathBuf),
    #[error("a previous update backup still exists and must be recovered first: {0}")]
    BackupExists(PathBuf),
    #[error("the generated update staging path already exists: {0}")]
    PreparedExists(PathBuf),
    #[error("could not inspect update transaction path {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Error)]
pub enum PreparePayloadError {
    #[error("update transaction paths are not siblings of the current installation")]
    InvalidTransactionPaths,
    #[error("update staging path already exists: {0}")]
    PreparedExists(PathBuf),
    #[error("a previous update backup still exists: {0}")]
    BackupExists(PathBuf),
    #[error("update payload contains too many entries")]
    TooManyEntries,
    #[error("update payload exceeds the extracted-size safety limit")]
    PayloadTooLarge,
    #[error("update payload contains a symbolic link or special file: {0}")]
    UnsafeEntry(PathBuf),
    #[error("staged executable failed copy verification")]
    ExecutableDigestMismatch,
    #[error("update payload verification failed: {0}")]
    Verification(String),
    #[error("update staging I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Payload(#[from] PayloadError),
    #[error(transparent)]
    Transaction(#[from] TransactionPlanError),
}

#[derive(Debug, Error)]
pub enum ApplyUpdateError {
    #[error("update transaction paths or prepared payload are inconsistent")]
    InvalidTransaction,
    #[error("the installation changed after the update was prepared")]
    InstallationChanged,
    #[error("prepared update payload is missing or has the wrong file type: {0}")]
    InvalidPreparedPayload(PathBuf),
    #[error("prepared update payload verification failed: {0}")]
    Verification(String),
    #[error("update swap failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "update swap failed ({replace_error}) and the previous installation could not be restored ({restore_error}); backup remains at {backup}"
    )]
    RestoreFailed {
        replace_error: String,
        restore_error: String,
        backup: PathBuf,
    },
    #[error(
        "rollback could not restore the previous installation ({restore_error}); the new installation remains active and the old backup remains at {backup}"
    )]
    RollbackRestoreFailed {
        restore_error: String,
        backup: PathBuf,
    },
    #[error(
        "rollback could neither restore the previous installation ({restore_error}) nor return the new installation to its target ({recovery_error}); old backup: {backup}, new payload: {failed_payload}"
    )]
    RollbackRecoveryFailed {
        restore_error: String,
        recovery_error: String,
        backup: PathBuf,
        failed_payload: PathBuf,
    },
    #[error(transparent)]
    Layout(#[from] InstallLayoutError),
}

pub fn detect_install_layout(executable: &Path) -> Result<InstallLayout, InstallLayoutError> {
    detect_install_layout_for(executable, env::consts::OS, &env::temp_dir())
}

pub fn locate_directory_payload(
    extracted_root: &Path,
    os: &str,
) -> Result<DirectoryPayload, PayloadError> {
    let metadata =
        fs::symlink_metadata(extracted_root).map_err(|source| PayloadError::Inspect {
            path: extracted_root.to_owned(),
            source,
        })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(PayloadError::NotDirectory(extracted_root.to_owned()));
    }
    let extracted_root = extracted_root
        .canonicalize()
        .map_err(|source| PayloadError::Inspect {
            path: extracted_root.to_owned(),
            source,
        })?;

    let mut candidates = Vec::new();
    collect_directory_payload_candidate(&extracted_root, os, &mut candidates)?;
    for entry in fs::read_dir(&extracted_root).map_err(|source| PayloadError::Inspect {
        path: extracted_root.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| PayloadError::Inspect {
            path: extracted_root.clone(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| PayloadError::Inspect {
            path: entry.path(),
            source,
        })?;
        if file_type.is_dir() && !file_type.is_symlink() {
            collect_directory_payload_candidate(&entry.path(), os, &mut candidates)?;
        }
    }

    match candidates.len() {
        0 => Err(PayloadError::MissingDirectoryBundle),
        1 => Ok(candidates.remove(0)),
        _ => Err(PayloadError::AmbiguousDirectoryBundle),
    }
}

pub fn locate_update_payload(
    extracted_root: &Path,
    kind: InstallKind,
    os: &str,
) -> Result<DirectoryPayload, PayloadError> {
    match kind {
        InstallKind::StandaloneExecutable => locate_standalone_payload(extracted_root, os),
        InstallKind::DirectoryBundle | InstallKind::MacApplicationBundle => {
            locate_directory_payload(extracted_root, os)
        }
    }
}

fn locate_standalone_payload(
    extracted_root: &Path,
    os: &str,
) -> Result<DirectoryPayload, PayloadError> {
    let metadata =
        fs::symlink_metadata(extracted_root).map_err(|source| PayloadError::Inspect {
            path: extracted_root.to_owned(),
            source,
        })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(PayloadError::NotDirectory(extracted_root.to_owned()));
    }
    let extracted_root = extracted_root
        .canonicalize()
        .map_err(|source| PayloadError::Inspect {
            path: extracted_root.to_owned(),
            source,
        })?;
    let executable_name = if os == "windows" {
        "SSHMountMate.exe"
    } else {
        "SSHMountMate"
    };
    let mut candidates = Vec::new();
    collect_standalone_candidate(&extracted_root, executable_name, &mut candidates)?;
    for entry in fs::read_dir(&extracted_root).map_err(|source| PayloadError::Inspect {
        path: extracted_root.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| PayloadError::Inspect {
            path: extracted_root.clone(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| PayloadError::Inspect {
            path: entry.path(),
            source,
        })?;
        if file_type.is_dir() && !file_type.is_symlink() {
            collect_standalone_candidate(&entry.path(), executable_name, &mut candidates)?;
        }
    }
    match candidates.len() {
        0 => Err(PayloadError::MissingStandaloneExecutable),
        1 => Ok(candidates.remove(0)),
        _ => Err(PayloadError::AmbiguousStandaloneExecutable),
    }
}

fn collect_standalone_candidate(
    root: &Path,
    executable_name: &str,
    candidates: &mut Vec<DirectoryPayload>,
) -> Result<(), PayloadError> {
    let executable = root.join(executable_name);
    let metadata = match fs::symlink_metadata(&executable) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(PayloadError::Inspect {
                path: executable,
                source,
            });
        }
    };
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        candidates.push(DirectoryPayload {
            root: root
                .canonicalize()
                .map_err(|source| PayloadError::Inspect {
                    path: root.to_owned(),
                    source,
                })?,
            executable: executable
                .canonicalize()
                .map_err(|source| PayloadError::Inspect {
                    path: executable,
                    source,
                })?,
        });
    }
    Ok(())
}

pub fn plan_transaction_paths(
    layout: &InstallLayout,
) -> Result<TransactionPaths, TransactionPlanError> {
    let parent = layout
        .replace_path
        .parent()
        .ok_or_else(|| TransactionPlanError::MissingParent(layout.replace_path.clone()))?;
    let name = layout
        .replace_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| TransactionPlanError::MissingName(layout.replace_path.clone()))?;
    let backup = parent.join(transaction_file_name(name, layout.kind, "backup", None));
    if path_entry_exists(&backup)? {
        return Err(TransactionPlanError::BackupExists(backup));
    }
    let token = Uuid::new_v4().simple().to_string();
    let prepared = parent.join(transaction_file_name(
        name,
        layout.kind,
        "prepared",
        Some(&token),
    ));
    if path_entry_exists(&prepared)? {
        return Err(TransactionPlanError::PreparedExists(prepared));
    }
    Ok(TransactionPaths { prepared, backup })
}

pub fn prepare_directory_payload(
    layout: &InstallLayout,
    payload: &DirectoryPayload,
    transaction: &TransactionPaths,
    os: &str,
) -> Result<PreparedPayload, PreparePayloadError> {
    validate_transaction_paths(layout, transaction)?;
    match layout.kind {
        InstallKind::StandaloneExecutable => {
            let executable_sha256 = copy_standalone_payload(payload, &transaction.prepared)?;
            let tree_sha256 = prepared_tree_sha256(&transaction.prepared)
                .map_err(PreparePayloadError::Verification)?;
            Ok(PreparedPayload {
                replace_path: transaction.prepared.clone(),
                executable: transaction.prepared.clone(),
                executable_sha256,
                tree_sha256,
            })
        }
        InstallKind::DirectoryBundle | InstallKind::MacApplicationBundle => {
            let source_metadata = safe_source_metadata(&payload.root)?;
            if !source_metadata.is_dir() {
                return Err(PreparePayloadError::UnsafeEntry(payload.root.clone()));
            }
            fs::create_dir(&transaction.prepared).map_err(|source| PreparePayloadError::Io {
                path: transaction.prepared.clone(),
                source,
            })?;
            let result = (|| {
                let mut totals = CopyTotals::default();
                copy_directory_contents(
                    &payload.root,
                    &transaction.prepared,
                    &source_metadata,
                    &mut totals,
                )?;
                let verified = locate_directory_payload(&transaction.prepared, os)?;
                if verified.root != transaction.prepared {
                    return Err(PreparePayloadError::InvalidTransactionPaths);
                }
                let executable_sha256 =
                    verify_copied_executable(&payload.executable, &verified.executable)?;
                let tree_sha256 = prepared_tree_sha256(&transaction.prepared)
                    .map_err(PreparePayloadError::Verification)?;
                Ok(PreparedPayload {
                    replace_path: transaction.prepared.clone(),
                    executable: verified.executable,
                    executable_sha256,
                    tree_sha256,
                })
            })();
            if result.is_err() {
                let _ = fs::remove_dir_all(&transaction.prepared);
            }
            result
        }
    }
}

pub fn apply_prepared_update(
    layout: &InstallLayout,
    prepared: &PreparedPayload,
    transaction: &TransactionPaths,
) -> Result<AppliedUpdate, ApplyUpdateError> {
    apply_prepared_update_for(
        layout,
        prepared,
        transaction,
        env::consts::OS,
        &env::temp_dir(),
    )
}

fn apply_prepared_update_for(
    layout: &InstallLayout,
    prepared: &PreparedPayload,
    transaction: &TransactionPaths,
    os: &str,
    temporary_directory: &Path,
) -> Result<AppliedUpdate, ApplyUpdateError> {
    validate_transaction_shape(layout, transaction)
        .map_err(|_| ApplyUpdateError::InvalidTransaction)?;
    if prepared.replace_path != transaction.prepared
        || !prepared.executable.starts_with(&prepared.replace_path)
    {
        return Err(ApplyUpdateError::InvalidTransaction);
    }
    validate_prepared_payload(layout.kind, prepared, os)?;

    let current = detect_install_layout_for(&layout.executable, os, temporary_directory)?;
    if current != *layout {
        return Err(ApplyUpdateError::InstallationChanged);
    }
    let current_executable_sha256 = file_sha256(&layout.executable)
        .map_err(|error| ApplyUpdateError::Verification(error.to_string()))?;
    let current_executable_relative = layout
        .executable
        .strip_prefix(&layout.replace_path)
        .map_err(|_| ApplyUpdateError::InvalidTransaction)?;
    let executable_relative = prepared
        .executable
        .strip_prefix(&prepared.replace_path)
        .map_err(|_| ApplyUpdateError::InvalidTransaction)?;
    let installed_executable = rebase_path(&layout.replace_path, executable_relative);

    swap_paths(
        &layout.replace_path,
        &prepared.replace_path,
        &transaction.backup,
        || rename_no_replace(&prepared.replace_path, &layout.replace_path),
    )?;
    let applied = AppliedUpdate {
        executable: installed_executable,
        target: layout.replace_path.clone(),
        failed_payload: transaction.prepared.clone(),
        backup: transaction.backup.clone(),
    };
    let backup_executable = rebase_path(&applied.backup, current_executable_relative);
    let backup_sha256 = match file_sha256(&backup_executable) {
        Ok(digest) => digest,
        Err(error) => {
            let verification_error = ApplyUpdateError::Verification(error.to_string());
            rollback_applied_update(&applied)?;
            return Err(verification_error);
        }
    };
    if backup_sha256 != current_executable_sha256 {
        rollback_applied_update(&applied)?;
        return Err(ApplyUpdateError::InstallationChanged);
    }
    Ok(applied)
}

pub fn rollback_applied_update(applied: &AppliedUpdate) -> Result<(), ApplyUpdateError> {
    rename_no_replace(&applied.target, &applied.failed_payload).map_err(|source| {
        ApplyUpdateError::Io {
            path: applied.failed_payload.clone(),
            source,
        }
    })?;
    match rename_no_replace(&applied.backup, &applied.target) {
        Ok(()) => Ok(()),
        Err(restore_error) => match rename_no_replace(&applied.failed_payload, &applied.target) {
            Ok(()) => Err(ApplyUpdateError::RollbackRestoreFailed {
                restore_error: restore_error.to_string(),
                backup: applied.backup.clone(),
            }),
            Err(recovery_error) => Err(ApplyUpdateError::RollbackRecoveryFailed {
                restore_error: restore_error.to_string(),
                recovery_error: recovery_error.to_string(),
                backup: applied.backup.clone(),
                failed_payload: applied.failed_payload.clone(),
            }),
        },
    }
}

pub fn commit_applied_update(applied: &AppliedUpdate) -> Result<(), ApplyUpdateError> {
    remove_path_entry(&applied.backup).map_err(|source| ApplyUpdateError::Io {
        path: applied.backup.clone(),
        source,
    })
}

fn transaction_file_name(
    name: &str,
    kind: InstallKind,
    phase: &str,
    token: Option<&str>,
) -> String {
    let (prefix, suffix) = transaction_name_parts(name, kind, phase);
    match token {
        Some(token) => format!("{prefix}-{token}{suffix}"),
        None => format!("{prefix}{suffix}"),
    }
}

fn rebase_path(root: &Path, relative: &Path) -> PathBuf {
    if relative.as_os_str().is_empty() {
        root.to_owned()
    } else {
        root.join(relative)
    }
}

fn transaction_name_parts(name: &str, kind: InstallKind, phase: &str) -> (String, String) {
    if matches!(
        kind,
        InstallKind::StandaloneExecutable | InstallKind::MacApplicationBundle
    ) {
        let path = Path::new(name);
        if let (Some(stem), Some(extension)) = (
            path.file_stem().and_then(|value| value.to_str()),
            path.extension().and_then(|value| value.to_str()),
        ) {
            return (
                format!(".{stem}.ssh-mountmate-{phase}"),
                format!(".{extension}"),
            );
        }
    }
    (format!(".{name}.ssh-mountmate-{phase}"), String::new())
}

fn validate_transaction_paths(
    layout: &InstallLayout,
    transaction: &TransactionPaths,
) -> Result<(), PreparePayloadError> {
    validate_transaction_shape(layout, transaction)?;
    if path_entry_exists(&transaction.prepared)? {
        return Err(PreparePayloadError::PreparedExists(
            transaction.prepared.clone(),
        ));
    }
    if path_entry_exists(&transaction.backup)? {
        return Err(PreparePayloadError::BackupExists(
            transaction.backup.clone(),
        ));
    }
    Ok(())
}

fn validate_transaction_shape(
    layout: &InstallLayout,
    transaction: &TransactionPaths,
) -> Result<(), PreparePayloadError> {
    let Some(parent) = layout.replace_path.parent() else {
        return Err(PreparePayloadError::InvalidTransactionPaths);
    };
    if transaction.prepared.parent() != Some(parent) || transaction.backup.parent() != Some(parent)
    {
        return Err(PreparePayloadError::InvalidTransactionPaths);
    }
    let Some(name) = layout
        .replace_path
        .file_name()
        .and_then(|value| value.to_str())
    else {
        return Err(PreparePayloadError::InvalidTransactionPaths);
    };
    if transaction
        .backup
        .file_name()
        .and_then(|value| value.to_str())
        != Some(transaction_file_name(name, layout.kind, "backup", None).as_str())
        || !valid_prepared_name(
            transaction
                .prepared
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default(),
            name,
            layout.kind,
        )
    {
        return Err(PreparePayloadError::InvalidTransactionPaths);
    }
    Ok(())
}

pub(crate) fn transaction_shape_is_valid(
    layout: &InstallLayout,
    transaction: &TransactionPaths,
) -> bool {
    validate_transaction_shape(layout, transaction).is_ok()
}

fn validate_prepared_payload(
    kind: InstallKind,
    prepared: &PreparedPayload,
    os: &str,
) -> Result<(), ApplyUpdateError> {
    let metadata =
        fs::symlink_metadata(&prepared.replace_path).map_err(|source| ApplyUpdateError::Io {
            path: prepared.replace_path.clone(),
            source,
        })?;
    let expected_type = match kind {
        InstallKind::StandaloneExecutable => metadata.is_file(),
        InstallKind::DirectoryBundle | InstallKind::MacApplicationBundle => metadata.is_dir(),
    };
    if metadata.file_type().is_symlink() || !expected_type {
        return Err(ApplyUpdateError::InvalidPreparedPayload(
            prepared.replace_path.clone(),
        ));
    }
    let executable_metadata =
        fs::symlink_metadata(&prepared.executable).map_err(|source| ApplyUpdateError::Io {
            path: prepared.executable.clone(),
            source,
        })?;
    if executable_metadata.file_type().is_symlink() || !executable_metadata.is_file() {
        return Err(ApplyUpdateError::InvalidPreparedPayload(
            prepared.executable.clone(),
        ));
    }
    let actual = file_sha256(&prepared.executable)
        .map_err(|error| ApplyUpdateError::Verification(error.to_string()))?;
    if actual != prepared.executable_sha256 {
        return Err(ApplyUpdateError::InvalidPreparedPayload(
            prepared.executable.clone(),
        ));
    }
    let tree_sha256 =
        prepared_tree_sha256(&prepared.replace_path).map_err(ApplyUpdateError::Verification)?;
    if tree_sha256 != prepared.tree_sha256 {
        return Err(ApplyUpdateError::InvalidPreparedPayload(
            prepared.replace_path.clone(),
        ));
    }
    if matches!(
        kind,
        InstallKind::DirectoryBundle | InstallKind::MacApplicationBundle
    ) {
        let verified = locate_directory_payload(&prepared.replace_path, os)
            .map_err(|_| ApplyUpdateError::InvalidPreparedPayload(prepared.replace_path.clone()))?;
        if verified.root != prepared.replace_path || verified.executable != prepared.executable {
            return Err(ApplyUpdateError::InvalidPreparedPayload(
                prepared.replace_path.clone(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn prepared_tree_sha256(root: &Path) -> Result<String, String> {
    let root_metadata = fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if root_metadata.file_type().is_symlink() {
        return Err(format!(
            "symbolic link in prepared payload: {}",
            root.display()
        ));
    }
    let mut entries = Vec::new();
    if root_metadata.is_file() {
        entries.push((PathBuf::new(), root.to_owned(), root_metadata));
    } else if root_metadata.is_dir() {
        collect_prepared_entries(root, root, &mut entries)?;
    } else {
        return Err(format!(
            "special file in prepared payload: {}",
            root.display()
        ));
    }
    if entries.len() > MAX_PREPARED_ENTRIES {
        return Err("prepared payload contains too many entries".into());
    }
    entries.sort_by_key(|entry| path_identity_bytes(&entry.0));

    let mut hasher = Sha256::new();
    let mut total_bytes = 0_u64;
    for (relative, path, metadata) in entries {
        hash_field(&mut hasher, &path_identity_bytes(&relative));
        hasher.update(prepared_permissions(&metadata).to_le_bytes());
        if metadata.is_dir() {
            hasher.update(b"d");
            continue;
        }
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "unsafe entry in prepared payload: {}",
                path.display()
            ));
        }
        hasher.update(b"f");
        hasher.update(metadata.len().to_le_bytes());
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| "prepared payload size overflow".to_owned())?;
        if total_bytes > MAX_PREPARED_BYTES {
            return Err("prepared payload exceeds the extracted-size safety limit".into());
        }
        let digest = file_sha256(&path).map_err(|error| error.to_string())?;
        hash_field(&mut hasher, digest.as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_prepared_entries(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<(PathBuf, PathBuf, fs::Metadata)>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
            return Err(format!(
                "unsafe entry in prepared payload: {}",
                path.display()
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_owned();
        entries.push((relative, path.clone(), metadata));
        if path.is_dir() {
            collect_prepared_entries(root, &path, entries)?;
        }
        if entries.len() > MAX_PREPARED_ENTRIES {
            return Err("prepared payload contains too many entries".into());
        }
    }
    Ok(())
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

#[cfg(unix)]
fn path_identity_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn path_identity_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(unix)]
fn prepared_permissions(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o7777
}

#[cfg(windows)]
fn prepared_permissions(metadata: &fs::Metadata) -> u32 {
    u32::from(metadata.permissions().readonly())
}

fn swap_paths(
    target: &Path,
    prepared: &Path,
    backup: &Path,
    install_prepared: impl FnOnce() -> io::Result<()>,
) -> Result<(), ApplyUpdateError> {
    rename_no_replace(target, backup).map_err(|source| ApplyUpdateError::Io {
        path: backup.to_owned(),
        source,
    })?;
    match install_prepared() {
        Ok(()) => Ok(()),
        Err(replace_error) => match rename_no_replace(backup, target) {
            Ok(()) => Err(ApplyUpdateError::Io {
                path: prepared.to_owned(),
                source: replace_error,
            }),
            Err(restore_error) => Err(ApplyUpdateError::RestoreFailed {
                replace_error: replace_error.to_string(),
                restore_error: restore_error.to_string(),
                backup: backup.to_owned(),
            }),
        },
    }
}

fn remove_path_entry(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(unix)]
pub(crate) fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(io::Error::from)
}

#[cfg(windows)]
pub(crate) fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::MoveFileW;

    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn valid_prepared_name(candidate: &str, target_name: &str, kind: InstallKind) -> bool {
    let (prefix, suffix) = transaction_name_parts(target_name, kind, "prepared");
    let Some(candidate) = candidate.strip_prefix(&format!("{prefix}-")) else {
        return false;
    };
    let token = if suffix.is_empty() {
        candidate
    } else {
        let Some(token) = candidate.strip_suffix(&suffix) else {
            return false;
        };
        token
    };
    token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Default)]
struct CopyTotals {
    entries: usize,
    bytes: u64,
}

fn copy_standalone_payload(
    payload: &DirectoryPayload,
    destination: &Path,
) -> Result<String, PreparePayloadError> {
    let metadata = safe_source_metadata(&payload.executable)?;
    if !metadata.is_file() {
        return Err(PreparePayloadError::UnsafeEntry(payload.executable.clone()));
    }
    if metadata.len() > MAX_PREPARED_BYTES {
        return Err(PreparePayloadError::PayloadTooLarge);
    }
    copy_regular_file(&payload.executable, destination, &metadata)?;
    let result = verify_copied_executable(&payload.executable, destination);
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

fn copy_directory_contents(
    source: &Path,
    destination: &Path,
    metadata: &fs::Metadata,
    totals: &mut CopyTotals,
) -> Result<(), PreparePayloadError> {
    bump_entry(totals)?;
    for entry in fs::read_dir(source).map_err(|source_error| PreparePayloadError::Io {
        path: source.to_owned(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| PreparePayloadError::Io {
            path: source.to_owned(),
            source: source_error,
        })?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let entry_metadata = safe_source_metadata(&source_path)?;
        if entry_metadata.is_dir() {
            fs::create_dir(&destination_path).map_err(|source_error| PreparePayloadError::Io {
                path: destination_path.clone(),
                source: source_error,
            })?;
            copy_directory_contents(&source_path, &destination_path, &entry_metadata, totals)?;
        } else if entry_metadata.is_file() {
            bump_entry(totals)?;
            totals.bytes = totals.bytes.saturating_add(entry_metadata.len());
            if totals.bytes > MAX_PREPARED_BYTES {
                return Err(PreparePayloadError::PayloadTooLarge);
            }
            copy_regular_file(&source_path, &destination_path, &entry_metadata)?;
        } else {
            return Err(PreparePayloadError::UnsafeEntry(source_path));
        }
    }
    fs::set_permissions(destination, metadata.permissions()).map_err(|source_error| {
        PreparePayloadError::Io {
            path: destination.to_owned(),
            source: source_error,
        }
    })?;
    Ok(())
}

fn safe_source_metadata(path: &Path) -> Result<fs::Metadata, PreparePayloadError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| PreparePayloadError::Io {
        path: path.to_owned(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(PreparePayloadError::UnsafeEntry(path.to_owned()));
    }
    Ok(metadata)
}

fn bump_entry(totals: &mut CopyTotals) -> Result<(), PreparePayloadError> {
    totals.entries = totals.entries.saturating_add(1);
    if totals.entries > MAX_PREPARED_ENTRIES {
        Err(PreparePayloadError::TooManyEntries)
    } else {
        Ok(())
    }
}

fn copy_regular_file(
    source: &Path,
    destination: &Path,
    metadata: &fs::Metadata,
) -> Result<(), PreparePayloadError> {
    let input = File::open(source).map_err(|source_error| PreparePayloadError::Io {
        path: source.to_owned(),
        source: source_error,
    })?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|source_error| PreparePayloadError::Io {
            path: destination.to_owned(),
            source: source_error,
        })?;
    let result = (|| {
        let mut limited = input.take(metadata.len() + 1);
        let copied = io::copy(&mut limited, &mut output).map_err(|source_error| {
            PreparePayloadError::Io {
                path: destination.to_owned(),
                source: source_error,
            }
        })?;
        if copied != metadata.len() {
            return Err(PreparePayloadError::Io {
                path: source.to_owned(),
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "source file changed while staging",
                ),
            });
        }
        output
            .flush()
            .map_err(|source_error| PreparePayloadError::Io {
                path: destination.to_owned(),
                source: source_error,
            })?;
        output
            .sync_all()
            .map_err(|source_error| PreparePayloadError::Io {
                path: destination.to_owned(),
                source: source_error,
            })?;
        fs::set_permissions(destination, metadata.permissions()).map_err(|source_error| {
            PreparePayloadError::Io {
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

fn verify_copied_executable(source: &Path, copied: &Path) -> Result<String, PreparePayloadError> {
    let expected = file_sha256(source)
        .map_err(|error| PreparePayloadError::Verification(error.to_string()))?;
    let actual = file_sha256(copied)
        .map_err(|error| PreparePayloadError::Verification(error.to_string()))?;
    if actual == expected {
        Ok(actual)
    } else {
        Err(PreparePayloadError::ExecutableDigestMismatch)
    }
}

fn detect_install_layout_for(
    executable: &Path,
    os: &str,
    temporary_directory: &Path,
) -> Result<InstallLayout, InstallLayoutError> {
    let metadata =
        fs::symlink_metadata(executable).map_err(|source| InstallLayoutError::Inspect {
            path: executable.to_owned(),
            source,
        })?;
    if metadata.file_type().is_symlink() {
        return Err(InstallLayoutError::SymlinkExecutable(executable.to_owned()));
    }
    if !metadata.is_file() {
        return Err(InstallLayoutError::NotExecutableFile(executable.to_owned()));
    }
    let executable = executable
        .canonicalize()
        .map_err(|source| InstallLayoutError::Inspect {
            path: executable.to_owned(),
            source,
        })?;

    if os == "windows" && path_is_within(&executable, temporary_directory, true) {
        return Err(InstallLayoutError::TemporaryWindowsLocation(executable));
    }

    if os == "macos" {
        if let Some(application) = mac_application_root(&executable) {
            return Ok(InstallLayout {
                kind: InstallKind::MacApplicationBundle,
                executable,
                replace_path: application,
            });
        }
        if executable
            .ancestors()
            .any(|path| path.extension().is_some_and(|value| value == "app"))
        {
            return Err(InstallLayoutError::MalformedMacApplication(executable));
        }
    }

    let parent = executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_owned();
    match directory_bundle_status(&parent, os)? {
        DirectoryBundleStatus::Complete => Ok(InstallLayout {
            kind: InstallKind::DirectoryBundle,
            executable,
            replace_path: parent,
        }),
        DirectoryBundleStatus::NotBundle => Ok(InstallLayout {
            kind: InstallKind::StandaloneExecutable,
            replace_path: executable.clone(),
            executable,
        }),
    }
}

fn path_is_within(path: &Path, directory: &Path, case_insensitive: bool) -> bool {
    directory
        .canonicalize()
        .is_ok_and(|directory| path_components_start_with(path, &directory, case_insensitive))
}

fn path_components_start_with(path: &Path, directory: &Path, case_insensitive: bool) -> bool {
    let mut path_components = path.components();
    directory.components().all(|expected| {
        path_components.next().is_some_and(|actual| {
            if case_insensitive {
                actual.as_os_str().to_string_lossy().to_lowercase()
                    == expected.as_os_str().to_string_lossy().to_lowercase()
            } else {
                actual == expected
            }
        })
    })
}

fn path_entry_exists(path: &Path) -> Result<bool, TransactionPlanError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(TransactionPlanError::Inspect {
            path: path.to_owned(),
            source,
        }),
    }
}

fn is_regular_file_without_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

fn mac_application_root(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let application = contents.parent()?;
    (application.extension()? == "app").then(|| application.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectoryBundleStatus {
    NotBundle,
    Complete,
}

fn directory_bundle_status(
    root: &Path,
    os: &str,
) -> Result<DirectoryBundleStatus, InstallLayoutError> {
    let marker = root.join(DIRECTORY_BUNDLE_MARKER_NAME);
    let marker_metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DirectoryBundleStatus::NotBundle);
        }
        Err(source) => {
            return Err(InstallLayoutError::Inspect {
                path: marker,
                source,
            });
        }
    };
    if !marker_metadata.is_file()
        || marker_metadata.file_type().is_symlink()
        || marker_metadata.len() != DIRECTORY_BUNDLE_MARKER.len() as u64
        || !fs::read(&marker).is_ok_and(|contents| contents == DIRECTORY_BUNDLE_MARKER)
    {
        return Err(InstallLayoutError::InvalidDirectoryBundleMarker(marker));
    }

    let binary = if os == "windows" {
        "rclone.exe"
    } else {
        "rclone"
    };
    let rclone = root.join("bin").join(binary);
    let digest = rclone.with_file_name(format!("{binary}.sha256"));
    if !is_regular_file_without_symlink(&rclone) || !is_regular_file_without_symlink(&digest) {
        return Err(InstallLayoutError::IncompleteDirectoryBundle(
            root.to_owned(),
        ));
    }
    verify_bundled(&rclone).map_err(|error| InstallLayoutError::CorruptDirectoryBundle {
        path: rclone,
        message: error.to_string(),
    })?;
    Ok(DirectoryBundleStatus::Complete)
}

fn collect_directory_payload_candidate(
    root: &Path,
    os: &str,
    candidates: &mut Vec<DirectoryPayload>,
) -> Result<(), PayloadError> {
    if os == "macos" && root.extension().is_some_and(|extension| extension == "app") {
        return collect_mac_application_payload(root, candidates);
    }
    if directory_bundle_status(root, os)? == DirectoryBundleStatus::NotBundle {
        return Ok(());
    }
    let executable = root.join(if os == "windows" {
        "SSHMountMate.exe"
    } else {
        "SSHMountMate"
    });
    let metadata = match fs::symlink_metadata(&executable) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(PayloadError::MissingExecutable(executable));
        }
        Err(source) => {
            return Err(PayloadError::Inspect {
                path: executable,
                source,
            });
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(PayloadError::MissingExecutable(executable));
    }
    candidates.push(DirectoryPayload {
        root: root.to_owned(),
        executable,
    });
    Ok(())
}

fn collect_mac_application_payload(
    root: &Path,
    candidates: &mut Vec<DirectoryPayload>,
) -> Result<(), PayloadError> {
    let root = root
        .canonicalize()
        .map_err(|source| PayloadError::Inspect {
            path: root.to_owned(),
            source,
        })?;
    let executable = root.join("Contents/MacOS/SSHMountMate");
    let info = root.join("Contents/Info.plist");
    let rclone = root.join("Contents/Resources/bin/rclone");
    if !is_regular_file_without_symlink(&executable)
        || !is_regular_file_without_symlink(&info)
        || !is_regular_file_without_symlink(&rclone)
        || !is_regular_file_without_symlink(&rclone.with_file_name("rclone.sha256"))
    {
        return Err(PayloadError::IncompleteMacApplication(root));
    }
    verify_bundled(&rclone).map_err(|error| PayloadError::CorruptRclone {
        path: rclone,
        message: error.to_string(),
    })?;
    let executable = executable
        .canonicalize()
        .map_err(|source| PayloadError::Inspect {
            path: executable,
            source,
        })?;
    let layout = detect_install_layout_for(&executable, "macos", Path::new("/private/tmp"))?;
    if layout.kind != InstallKind::MacApplicationBundle || layout.replace_path != root {
        return Err(PayloadError::IncompleteMacApplication(root));
    }
    candidates.push(DirectoryPayload { root, executable });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::*;

    fn create_file(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(path)
            .unwrap()
            .write_all(b"binary")
            .unwrap();
    }

    fn create_directory_bundle(root: &Path, os: &str) -> PathBuf {
        let executable = root.join(if os == "windows" {
            "SSHMountMate.exe"
        } else {
            "SSHMountMate"
        });
        let rclone_name = if os == "windows" {
            "rclone.exe"
        } else {
            "rclone"
        };
        let rclone = root.join("bin").join(rclone_name);
        create_file(&executable);
        create_file(&rclone);
        fs::write(
            rclone.with_file_name(format!("{rclone_name}.sha256")),
            format!("{:x}", Sha256::digest(b"binary")),
        )
        .unwrap();
        fs::write(
            root.join(DIRECTORY_BUNDLE_MARKER_NAME),
            DIRECTORY_BUNDLE_MARKER,
        )
        .unwrap();
        executable
    }

    fn create_mac_application(root: &Path, executable_contents: &[u8]) -> PathBuf {
        let executable = root.join("Contents/MacOS/SSHMountMate");
        let rclone = root.join("Contents/Resources/bin/rclone");
        create_file(&executable);
        fs::write(&executable, executable_contents).unwrap();
        create_file(&rclone);
        fs::write(
            rclone.with_file_name("rclone.sha256"),
            format!("{:x}", Sha256::digest(b"binary")),
        )
        .unwrap();
        fs::write(root.join("Contents/Info.plist"), b"<?xml version=\"1.0\"?>").unwrap();
        executable
    }

    #[test]
    fn standalone_executable_replaces_only_itself() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("portable/SSHMountMate.exe");
        create_file(&executable);

        let layout =
            detect_install_layout_for(&executable, "windows", &temp.path().join("unrelated-temp"))
                .unwrap();

        assert_eq!(layout.kind, InstallKind::StandaloneExecutable);
        assert_eq!(layout.replace_path, executable.canonicalize().unwrap());
    }

    #[test]
    fn standalone_update_payload_accepts_root_or_one_wrapper() {
        for wrapped in [false, true] {
            let temp = tempdir().unwrap();
            let root = temp.path().join("extracted");
            let payload_root = if wrapped {
                root.join("SSHMountMate-windows-x64")
            } else {
                root.clone()
            };
            let executable = payload_root.join("SSHMountMate.exe");
            create_file(&executable);

            let payload =
                locate_update_payload(&root, InstallKind::StandaloneExecutable, "windows").unwrap();

            assert_eq!(payload.root, payload_root.canonicalize().unwrap());
            assert_eq!(payload.executable, executable.canonicalize().unwrap());
        }
    }

    #[test]
    fn standalone_update_payload_rejects_missing_and_ambiguous_executables() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("extracted");
        fs::create_dir(&root).unwrap();
        assert!(matches!(
            locate_update_payload(&root, InstallKind::StandaloneExecutable, "linux"),
            Err(PayloadError::MissingStandaloneExecutable)
        ));

        create_file(&root.join("first/SSHMountMate"));
        create_file(&root.join("second/SSHMountMate"));
        assert!(matches!(
            locate_update_payload(&root, InstallKind::StandaloneExecutable, "linux"),
            Err(PayloadError::AmbiguousStandaloneExecutable)
        ));
    }

    #[test]
    fn complete_directory_bundle_replaces_the_bundle_directory() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("SSHMountMate");
        let executable = create_directory_bundle(&root, "windows");

        let layout =
            detect_install_layout_for(&executable, "windows", &temp.path().join("other")).unwrap();

        assert_eq!(layout.kind, InstallKind::DirectoryBundle);
        assert_eq!(layout.replace_path, root.canonicalize().unwrap());
    }

    #[test]
    fn adjacent_rclone_without_an_install_marker_does_not_claim_the_directory() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("user-folder");
        let executable = root.join("renamed.exe");
        create_file(&executable);
        create_file(&root.join("bin/rclone.exe"));
        create_file(&root.join("bin/rclone.exe.sha256"));

        let layout =
            detect_install_layout_for(&executable, "windows", &temp.path().join("other")).unwrap();

        assert_eq!(layout.kind, InstallKind::StandaloneExecutable);
        assert_eq!(layout.replace_path, executable.canonicalize().unwrap());
    }

    #[test]
    fn marked_but_incomplete_directory_bundle_is_rejected() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("SSHMountMate");
        let executable = root.join("SSHMountMate");
        create_file(&executable);
        fs::write(
            root.join(DIRECTORY_BUNDLE_MARKER_NAME),
            DIRECTORY_BUNDLE_MARKER,
        )
        .unwrap();

        assert!(matches!(
            detect_install_layout_for(&executable, "linux", temp.path()),
            Err(InstallLayoutError::IncompleteDirectoryBundle(_))
        ));
    }

    #[test]
    fn marked_directory_bundle_with_tampered_rclone_is_rejected() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("SSHMountMate");
        let executable = create_directory_bundle(&root, "linux");
        fs::write(root.join("bin/rclone"), b"tampered").unwrap();

        assert!(matches!(
            detect_install_layout_for(&executable, "linux", temp.path()),
            Err(InstallLayoutError::CorruptDirectoryBundle { .. })
        ));
    }

    #[test]
    fn directory_payload_accepts_root_and_single_wrapper_layouts() {
        for wrapped in [false, true] {
            let temp = tempdir().unwrap();
            let extracted = temp.path().join("extracted");
            let bundle = if wrapped {
                extracted.join("SSHMountMate")
            } else {
                extracted.clone()
            };
            fs::create_dir_all(&extracted).unwrap();
            let executable = create_directory_bundle(&bundle, "windows");

            let payload = locate_directory_payload(&extracted, "windows").unwrap();

            assert_eq!(payload.root, bundle.canonicalize().unwrap());
            assert_eq!(payload.executable, executable.canonicalize().unwrap());
        }
    }

    #[test]
    fn directory_payload_rejects_ambiguous_bundles() {
        let temp = tempdir().unwrap();
        let extracted = temp.path().join("extracted");
        create_directory_bundle(&extracted.join("first"), "linux");
        create_directory_bundle(&extracted.join("second"), "linux");

        assert!(matches!(
            locate_directory_payload(&extracted, "linux"),
            Err(PayloadError::AmbiguousDirectoryBundle)
        ));
    }

    #[test]
    fn mac_application_payload_is_discovered_and_rclone_is_verified() {
        for wrapped in [false, true] {
            let temp = tempdir().unwrap();
            let extracted = temp.path().join("extracted");
            let application = if wrapped {
                extracted.join("SSH MountMate.app")
            } else {
                extracted.with_extension("app")
            };
            fs::create_dir_all(&extracted).unwrap();
            let executable = create_mac_application(&application, b"new application");

            let payload =
                locate_directory_payload(if wrapped { &extracted } else { &application }, "macos")
                    .unwrap();

            assert_eq!(payload.root, application.canonicalize().unwrap());
            assert_eq!(payload.executable, executable.canonicalize().unwrap());
        }
    }

    #[test]
    fn mac_application_payload_is_staged_swapped_and_rolled_back_as_one_bundle() {
        let temp = tempdir().unwrap();
        let installed = temp.path().join("SSH MountMate.app");
        let installed_executable = create_mac_application(&installed, b"old application");
        let layout =
            detect_install_layout_for(&installed_executable, "macos", temp.path()).unwrap();
        let extracted = temp.path().join("update/SSH MountMate.app");
        create_mac_application(&extracted, b"new application");
        let payload = locate_directory_payload(&extracted, "macos").unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();

        let prepared = prepare_directory_payload(&layout, &payload, &transaction, "macos").unwrap();
        assert_eq!(prepared.replace_path.extension().unwrap(), "app");
        let applied =
            apply_prepared_update_for(&layout, &prepared, &transaction, "macos", temp.path())
                .unwrap();
        assert_eq!(fs::read(&applied.executable).unwrap(), b"new application");

        rollback_applied_update(&applied).unwrap();
        assert_eq!(fs::read(installed_executable).unwrap(), b"old application");
    }

    #[test]
    fn transaction_paths_are_siblings_and_existing_backups_block_updates() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("SSHMountMate");
        create_file(&executable);
        let layout = detect_install_layout_for(&executable, "linux", temp.path()).unwrap();

        let transaction = plan_transaction_paths(&layout).unwrap();
        let canonical_executable = executable.canonicalize().unwrap();
        assert_eq!(transaction.prepared.parent(), canonical_executable.parent());
        assert_eq!(transaction.backup.parent(), canonical_executable.parent());
        assert_ne!(transaction.prepared, transaction.backup);

        create_file(&transaction.backup);
        assert!(matches!(
            plan_transaction_paths(&layout),
            Err(TransactionPlanError::BackupExists(_))
        ));
    }

    #[test]
    fn standalone_windows_transaction_paths_keep_the_exe_extension() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("portable/SSHMountMate.exe");
        create_file(&executable);
        let layout =
            detect_install_layout_for(&executable, "windows", &temp.path().join("unrelated-temp"))
                .unwrap();

        let transaction = plan_transaction_paths(&layout).unwrap();

        assert_eq!(transaction.prepared.extension().unwrap(), "exe");
        assert_eq!(transaction.backup.extension().unwrap(), "exe");
        assert!(valid_prepared_name(
            transaction.prepared.file_name().unwrap().to_str().unwrap(),
            "SSHMountMate.exe",
            InstallKind::StandaloneExecutable,
        ));
    }

    #[test]
    fn standalone_payload_is_copied_and_verified_on_the_install_volume() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("portable/SSHMountMate.exe");
        create_file(&executable);
        let layout =
            detect_install_layout_for(&executable, "windows", &temp.path().join("unrelated-temp"))
                .unwrap();
        let payload_root = temp.path().join("extracted");
        let payload_executable = create_directory_bundle(&payload_root, "windows");
        fs::write(&payload_executable, b"new executable").unwrap();
        let payload = locate_directory_payload(&payload_root, "windows").unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();

        let prepared =
            prepare_directory_payload(&layout, &payload, &transaction, "windows").unwrap();

        assert_eq!(prepared.replace_path, transaction.prepared);
        assert_eq!(prepared.executable, transaction.prepared);
        assert_eq!(fs::read(prepared.executable).unwrap(), b"new executable");
        assert!(!transaction.backup.exists());
    }

    #[test]
    fn directory_payload_is_copied_then_fully_revalidated() {
        let temp = tempdir().unwrap();
        let current_root = temp.path().join("installed");
        let current_executable = create_directory_bundle(&current_root, "linux");
        let layout = detect_install_layout_for(&current_executable, "linux", temp.path()).unwrap();
        let payload_root = temp.path().join("extracted");
        create_directory_bundle(&payload_root, "linux");
        fs::write(payload_root.join("release-notes.txt"), b"new release").unwrap();
        let payload = locate_directory_payload(&payload_root, "linux").unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();

        let prepared = prepare_directory_payload(&layout, &payload, &transaction, "linux").unwrap();

        assert_eq!(prepared.replace_path, transaction.prepared);
        assert_eq!(
            fs::read(transaction.prepared.join("release-notes.txt")).unwrap(),
            b"new release"
        );
        assert_eq!(
            locate_directory_payload(&transaction.prepared, "linux")
                .unwrap()
                .executable,
            prepared.executable
        );

        fs::write(transaction.prepared.join("release-notes.txt"), b"tampered").unwrap();
        assert!(matches!(
            apply_prepared_update_for(
                &layout,
                &prepared,
                &transaction,
                "linux",
                &temp.path().join("unrelated-temp"),
            ),
            Err(ApplyUpdateError::InvalidPreparedPayload(_))
        ));
        assert!(layout.replace_path.exists());
        assert!(!transaction.backup.exists());
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_payload_entries_fail_staging_and_are_cleaned() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let current_root = temp.path().join("installed");
        let current_executable = create_directory_bundle(&current_root, "linux");
        let layout = detect_install_layout_for(&current_executable, "linux", temp.path()).unwrap();
        let payload_root = temp.path().join("extracted");
        create_directory_bundle(&payload_root, "linux");
        let payload = locate_directory_payload(&payload_root, "linux").unwrap();
        symlink("outside", payload_root.join("unsafe-link")).unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();

        assert!(matches!(
            prepare_directory_payload(&layout, &payload, &transaction, "linux"),
            Err(PreparePayloadError::UnsafeEntry(_))
        ));
        assert!(!transaction.prepared.exists());
    }

    #[test]
    fn forged_transaction_paths_are_rejected_before_copying() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("SSHMountMate");
        create_file(&executable);
        let layout = detect_install_layout_for(&executable, "linux", temp.path()).unwrap();
        let payload_root = temp.path().join("extracted");
        create_directory_bundle(&payload_root, "linux");
        let payload = locate_directory_payload(&payload_root, "linux").unwrap();
        let forged = TransactionPaths {
            prepared: temp.path().join("arbitrary-prepared"),
            backup: temp.path().join("arbitrary-backup"),
        };

        assert!(matches!(
            prepare_directory_payload(&layout, &payload, &forged, "linux"),
            Err(PreparePayloadError::InvalidTransactionPaths)
        ));
        assert!(!forged.prepared.exists());
    }

    #[test]
    fn occupied_prepared_path_is_never_removed_by_failed_staging() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("SSHMountMate");
        create_file(&executable);
        let layout = detect_install_layout_for(&executable, "linux", temp.path()).unwrap();
        let payload_root = temp.path().join("extracted");
        create_directory_bundle(&payload_root, "linux");
        let payload = locate_directory_payload(&payload_root, "linux").unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();
        fs::write(&transaction.prepared, b"not owned by updater").unwrap();

        assert!(matches!(
            prepare_directory_payload(&layout, &payload, &transaction, "linux"),
            Err(PreparePayloadError::PreparedExists(_))
        ));
        assert_eq!(
            fs::read(&transaction.prepared).unwrap(),
            b"not owned by updater"
        );
    }

    #[test]
    fn standalone_swap_and_rollback_restore_the_previous_executable() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("SSHMountMate");
        fs::write(&executable, b"old executable").unwrap();
        let layout = detect_install_layout_for(&executable, "linux", temp.path()).unwrap();
        let payload_root = temp.path().join("extracted");
        let payload_executable = create_directory_bundle(&payload_root, "linux");
        fs::write(&payload_executable, b"new executable").unwrap();
        let payload = locate_directory_payload(&payload_root, "linux").unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();
        let prepared = prepare_directory_payload(&layout, &payload, &transaction, "linux").unwrap();

        let applied = apply_prepared_update_for(
            &layout,
            &prepared,
            &transaction,
            "linux",
            &temp.path().join("unrelated-temp"),
        )
        .unwrap();

        assert_eq!(fs::read(&executable).unwrap(), b"new executable");
        assert_eq!(fs::read(&transaction.backup).unwrap(), b"old executable");
        assert!(!transaction.prepared.exists());

        rollback_applied_update(&applied).unwrap();
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
        assert_eq!(fs::read(&transaction.prepared).unwrap(), b"new executable");
        assert!(!transaction.backup.exists());
    }

    #[test]
    fn directory_swap_is_committed_only_after_backup_cleanup() {
        let temp = tempdir().unwrap();
        let installed = temp.path().join("installed");
        let executable = create_directory_bundle(&installed, "linux");
        fs::write(&executable, b"old executable").unwrap();
        let layout = detect_install_layout_for(&executable, "linux", temp.path()).unwrap();
        let payload_root = temp.path().join("extracted");
        let payload_executable = create_directory_bundle(&payload_root, "linux");
        fs::write(&payload_executable, b"new executable").unwrap();
        let payload = locate_directory_payload(&payload_root, "linux").unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();
        let prepared = prepare_directory_payload(&layout, &payload, &transaction, "linux").unwrap();

        let applied = apply_prepared_update_for(
            &layout,
            &prepared,
            &transaction,
            "linux",
            &temp.path().join("unrelated-temp"),
        )
        .unwrap();

        assert_eq!(fs::read(&applied.executable).unwrap(), b"new executable");
        assert_eq!(
            fs::read(transaction.backup.join("SSHMountMate")).unwrap(),
            b"old executable"
        );
        commit_applied_update(&applied).unwrap();
        assert!(!transaction.backup.exists());
        assert_eq!(fs::read(&applied.executable).unwrap(), b"new executable");
    }

    #[test]
    fn failed_second_swap_step_restores_the_original_target() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("target");
        let prepared = temp.path().join("prepared");
        let backup = temp.path().join("backup");
        fs::write(&target, b"old").unwrap();
        fs::write(&prepared, b"new").unwrap();

        assert!(matches!(
            swap_paths(&target, &prepared, &backup, || {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"))
            }),
            Err(ApplyUpdateError::Io { .. })
        ));

        assert_eq!(fs::read(target).unwrap(), b"old");
        assert_eq!(fs::read(prepared).unwrap(), b"new");
        assert!(!backup.exists());
    }

    #[test]
    fn swap_never_overwrites_an_existing_backup() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("target");
        let prepared = temp.path().join("prepared");
        let backup = temp.path().join("backup");
        fs::write(&target, b"old").unwrap();
        fs::write(&prepared, b"new").unwrap();
        fs::write(&backup, b"previous backup").unwrap();

        assert!(matches!(
            swap_paths(&target, &prepared, &backup, || {
                panic!("the install step must not run when backup reservation fails")
            }),
            Err(ApplyUpdateError::Io { .. })
        ));

        assert_eq!(fs::read(target).unwrap(), b"old");
        assert_eq!(fs::read(prepared).unwrap(), b"new");
        assert_eq!(fs::read(backup).unwrap(), b"previous backup");
    }

    #[test]
    fn prepared_executable_tampering_is_rejected_before_swap() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("SSHMountMate");
        fs::write(&executable, b"old executable").unwrap();
        let layout = detect_install_layout_for(&executable, "linux", temp.path()).unwrap();
        let payload_root = temp.path().join("extracted");
        create_directory_bundle(&payload_root, "linux");
        let payload = locate_directory_payload(&payload_root, "linux").unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();
        let prepared = prepare_directory_payload(&layout, &payload, &transaction, "linux").unwrap();
        fs::write(&prepared.executable, b"tampered").unwrap();

        assert!(matches!(
            apply_prepared_update_for(
                &layout,
                &prepared,
                &transaction,
                "linux",
                &temp.path().join("unrelated-temp"),
            ),
            Err(ApplyUpdateError::InvalidPreparedPayload(_))
        ));
        assert_eq!(fs::read(executable).unwrap(), b"old executable");
        assert!(!transaction.backup.exists());
    }

    #[cfg(unix)]
    #[test]
    fn broken_backup_symlinks_still_block_update_transactions() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let executable = temp.path().join("SSHMountMate");
        create_file(&executable);
        let layout = detect_install_layout_for(&executable, "linux", temp.path()).unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();
        symlink(
            temp.path().join("missing-backup-target"),
            &transaction.backup,
        )
        .unwrap();

        assert!(matches!(
            plan_transaction_paths(&layout),
            Err(TransactionPlanError::BackupExists(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn directory_bundle_rejects_a_symlinked_rclone() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let root = temp.path().join("SSHMountMate");
        let executable = create_directory_bundle(&root, "linux");
        let rclone = root.join("bin/rclone");
        let external = temp.path().join("external-rclone");
        create_file(&external);
        fs::remove_file(&rclone).unwrap();
        symlink(external, rclone).unwrap();

        assert!(matches!(
            detect_install_layout_for(&executable, "linux", temp.path()),
            Err(InstallLayoutError::IncompleteDirectoryBundle(_))
        ));
    }

    #[test]
    fn mac_application_replaces_the_entire_app_bundle() {
        let temp = tempdir().unwrap();
        let application = temp.path().join("SSH MountMate.app");
        let executable = application.join("Contents/MacOS/SSHMountMate");
        create_file(&executable);

        let layout = detect_install_layout_for(&executable, "macos", temp.path()).unwrap();

        assert_eq!(layout.kind, InstallKind::MacApplicationBundle);
        assert_eq!(layout.replace_path, application.canonicalize().unwrap());
    }

    #[test]
    fn malformed_mac_application_is_not_treated_as_a_directory_bundle() {
        let temp = tempdir().unwrap();
        let executable = temp
            .path()
            .join("SSH MountMate.app/Unexpected/SSHMountMate");
        create_file(&executable);

        assert!(matches!(
            detect_install_layout_for(&executable, "macos", temp.path()),
            Err(InstallLayoutError::MalformedMacApplication(_))
        ));
    }

    #[test]
    fn windows_temporary_extractions_are_rejected() {
        let temp = tempdir().unwrap();
        let temporary_directory = temp.path().join("Temp");
        let executable = temporary_directory.join("archive/SSHMountMate.exe");
        create_file(&executable);

        assert!(matches!(
            detect_install_layout_for(&executable, "windows", &temporary_directory),
            Err(InstallLayoutError::TemporaryWindowsLocation(_))
        ));
    }

    #[test]
    fn windows_containment_is_case_insensitive_but_component_aware() {
        assert!(path_components_start_with(
            Path::new("C:/Users/Agent/AppData/Local/Temp/archive/app.exe"),
            Path::new("c:/users/agent/appdata/local/temp"),
            true,
        ));
        assert!(!path_components_start_with(
            Path::new("C:/Users/Agent/AppData/Local/Temporary/app.exe"),
            Path::new("c:/users/agent/appdata/local/temp"),
            true,
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_launchers_are_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let target = temp.path().join("SSHMountMate-real");
        let launcher = temp.path().join("SSHMountMate");
        create_file(&target);
        symlink(target, &launcher).unwrap();

        assert!(matches!(
            detect_install_layout_for(&launcher, "linux", temp.path()),
            Err(InstallLayoutError::SymlinkExecutable(_))
        ));
    }
}
