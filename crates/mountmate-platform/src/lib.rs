use std::path::Path;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalProgressState {
    Hidden,
    Indeterminate,
    Normal { completed: u64, total: u64 },
    Paused { completed: u64, total: u64 },
    Error { completed: u64, total: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub body: String,
    pub progress: Option<(u64, u64)>,
}

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("{0} is not supported on this desktop environment")]
    Unsupported(&'static str),
    #[error("platform integration failed: {0}")]
    Failed(String),
}

pub trait PlatformIntegration: Send + Sync {
    fn show_notification(&self, notification: &Notification) -> Result<(), PlatformError>;
    fn set_global_progress(&self, state: GlobalProgressState) -> Result<(), PlatformError>;
    fn register_file_manager_menu(&self, executable: &Path) -> Result<(), PlatformError>;
    fn unregister_file_manager_menu(&self) -> Result<(), PlatformError>;
}

pub struct Platform;

impl PlatformIntegration for Platform {
    fn show_notification(&self, _notification: &Notification) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("native notifications"))
    }

    fn set_global_progress(&self, _state: GlobalProgressState) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("taskbar or dock progress"))
    }

    fn register_file_manager_menu(&self, _executable: &Path) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("file-manager integration"))
    }

    fn unregister_file_manager_menu(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("file-manager integration"))
    }
}
