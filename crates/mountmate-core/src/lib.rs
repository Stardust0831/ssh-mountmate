pub mod model;
pub mod paths;
pub mod process;
pub mod rc;
pub mod rclone;
pub mod storage;
pub mod transfer;
pub mod update;

pub use model::{AuthMethod, ConnectionMethod, MountState, ServerConfig, Settings};

pub const APP_NAME: &str = "SSH MountMate";
pub const APP_ID: &str = "ssh-mountmate";
pub const LEGACY_APP_ID: &str = "rsshmount";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
