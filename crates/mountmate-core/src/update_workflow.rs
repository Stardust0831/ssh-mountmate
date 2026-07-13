use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::paths::AppPaths;
use crate::update::{
    ReleaseAsset, UpdateError, download_verified_asset, expected_asset_name, safe_extract_zip,
    verified_sha256,
};
use crate::update_helper::{
    UpdateHelperAuthorization, UpdateHelperError, capture_current_process_identity,
    launch_update_helper, materialize_update_helper, write_update_plan,
};
use crate::update_install::{
    InstallLayoutError, PayloadError, PreparePayloadError, PreparedPayload, TransactionPaths,
    TransactionPlanError, detect_install_layout, locate_directory_payload, plan_transaction_paths,
    prepare_directory_payload,
};

#[derive(Debug, Error)]
pub enum UpdateWorkflowError {
    #[error("the release asset does not match this platform: expected {expected}, got {actual}")]
    WrongAsset { expected: String, actual: String },
    #[error(transparent)]
    Download(#[from] UpdateError),
    #[error(transparent)]
    Layout(#[from] InstallLayoutError),
    #[error(transparent)]
    Payload(#[from] PayloadError),
    #[error(transparent)]
    Prepare(#[from] PreparePayloadError),
    #[error(transparent)]
    Transaction(#[from] TransactionPlanError),
    #[error(transparent)]
    Helper(#[from] UpdateHelperError),
}

#[derive(Debug, Clone)]
pub struct PreparedUpdateLaunch {
    helper_executable: PathBuf,
    authorization: UpdateHelperAuthorization,
    prepared: PreparedPayload,
    transaction: TransactionPaths,
}

impl PreparedUpdateLaunch {
    pub fn launch(self) -> Result<u32, UpdateWorkflowError> {
        match launch_update_helper(&self.helper_executable, &self.authorization) {
            Ok(pid) => Ok(pid),
            Err(error) => {
                let _ = fs::remove_file(&self.authorization.plan_path);
                let _ = remove_owned_prepared(&self.prepared, &self.transaction);
                Err(error.into())
            }
        }
    }

    pub fn cancel(self) {
        let _ = fs::remove_file(&self.authorization.plan_path);
        let _ = remove_owned_prepared(&self.prepared, &self.transaction);
    }
}

pub fn prepare_update_install(
    paths: &AppPaths,
    asset: &ReleaseAsset,
    current_executable: &Path,
    relaunch_arguments: Vec<String>,
    progress: Option<&mut dyn FnMut(u64, u64)>,
) -> Result<PreparedUpdateLaunch, UpdateWorkflowError> {
    let expected = expected_asset_name();
    if asset.name != expected {
        return Err(UpdateWorkflowError::WrongAsset {
            expected,
            actual: asset.name.clone(),
        });
    }
    let layout = detect_install_layout(current_executable)?;
    let parent = capture_current_process_identity(current_executable)?;
    let helper_executable =
        materialize_update_helper(&paths.update_helper_dir(), current_executable)?;

    let digest = verified_sha256(asset).map_err(UpdateError::from)?;
    let cache = paths.update_cache_dir();
    let archive = cache.join(&asset.name);
    let extracted = cache.join(format!("payload-{}", &digest[..16]));
    download_verified_asset(asset, &archive, progress)?;
    safe_extract_zip(&archive, &extracted)?;
    let payload = locate_directory_payload(&extracted, std::env::consts::OS)?;
    let transaction = plan_transaction_paths(&layout)?;
    let prepared =
        prepare_directory_payload(&layout, &payload, &transaction, std::env::consts::OS)?;
    let authorization = match write_update_plan(
        &paths.update_state_dir(),
        parent,
        layout,
        prepared.clone(),
        transaction.clone(),
        relaunch_arguments,
    ) {
        Ok(authorization) => authorization,
        Err(error) => {
            let _ = remove_owned_prepared(&prepared, &transaction);
            return Err(error.into());
        }
    };
    Ok(PreparedUpdateLaunch {
        helper_executable,
        authorization,
        prepared,
        transaction,
    })
}

fn remove_owned_prepared(
    prepared: &PreparedPayload,
    transaction: &TransactionPaths,
) -> std::io::Result<()> {
    if prepared.replace_path != transaction.prepared {
        return Ok(());
    }
    let metadata = match fs::symlink_metadata(&transaction.prepared) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(&transaction.prepared)
    } else {
        fs::remove_file(&transaction.prepared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_never_removes_a_path_not_owned_by_the_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let owned = temp.path().join("prepared");
        let unrelated = temp.path().join("unrelated");
        fs::write(&owned, b"owned").unwrap();
        fs::write(&unrelated, b"keep").unwrap();
        let prepared = PreparedPayload {
            replace_path: unrelated.clone(),
            executable: unrelated.clone(),
            executable_sha256: "a".repeat(64),
        };
        let transaction = TransactionPaths {
            prepared: owned.clone(),
            backup: temp.path().join("backup"),
        };

        remove_owned_prepared(&prepared, &transaction).unwrap();

        assert!(owned.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn mismatched_platform_asset_is_rejected_before_any_installation_work() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let asset = ReleaseAsset {
            name: "SSHMountMate-wrong-platform.zip".into(),
            url: String::new(),
            digest: String::new(),
            size: 0,
        };

        assert!(matches!(
            prepare_update_install(
                &paths,
                &asset,
                Path::new("missing-executable"),
                Vec::new(),
                None,
            ),
            Err(UpdateWorkflowError::WrongAsset { .. })
        ));
        assert!(!paths.update_cache_dir().exists());
    }
}
