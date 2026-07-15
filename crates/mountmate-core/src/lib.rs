pub mod app_command;
pub mod capacity;
pub mod connection;
pub mod dependency;
pub mod model;
pub mod mountpoint;
pub mod paths;
pub mod process;
pub mod rc;
pub mod rclone;
pub mod rclone_binary;
pub mod runtime;
pub mod service;
pub mod ssh;
pub mod storage;
pub mod transfer;
pub mod update;
pub mod update_helper;
pub mod update_install;
pub mod update_workflow;

pub use model::{
    AuthMethod, ConnectionMethod, MountBackend, MountPhase, MountState, ServerConfig, Settings,
};

pub const APP_NAME: &str = "SSH MountMate";
pub const APP_ID: &str = "ssh-mountmate";
pub const LEGACY_APP_ID: &str = "rsshmount";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
