#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::widget::{
    Space, button, checkbox, column, container, pick_list, progress_bar, row, scrollable, stack,
    text, text_editor, text_input, toggler, tooltip,
};
use iced::{
    Center, Element, Fill, Length, Point, Size, Subscription, Task, Theme, clipboard, window,
};
use mountmate_core::app_command::{
    AppCommand, AppCommandError, AppCommandServer, InstanceLock, running_instance,
    same_instance_build, send_command_retry,
};
use mountmate_core::capacity::CapacityInfo;
use mountmate_core::connection::{
    ConnectionDraft, ConnectionSource, DraftError, ImportAction, ImportStatus, SecretAction,
    SshImportPlan,
};
use mountmate_core::dependency::{DependencyStatus, check_dependencies};
use mountmate_core::mountpoint::HOME_MOUNTPOINT_VALUE;
use mountmate_core::paths::AppPaths;
use mountmate_core::process::MountStatus;
use mountmate_core::rclone_binary::resolve_rclone;
use mountmate_core::service::MountService;
use mountmate_core::ssh::{
    default_ssh_config_path, prepare_managed_ssh_server, remove_managed_ssh_server,
};
use mountmate_core::storage::{self, read_json};
use mountmate_core::transfer::TransferSnapshot;
use mountmate_core::update::{UpdateInfo, check_for_updates};
use mountmate_core::update_helper::{
    UpdateHealthAuthorization, run_update_helper, write_update_health_marker,
};
use mountmate_core::update_workflow::{PreparedUpdateLaunch, prepare_update_install};
use mountmate_core::{
    APP_NAME, AuthMethod, ConnectionMethod, MountState, ServerConfig, Settings, VERSION,
};
#[cfg(windows)]
use mountmate_platform::NativeWindowHandle;
use mountmate_platform::{GlobalProgressState, Platform, PlatformIntegration};
use mountmate_platform::{
    Notification as NativeNotification, NotificationLevel as NativeNotificationLevel,
};

mod cli;
mod i18n;
mod transfer_center;
mod tray;

const CUSTOM_MOUNTPOINT_PENDING: &str = "__ui_custom_mountpoint_pending__";

use cli::LaunchAction;
use i18n::{Choice, LanguagePreference as Language, Locale, TextKey};
use transfer_center::{
    TransferTotals, connection_view as transfer_connection_view, totals as transfer_totals,
};
use tray::{TrayAction, TrayController};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let action = cli::parse(std::env::args().skip(1))?;
    match action {
        LaunchAction::Help => {
            println!("{}", cli::help());
            return Ok(());
        }
        LaunchAction::Version => {
            println!("{APP_NAME} {VERSION}");
            return Ok(());
        }
        LaunchAction::Licenses => {
            println!("{}", cli::licenses());
            return Ok(());
        }
        LaunchAction::CheckUpdate => {
            let info = check_for_updates(VERSION).map_err(|error| error.to_string())?;
            if info.is_newer {
                println!("Update available: {}", info.latest_version);
                println!("Release: {}", info.release_url);
                println!(
                    "Verified asset: {}",
                    info.asset
                        .as_ref()
                        .map_or("unavailable", |asset| asset.name.as_str())
                );
            } else {
                println!("SSH MountMate {VERSION} is up to date");
            }
            return Ok(());
        }
        LaunchAction::RclonePath => {
            let paths = AppPaths::discover();
            let resolved = resolve_rclone(&paths, &application_root(), None)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "rclone is not available".to_owned())?;
            println!("{}", resolved.path.display());
            return Ok(());
        }
        LaunchAction::RegisterFileManagerMenu => {
            let executable = std::env::current_exe()
                .map_err(|error| format!("Could not locate the current executable: {error}"))?;
            Platform
                .register_file_manager_menu(&executable)
                .map_err(|error| error.to_string())?;
            println!(
                "File-manager commands registered for {}",
                executable.display()
            );
            return Ok(());
        }
        LaunchAction::UnregisterFileManagerMenu => {
            Platform
                .unregister_file_manager_menu()
                .map_err(|error| error.to_string())?;
            println!("File-manager commands removed");
            return Ok(());
        }
        action @ (LaunchAction::RegisterLoginStartup | LaunchAction::UnregisterLoginStartup) => {
            let enabled = matches!(action, LaunchAction::RegisterLoginStartup);
            let executable = std::env::current_exe()
                .map_err(|error| format!("Could not locate the current executable: {error}"))?;
            Platform
                .set_login_startup(&executable, enabled)
                .map_err(|error| error.to_string())?;
            println!(
                "Login startup {}",
                if enabled { "registered" } else { "removed" }
            );
            return Ok(());
        }
        LaunchAction::RunUpdateHelper(authorization) => {
            let executable = std::env::current_exe()
                .map_err(|error| format!("Could not locate the update helper: {error}"))?;
            run_update_helper(&authorization.plan_path, &authorization.token, &executable)
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        LaunchAction::Gui { .. } | LaunchAction::Headless(_) => {}
    }

    let paths = AppPaths::discover();
    let instance_lock = match InstanceLock::try_acquire(&paths.app_instance_lock()) {
        Ok(lock) => Arc::new(lock),
        Err(AppCommandError::AlreadyRunning) => {
            let command = match &action {
                LaunchAction::Gui { command, .. } | LaunchAction::Headless(command) => {
                    command.clone()
                }
                _ => unreachable!(),
            };
            let current_executable = std::env::current_exe()
                .map_err(|error| format!("Could not locate this executable: {error}"))?;
            let running = running_instance(&paths.app_command_state())
                .map_err(|error| format!("Could not verify the running instance: {error}"))?;
            let gui_launch = matches!(&action, LaunchAction::Gui { .. });
            if gui_launch && !same_instance_build(&running, &current_executable, VERSION) {
                let version = if running.version.is_empty() {
                    "unknown / 未知"
                } else {
                    &running.version
                };
                let confirmed = rfd::MessageDialog::new()
                    .set_title(APP_NAME)
                    .set_description(format!(
                        "A different SSH MountMate instance is already running.\n\nRunning: {version}\n{}\n\nCurrent: {VERSION}\n{}\n\nExit the running interface and start this version? Mounted drives and background rclone transfers will remain active.\n\n检测到托盘中运行的是另一个版本或路径。是否退出旧界面并启动当前版本？现有挂载和后台 rclone 传输会继续。",
                        running.executable.display(),
                        current_executable.display()
                    ))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show();
                if confirmed != rfd::MessageDialogResult::Yes {
                    return Ok(());
                }
                if let Err(error) = send_command_retry(
                    &paths.app_command_state(),
                    &AppCommand::ExitForReplacement,
                    Duration::from_secs(2),
                ) {
                    rfd::MessageDialog::new()
                        .set_title(APP_NAME)
                        .set_description(format!(
                            "The running build cannot close itself for replacement ({error}). Exit it from the system tray, then open this version again.\n\n当前托盘版本不支持自动退出替换。请先从系统托盘退出旧版本，再重新打开当前版本。"
                        ))
                        .set_buttons(rfd::MessageButtons::Ok)
                        .show();
                    return Ok(());
                }
                let deadline = Instant::now() + Duration::from_secs(8);
                loop {
                    match InstanceLock::try_acquire(&paths.app_instance_lock()) {
                        Ok(lock) => break Arc::new(lock),
                        Err(AppCommandError::AlreadyRunning) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        Err(AppCommandError::AlreadyRunning) => {
                            rfd::MessageDialog::new()
                                .set_title(APP_NAME)
                                .set_description("The running interface did not exit. It may still be completing a mount operation. Wait for that operation, exit the tray instance, and open this version again.\n\n旧界面未能退出，可能仍在执行挂载操作。请等待操作完成，从托盘退出旧实例后重新打开当前版本。")
                                .set_buttons(rfd::MessageButtons::Ok)
                                .show();
                            return Ok(());
                        }
                        Err(error) => return Err(error.to_string()),
                    }
                }
            } else {
                send_command_retry(&paths.app_command_state(), &command, Duration::from_secs(2))
                    .map_err(|error| format!("Could not contact the running instance: {error}"))?;
                return Ok(());
            }
        }
        Err(error) => return Err(error.to_string()),
    };

    match action {
        LaunchAction::Headless(command) => run_headless(&paths, command),
        LaunchAction::Gui {
            command: initial_command,
            update_health,
        } => {
            let (command_sender, command_receiver) = async_channel::unbounded();
            let command_server = Arc::new(
                AppCommandServer::start_with_version(
                    paths.app_command_state(),
                    &Platform,
                    VERSION,
                    move |command| {
                        diagnostic_trace(&format!("ipc-server received {command:?}"));
                        let _ = command_sender.send_blocking(command);
                    },
                )
                .map_err(|error| error.to_string())?,
            );
            let bootstrap = Bootstrap {
                paths,
                instance_lock,
                command_server,
                command_receiver: CommandSubscription(command_receiver),
                initial_command,
                update_health,
            };
            iced::daemon(move || App::new(bootstrap.clone()), App::update, App::view)
                .title(App::title)
                .theme(App::theme)
                .subscription(App::subscription)
                .run()
                .map_err(|error| error.to_string())
        }
        _ => unreachable!(),
    }
}

#[derive(Clone)]
struct Bootstrap {
    paths: AppPaths,
    instance_lock: Arc<InstanceLock>,
    command_server: Arc<AppCommandServer>,
    command_receiver: CommandSubscription,
    initial_command: AppCommand,
    update_health: Option<UpdateHealthAuthorization>,
}

#[derive(Clone)]
struct CommandSubscription(async_channel::Receiver<AppCommand>);

impl Hash for CommandSubscription {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "ssh-mountmate.app-command-subscription".hash(state);
    }
}

fn command_stream(subscription: &CommandSubscription) -> async_channel::Receiver<AppCommand> {
    subscription.0.clone()
}

#[derive(Clone)]
struct TraySubscription(async_channel::Receiver<TrayAction>);

impl Hash for TraySubscription {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "ssh-mountmate.tray-subscription".hash(state);
    }
}

fn tray_stream(subscription: &TraySubscription) -> async_channel::Receiver<TrayAction> {
    subscription.0.clone()
}

fn run_headless(paths: &AppPaths, command: AppCommand) -> Result<(), String> {
    let settings = storage::load_settings(paths).map_err(|error| error.to_string())?;
    let servers = storage::load_servers(paths).map_err(|error| error.to_string())?;
    let service = MountService::new(paths.clone(), application_root());
    match command {
        AppCommand::Mount { id } => {
            let server = find_server(&servers, &id)?;
            let state = service
                .mount(server, &settings)
                .map_err(|error| error.to_string())?;
            println!("Mounted {} at {}", state.remote, state.mountpoint.display());
        }
        AppCommand::Unmount { id } => {
            find_server(&servers, &id)?;
            service.unmount(&id).map_err(|error| error.to_string())?;
            println!("Unmounted {id}");
        }
        AppCommand::Open { id } => {
            find_server(&servers, &id)?;
            let state: MountState =
                read_json(&paths.state_file(&id)).map_err(|error| error.to_string())?;
            let locale =
                Locale::from_preference(Language::from_value(&settings.language), Locale::system());
            open_path(&state.mountpoint, locale)?;
        }
        AppCommand::RefreshPath { path } => {
            let result = service
                .refresh_path(&servers, &path)
                .map_err(|error| error.to_string())?;
            print_refresh_result(&result);
        }
        AppCommand::Refresh { id, relative_dir } => {
            find_server(&servers, &id)?;
            let result = service
                .refresh(&id, &relative_dir, false)
                .map_err(|error| error.to_string())?;
            print_refresh_result(&result);
        }
        AppCommand::MountAll => run_headless_batch(&service, &servers, |service, server| {
            service
                .mount(server, &settings)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })?,
        AppCommand::UnmountAll => run_headless_batch(&service, &servers, |service, server| {
            if paths.state_file(&server.id).exists() {
                service
                    .unmount(&server.id)
                    .map_err(|error| error.to_string())
            } else {
                Ok(())
            }
        })?,
        AppCommand::ShowMain | AppCommand::ShowTransfers => {
            return Err("a window command requires the GUI".into());
        }
        AppCommand::ExitForReplacement => {
            return Err("instance replacement requires the GUI".into());
        }
    }
    Ok(())
}

fn run_headless_batch(
    service: &MountService,
    servers: &[ServerConfig],
    operation: impl Fn(&MountService, &ServerConfig) -> Result<(), String> + Sync,
) -> Result<(), String> {
    let failures = std::thread::scope(|scope| {
        let tasks: Vec<_> = servers
            .iter()
            .map(|server| {
                let operation = &operation;
                scope.spawn(move || {
                    operation(service, server)
                        .err()
                        .map(|error| format!("{}: {error}", server.display_name()))
                })
            })
            .collect();
        tasks
            .into_iter()
            .filter_map(|task| task.join().ok().flatten())
            .collect::<Vec<_>>()
    });
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

fn find_server<'a>(servers: &'a [ServerConfig], id: &str) -> Result<&'a ServerConfig, String> {
    servers
        .iter()
        .find(|server| server.id == id)
        .ok_or_else(|| format!("Connection does not exist: {id}"))
}

fn print_refresh_result(result: &mountmate_core::rc::RefreshResult) {
    let directory = if result.relative_dir.is_empty() {
        "mount root"
    } else {
        &result.relative_dir
    };
    println!("Remote cache refreshed: {directory}");
    println!("Remote verified: {} direct entries", result.entries.len());
    if result.pending_uploads > 0 {
        println!(
            "{} local file(s) are still waiting to upload",
            result.pending_uploads
        );
    }
}

struct App {
    paths: AppPaths,
    _instance_lock: Arc<InstanceLock>,
    _command_server: Arc<AppCommandServer>,
    command_receiver: CommandSubscription,
    pending_commands: VecDeque<AppCommand>,
    settings: Settings,
    system_locale: Locale,
    servers: Vec<ServerConfig>,
    service: MountService,
    mount_statuses: HashMap<String, MountStatus>,
    busy: HashSet<String>,
    transfers: HashMap<String, TransferSnapshot>,
    transfer_errors: HashMap<String, String>,
    transfer_failures: HashMap<String, u8>,
    transfer_refreshing: bool,
    main_window: window::Id,
    main_window_ready: bool,
    main_window_opening: bool,
    pending_main_activation: bool,
    tray: Option<TrayController>,
    tray_action_sender: async_channel::Sender<TrayAction>,
    tray_actions: TraySubscription,
    tray_error: Option<String>,
    exit_confirmation_open: bool,
    transfer_popup: Option<window::Id>,
    transfer_popup_expanded: bool,
    dismissed_popups: HashSet<String>,
    synced_polls: HashMap<String, u8>,
    notification_tracker: TransferNotificationTracker,
    pending_unmount_after_sync: HashSet<String>,
    confirmed_unmounts: HashSet<String>,
    popup_close_notice_shown: bool,
    screen: Screen,
    connection_draft: Option<ConnectionDraft>,
    connection_custom_mountpoint: String,
    settings_draft: Option<SettingsDraft>,
    log_view: Option<MountLogView>,
    log_window: Option<window::Id>,
    custom_setting: Option<CustomSettingDraft>,
    editor_saving: bool,
    ssh_import_loading: bool,
    ssh_import_plan: Option<SshImportPlan>,
    ssh_import_actions: Vec<ImportAction>,
    pending_delete: Option<String>,
    status: String,
    update_health: Option<UpdateHealthAuthorization>,
    update_info: Option<UpdateInfo>,
    update_checking: bool,
    update_error: Option<String>,
    update_downloading: bool,
    update_progress: Arc<Mutex<UpdateDownloadProgress>>,
    prepared_update: Option<PreparedUpdateLaunch>,
    dependency_status: Option<DependencyStatus>,
    dependency_checking: bool,
    capacities: HashMap<String, CapacityInfo>,
    capacity_errors: HashSet<String>,
    capacity_refreshing: bool,
    capacity_refresh_pending: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct UpdateDownloadProgress {
    received: u64,
    total: u64,
}

#[derive(Default)]
struct TransferNotificationTracker {
    observed_active: HashSet<String>,
    notified_errors: HashSet<String>,
}

impl TransferNotificationTracker {
    fn observe(
        &mut self,
        server_id: &str,
        display_name: &str,
        snapshot: &TransferSnapshot,
        synced_polls: u8,
        locale: Locale,
    ) -> Vec<NativeNotification> {
        let mut notifications = Vec::new();
        if transfer_is_active(snapshot) {
            self.observed_active.insert(server_id.into());
        }

        let failed = snapshot.errors > 0 || snapshot.out_of_space;
        if failed {
            if self.notified_errors.insert(server_id.into()) {
                notifications.push(transfer_error_notification(
                    locale,
                    server_id,
                    display_name,
                    snapshot,
                ));
            }
        } else {
            self.notified_errors.remove(server_id);
        }

        if snapshot.synced && synced_polls >= 2 && self.observed_active.remove(server_id) {
            notifications.push(transfer_complete_notification(
                locale,
                server_id,
                display_name,
            ));
        }
        notifications
    }

    fn forget(&mut self, server_id: &str) {
        self.observed_active.remove(server_id);
        self.notified_errors.remove(server_id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Connections,
    TransferCenter,
    ConnectionEditor,
    Settings,
}

#[derive(Debug, Clone)]
struct LoadedMountLog {
    path: PathBuf,
    content: String,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct MountLogView {
    server_id: String,
    display_name: String,
    path: PathBuf,
    content: text_editor::Content,
    truncated: bool,
    loading: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LogChoice {
    id: String,
    label: String,
}

impl std::fmt::Display for LogChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.label)
    }
}

#[derive(Debug, Clone, Copy)]
enum ConnectionField {
    Name,
    HostAlias,
    Host,
    User,
    Port,
    KeyFile,
    SshConfigPath,
}

#[derive(Clone)]
struct SecretInput(String);

impl std::fmt::Debug for SecretInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[derive(Debug, Clone, Copy)]
enum SettingsField {
    CacheRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingKind {
    MaxSize,
    MaxAge,
    MinFreeSpace,
    WriteBack,
    DirCacheTime,
    BufferSize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SettingOption {
    kind: SettingKind,
    value: String,
    label: String,
    custom: bool,
}

impl std::fmt::Display for SettingOption {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.label)
    }
}

#[derive(Debug, Clone)]
struct CustomSettingDraft {
    kind: SettingKind,
    digits: String,
    unit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheMode {
    Off,
    Minimal,
    Writes,
    Full,
}

impl CacheMode {
    const ALL: [Self; 4] = [Self::Off, Self::Minimal, Self::Writes, Self::Full];

    fn from_value(value: &str) -> Self {
        match value {
            "off" => Self::Off,
            "minimal" => Self::Minimal,
            "writes" => Self::Writes,
            _ => Self::Full,
        }
    }

    fn value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Writes => "writes",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Clone)]
struct SettingsDraft {
    cache_root: String,
    cache_mode: CacheMode,
    max_size: String,
    max_age: String,
    min_free_space: String,
    write_back: String,
    dir_cache_time: String,
    buffer_size: String,
    startup_all: bool,
    auto_show_transfers: bool,
    auto_check_updates: bool,
    language: Language,
}

impl SettingsDraft {
    fn from_settings(settings: &Settings) -> Self {
        Self {
            cache_root: settings.cache_root.display().to_string(),
            cache_mode: CacheMode::from_value(&settings.vfs_cache_mode),
            max_size: settings.vfs_cache_max_size.clone(),
            max_age: settings.vfs_cache_max_age.clone(),
            min_free_space: settings.vfs_cache_min_free_space.clone(),
            write_back: settings.vfs_write_back.clone(),
            dir_cache_time: settings.dir_cache_time.clone(),
            buffer_size: settings.buffer_size.clone(),
            startup_all: settings.startup_all,
            auto_show_transfers: settings.auto_show_transfers,
            auto_check_updates: settings.auto_check_updates,
            language: Language::from_value(&settings.language),
        }
    }

    fn build(&self, original: &Settings, locale: Locale) -> Result<Settings, String> {
        if self.cache_root.trim().is_empty() {
            return Err(match locale {
                Locale::English => "Cache root is required".into(),
                Locale::Chinese => "必须填写缓存目录".into(),
            });
        }
        if self.cache_root.chars().any(char::is_control) {
            return Err(match locale {
                Locale::English => "Cache root must not contain control characters".into(),
                Locale::Chinese => "缓存目录不能包含控制字符".into(),
            });
        }
        for (name, value, required) in [
            (TextKey::MaximumAge, self.max_age.as_str(), true),
            (TextKey::WriteBackDelay, self.write_back.as_str(), true),
            (
                TextKey::DirectoryCacheTime,
                self.dir_cache_time.as_str(),
                true,
            ),
            (TextKey::MaximumSize, self.max_size.as_str(), false),
            (
                TextKey::MinimumFreeSpace,
                self.min_free_space.as_str(),
                false,
            ),
            (TextKey::BufferSize, self.buffer_size.as_str(), false),
        ] {
            validate_setting_value(locale.text(name), value, required, locale)?;
        }
        let mut settings = original.clone();
        settings.cache_root = PathBuf::from(self.cache_root.trim());
        settings.vfs_cache_mode = self.cache_mode.value().into();
        settings.vfs_cache_max_size = self.max_size.trim().into();
        settings.vfs_cache_max_age = self.max_age.trim().into();
        settings.vfs_cache_min_free_space = self.min_free_space.trim().into();
        settings.vfs_write_back = self.write_back.trim().into();
        settings.dir_cache_time = self.dir_cache_time.trim().into();
        settings.buffer_size = self.buffer_size.trim().into();
        settings.startup_all = self.startup_all;
        settings.auto_show_transfers = self.auto_show_transfers;
        settings.auto_check_updates = self.auto_check_updates;
        settings.language = self.language.value().into();
        Ok(settings)
    }
}

#[derive(Debug, Clone)]
enum Message {
    AppCommand(AppCommand),
    TrayAction(TrayAction),
    TrayTick,
    MainWindowOpened(window::Id),
    Refresh,
    RefreshFinished(Result<mountmate_core::rc::RefreshResult, String>),
    StatusesLoaded(Vec<(String, Result<MountStatus, String>)>),
    TransferTick,
    TransfersLoaded(Vec<(String, Result<TransferSnapshot, String>)>),
    NotificationFinished(Result<(), String>),
    PopupOpened(window::Id),
    ClosePopup(window::Id),
    TogglePopupDetails,
    CloseRequested(window::Id),
    ExitDecision(bool),
    WindowClosed(window::Id),
    AddConnection,
    OpenTransfers,
    CloseTransfers,
    CloseTransfersDecision(rfd::MessageDialogResult),
    OpenSettings,
    OpenLogChooser,
    LogWindowOpened(window::Id),
    OpenLog(String),
    ReloadLog,
    LogLoaded {
        id: String,
        result: Result<LoadedMountLog, String>,
    },
    CopyLog,
    LogAction(text_editor::Action),
    CloseLog,
    CancelEditor,
    ConnectionSourceChanged(ConnectionSource),
    ConnectionFieldChanged(ConnectionField, String),
    RemoteBaseChanged(String),
    RemoteSuffixChanged(String),
    MountpointChoiceChanged(String),
    CustomMountpointChanged(String),
    BrowseMountpoint,
    MountpointPicked(Option<PathBuf>),
    ConnectionAuthChanged(AuthMethod),
    ConnectionMethodChanged(ConnectionMethod),
    PasswordChanged(SecretInput),
    KeyPassphraseChanged(SecretInput),
    ManagedSshChanged(bool),
    CopyKeyChanged(bool),
    LoadSshConfig,
    BrowseSshConfig,
    SshConfigPicked(Option<PathBuf>),
    BrowsePrivateKey,
    PrivateKeyPicked(Option<PathBuf>),
    SshImportLoaded {
        config_path: PathBuf,
        result: Result<SshImportPlan, String>,
    },
    SshHostSelected(String),
    SshImportActionChanged(usize, ImportAction),
    SaveConnection,
    ConnectionSaved(Result<Vec<ServerConfig>, String>),
    SettingsFieldChanged(SettingsField, String),
    BrowseCacheRoot,
    CacheRootPicked(Option<PathBuf>),
    CacheModeChanged(CacheMode),
    SettingOptionChanged(SettingOption),
    CustomSettingDigitsChanged(String),
    CustomSettingUnitChanged(String),
    SaveCustomSetting,
    CancelCustomSetting,
    StartupAllChanged(bool),
    AutoTransfersChanged(bool),
    AutoTransfersDecision(rfd::MessageDialogResult),
    AutoUpdatesChanged(bool),
    CheckForUpdates,
    UpdateChecked {
        manual: bool,
        result: Result<UpdateInfo, String>,
    },
    DownloadUpdate,
    UpdatePrepared(Result<PreparedUpdateLaunch, String>),
    InstallUpdateDecision(bool),
    CheckDependencies,
    DependenciesChecked(Result<DependencyStatus, String>),
    CapacityTick,
    CapacitiesLoaded(Vec<(String, Result<Option<CapacityInfo>, String>)>),
    LanguageChanged(Language),
    RegisterFileManagerMenu,
    UnregisterFileManagerMenu,
    FileManagerMenuFinished(Result<bool, String>),
    SaveSettings,
    SettingsSaved(Result<Settings, String>),
    StartupReconciled(Result<(), String>),
    Mount(String),
    CancelPendingUnmount(String),
    UnmountWaitDecision {
        ids: Vec<String>,
        result: rfd::MessageDialogResult,
    },
    UnmountNowDecision {
        ids: Vec<String>,
        result: rfd::MessageDialogResult,
    },
    MountFinished {
        id: String,
        operation: MountOperation,
        result: Result<String, String>,
    },
    Open(String),
    OpenFinished(Result<(), String>),
    Edit(String),
    Remove(String),
    CancelRemove,
    ConfirmRemove,
    RemoveFinished(Result<Vec<ServerConfig>, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MountOperation {
    Mount,
    Unmount,
}

impl App {
    fn title(&self, window: window::Id) -> String {
        if window == self.main_window {
            format!("{APP_NAME} {VERSION}")
        } else if self.log_window == Some(window) {
            format!("{} - {APP_NAME}", self.locale().text(TextKey::Logs))
        } else {
            self.locale().text(TextKey::FileTransfer).into()
        }
    }

    fn locale(&self) -> Locale {
        let preference = self
            .settings_draft
            .as_ref()
            .map(|draft| draft.language)
            .unwrap_or_else(|| Language::from_value(&self.settings.language));
        Locale::from_preference(preference, self.system_locale)
    }

    fn theme(&self, _window: window::Id) -> Theme {
        Theme::Dark
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run_with(self.command_receiver.clone(), command_stream)
                .map(Message::AppCommand),
            Subscription::run_with(self.tray_actions.clone(), tray_stream).map(Message::TrayAction),
            iced::time::every(Duration::from_millis(100)).map(|_| Message::TrayTick),
            iced::time::every(Duration::from_secs(1)).map(|_| Message::TransferTick),
            iced::time::every(Duration::from_secs(30)).map(|_| Message::CapacityTick),
            window::close_requests().map(Message::CloseRequested),
            window::close_events().map(Message::WindowClosed),
        ])
    }

    fn new(bootstrap: Bootstrap) -> (Self, Task<Message>) {
        let Bootstrap {
            paths,
            instance_lock,
            command_server,
            command_receiver,
            initial_command,
            update_health,
        } = bootstrap;
        let service = MountService::new(paths.clone(), application_root());
        let settings = storage::load_settings(&paths).unwrap_or_default();
        let system_locale = Locale::system();
        let locale =
            Locale::from_preference(Language::from_value(&settings.language), system_locale);
        let (servers, status) = match storage::load_servers(&paths) {
            Ok(servers) => (servers, locale.text(TextKey::LoadingMountStatus).into()),
            Err(error) => (
                Vec::new(),
                match locale {
                    Locale::English => format!("Could not load existing configuration: {error}"),
                    Locale::Chinese => format!("无法加载现有配置：{error}"),
                },
            ),
        };
        let (main_window, open_window) = window::open(main_window_settings());
        let screen = if initial_command == AppCommand::ShowTransfers {
            Screen::TransferCenter
        } else {
            Screen::Connections
        };
        let (tray_action_sender, tray_actions) = async_channel::unbounded();
        let mut app = Self {
            paths,
            _instance_lock: instance_lock,
            _command_server: command_server,
            command_receiver,
            pending_commands: VecDeque::new(),
            settings,
            system_locale,
            servers,
            service,
            mount_statuses: HashMap::new(),
            busy: HashSet::new(),
            transfers: HashMap::new(),
            transfer_errors: HashMap::new(),
            transfer_failures: HashMap::new(),
            transfer_refreshing: false,
            main_window,
            main_window_ready: false,
            main_window_opening: true,
            pending_main_activation: false,
            tray: None,
            tray_action_sender,
            tray_actions: TraySubscription(tray_actions),
            tray_error: None,
            exit_confirmation_open: false,
            transfer_popup: None,
            transfer_popup_expanded: false,
            dismissed_popups: HashSet::new(),
            synced_polls: HashMap::new(),
            notification_tracker: TransferNotificationTracker::default(),
            pending_unmount_after_sync: HashSet::new(),
            confirmed_unmounts: HashSet::new(),
            popup_close_notice_shown: false,
            screen,
            connection_draft: None,
            connection_custom_mountpoint: String::new(),
            settings_draft: None,
            log_view: None,
            log_window: None,
            custom_setting: None,
            editor_saving: false,
            ssh_import_loading: false,
            ssh_import_plan: None,
            ssh_import_actions: Vec::new(),
            pending_delete: None,
            status,
            update_health,
            update_info: None,
            update_checking: false,
            update_error: None,
            update_downloading: false,
            update_progress: Arc::new(Mutex::new(UpdateDownloadProgress::default())),
            prepared_update: None,
            dependency_status: None,
            dependency_checking: false,
            capacities: HashMap::new(),
            capacity_errors: HashSet::new(),
            capacity_refreshing: false,
            capacity_refresh_pending: false,
        };
        let mut tasks = vec![
            open_window.map(Message::MainWindowOpened),
            app.status_task(),
        ];
        if screen == Screen::TransferCenter {
            tasks.push(app.transfer_task());
        }
        if app.settings.auto_check_updates {
            app.update_checking = true;
            tasks.push(app.check_update_task(false));
        }
        if app.settings.startup_all {
            tasks.push(app.reconcile_startup_task());
        }
        let task = Task::batch(tasks);
        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let locale = self.locale();
        match message {
            Message::AppCommand(command) => {
                diagnostic_trace(&format!("app received {command:?}"));
                return self.handle_app_command(command);
            }
            Message::TrayAction(action) => return self.handle_tray_action(action),
            Message::TrayTick => {
                if self.tray.is_some() {
                    TrayController::desktop_iteration();
                    self.sync_tray();
                }
            }
            Message::MainWindowOpened(id) => {
                if id == self.main_window {
                    diagnostic_trace(&format!("main window opened {id:?}"));
                    self.main_window_ready = true;
                    self.main_window_opening = false;
                    self.initialize_tray();
                    if let Some(authorization) = self.update_health.take()
                        && let Err(error) = write_update_health_marker(
                            &self.paths.update_state_dir(),
                            &authorization,
                        )
                    {
                        self.status = format!("Update health confirmation failed: {error}");
                    }
                    let native_smoke = native_integration_smoke_enabled();
                    let progress = if native_smoke {
                        GlobalProgressState::Normal {
                            completed: 1,
                            total: 2,
                        }
                    } else {
                        self.global_progress_state()
                    };
                    let mut tasks = vec![set_native_global_progress(id, progress)];
                    if native_smoke {
                        tasks.push(show_native_notification(
                            native_integration_smoke_notification(),
                        ));
                    }
                    if self.pending_main_activation {
                        self.pending_main_activation = false;
                        tasks.push(self.activate_main_window());
                    }
                    return Task::batch(tasks);
                }
            }
            Message::Refresh => match storage::load_servers(&self.paths) {
                Ok(servers) => {
                    self.servers = servers;
                    self.status = locale.text(TextKey::RefreshingMountStatus).into();
                    return self.status_task();
                }
                Err(error) => self.status = error.to_string(),
            },
            Message::RefreshFinished(result) => match result {
                Ok(result) => self.status = locale.refresh_complete(&result),
                Err(error) => self.status = error,
            },
            Message::StatusesLoaded(results) => {
                let mut errors = Vec::new();
                let mut unmounted = Vec::new();
                for (id, result) in results {
                    match result {
                        Ok(status) => {
                            self.mount_statuses.insert(id.clone(), status);
                            if status == MountStatus::Unmounted {
                                unmounted.push(id);
                            }
                        }
                        Err(error) => errors.push(error),
                    }
                }
                self.status = errors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| locale.text(TextKey::Ready).into());
                let mut tasks: Vec<_> = unmounted
                    .iter()
                    .map(|id| self.close_popups_for_server(id))
                    .collect();
                for id in &unmounted {
                    self.capacities.remove(id);
                    self.capacity_errors.remove(id);
                }
                tasks.push(self.capacity_task());
                tasks.push(set_native_global_progress(
                    self.main_window,
                    self.global_progress_state(),
                ));
                return Task::batch(tasks);
            }
            Message::TransferTick => return self.transfer_task(),
            Message::CapacityTick => return self.capacity_task(),
            Message::CapacitiesLoaded(results) => {
                self.capacity_refreshing = false;
                for (id, result) in results {
                    match result {
                        Ok(Some(capacity)) => {
                            self.capacities.insert(id.clone(), capacity);
                            self.capacity_errors.remove(&id);
                        }
                        Ok(None) => {
                            self.capacities.remove(&id);
                            self.capacity_errors.insert(id);
                        }
                        Err(_) => {
                            self.capacity_errors.insert(id);
                        }
                    }
                }
                if self.capacity_refresh_pending {
                    self.capacity_refresh_pending = false;
                    return self.capacity_task();
                }
            }
            Message::TransfersLoaded(results) => {
                self.transfer_refreshing = false;
                let mut notifications = Vec::new();
                for (id, result) in results {
                    match result {
                        Ok(snapshot) => {
                            self.transfer_failures.remove(&id);
                            let synced_polls = if snapshot.synced {
                                let polls = self.synced_polls.entry(id.clone()).or_default();
                                *polls = polls.saturating_add(1);
                                *polls
                            } else {
                                self.synced_polls.remove(&id);
                                0
                            };
                            let display_name = self
                                .servers
                                .iter()
                                .find(|server| server.id == id)
                                .map(|server| server.display_name().to_owned())
                                .unwrap_or_else(|| id.clone());
                            notifications.extend(self.notification_tracker.observe(
                                &id,
                                &display_name,
                                &snapshot,
                                synced_polls,
                                locale,
                            ));
                            self.transfers.insert(id.clone(), snapshot);
                            self.transfer_errors.remove(&id);
                        }
                        Err(error) => {
                            let failures = self.transfer_failures.entry(id.clone()).or_default();
                            *failures = failures.saturating_add(1);
                            if transfer_failure_is_visible(*failures) {
                                self.transfer_errors.insert(id, error);
                            }
                        }
                    }
                }
                let ready_unmounts = self
                    .pending_unmount_after_sync
                    .iter()
                    .filter(|id| {
                        self.transfers
                            .get(*id)
                            .is_some_and(|snapshot| snapshot.synced)
                            && !self.transfer_errors.contains_key(*id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for id in &ready_unmounts {
                    self.pending_unmount_after_sync.remove(id);
                    self.confirmed_unmounts.insert(id.clone());
                }
                let mut tasks = vec![
                    self.reconcile_transfer_popups(),
                    set_native_global_progress(self.main_window, self.global_progress_state()),
                ];
                tasks.extend(
                    ready_unmounts
                        .into_iter()
                        .map(|id| self.start_mount_operation(id, Some(MountOperation::Unmount))),
                );
                tasks.extend(notifications.into_iter().map(show_native_notification));
                return Task::batch(tasks);
            }
            Message::NotificationFinished(result) => match result {
                Ok(()) => diagnostic_trace("native notification submitted"),
                Err(error) => {
                    diagnostic_trace(&format!("native notification failed: {error}"));
                }
            },
            Message::PopupOpened(id) => {
                diagnostic_trace(&format!(
                    "shared transfer popup opened for {} connection(s) {id:?}",
                    self.active_transfer_ids().len()
                ));
                return configure_popup_window(id, transfer_popup_size(false));
            }
            Message::ClosePopup(id) => {
                let mut tasks = vec![window::close(id)];
                if self.transfer_popup == Some(id) {
                    self.dismissed_popups.extend(self.active_transfer_ids());
                    self.transfer_popup = None;
                    self.transfer_popup_expanded = false;
                    if !self.popup_close_notice_shown {
                        self.popup_close_notice_shown = true;
                        self.status = match locale {
                            Locale::English => {
                                "Transfer window hidden; uploads continue in the background".into()
                            }
                            Locale::Chinese => "传输窗口已隐藏；上传仍在后台继续".into(),
                        };
                        tasks.push(show_native_notification(background_transfer_notification(
                            locale,
                        )));
                    }
                }
                return Task::batch(tasks);
            }
            Message::TogglePopupDetails => {
                let Some(id) = self.transfer_popup else {
                    return Task::none();
                };
                self.transfer_popup_expanded = !self.transfer_popup_expanded;
                let size = transfer_popup_size(self.transfer_popup_expanded);
                return Task::batch([window::resize(id, size), configure_popup_window(id, size)]);
            }
            Message::CloseRequested(id) if id == self.main_window => {
                return self.hide_main_window();
            }
            Message::CloseRequested(id) if self.transfer_popup == Some(id) => {
                self.dismissed_popups.extend(self.active_transfer_ids());
                self.transfer_popup = None;
                self.transfer_popup_expanded = false;
                if !self.popup_close_notice_shown {
                    self.popup_close_notice_shown = true;
                    self.status = match locale {
                        Locale::English => {
                            "Transfer window hidden; uploads continue in the background".into()
                        }
                        Locale::Chinese => "传输窗口已隐藏；上传仍在后台继续".into(),
                    };
                    return Task::batch([
                        window::close(id),
                        show_native_notification(background_transfer_notification(locale)),
                    ]);
                }
                return window::close(id);
            }
            Message::CloseRequested(id) if self.log_window == Some(id) => {
                self.log_window = None;
                self.log_view = None;
                return window::close(id);
            }
            Message::CloseRequested(_) => {}
            Message::ExitDecision(confirmed) => {
                self.exit_confirmation_open = false;
                if confirmed {
                    return iced::exit();
                }
            }
            Message::WindowClosed(id) if id == self.main_window => {
                diagnostic_trace(&format!("main window closed {id:?}"));
                self.main_window_ready = false;
                self.main_window_opening = false;
                if self.tray.is_none() {
                    return iced::exit();
                }
            }
            Message::WindowClosed(id) => {
                if self.transfer_popup == Some(id) {
                    self.dismissed_popups.extend(self.active_transfer_ids());
                    self.transfer_popup = None;
                    self.transfer_popup_expanded = false;
                } else if self.log_window == Some(id) {
                    self.log_window = None;
                    self.log_view = None;
                }
            }
            Message::AddConnection => {
                let mut draft = ConnectionDraft::default();
                draft.ssh_config_path = default_ssh_config_path().display().to_string();
                self.connection_draft = Some(draft);
                self.connection_custom_mountpoint.clear();
                self.ssh_import_plan = None;
                self.ssh_import_actions.clear();
                self.screen = Screen::ConnectionEditor;
                self.status = locale.text(TextKey::NewConnection).into();
            }
            Message::OpenTransfers => {
                self.screen = Screen::TransferCenter;
                self.status = locale.text(TextKey::TransferCenter).into();
                return self.transfer_task();
            }
            Message::CloseTransfers => {
                if !self.active_transfer_ids().is_empty() {
                    let description = match locale {
                        Locale::English => {
                            "Uploads are still pending. Closing the transfer center only hides this view; transfers continue in the background and mounted drives must remain available. Close the view?"
                        }
                        Locale::Chinese => {
                            "仍有待上传任务。关闭传输中心只会隐藏此页面，传输仍在后台继续，挂载必须保持可用。是否关闭此页面？"
                        }
                    };
                    return Task::perform(
                        async move {
                            rfd::AsyncMessageDialog::new()
                                .set_title(APP_NAME)
                                .set_description(description)
                                .set_buttons(rfd::MessageButtons::YesNo)
                                .show()
                                .await
                        },
                        Message::CloseTransfersDecision,
                    );
                }
                self.screen = Screen::Connections;
                self.status = locale.text(TextKey::Ready).into();
            }
            Message::CloseTransfersDecision(result) => {
                if result == rfd::MessageDialogResult::Yes {
                    self.screen = Screen::Connections;
                    self.status = match locale {
                        Locale::English => {
                            "Transfer center hidden; uploads continue in the background".into()
                        }
                        Locale::Chinese => "传输中心已隐藏；上传仍在后台继续".into(),
                    };
                }
            }
            Message::OpenSettings => {
                self.settings_draft = Some(SettingsDraft::from_settings(&self.settings));
                self.screen = Screen::Settings;
                self.status = locale.text(TextKey::Settings).into();
                self.dependency_checking = true;
                return self.dependency_check_task();
            }
            Message::OpenLogChooser => return self.open_log_window(None),
            Message::LogWindowOpened(id) => {
                if self.log_window == Some(id) {
                    return window::gain_focus(id);
                }
            }
            Message::OpenLog(id) => return self.open_log(id),
            Message::ReloadLog => {
                let Some(log_view) = &mut self.log_view else {
                    return Task::none();
                };
                log_view.loading = true;
                self.status = locale.text(TextKey::LoadingLog).into();
                return load_log_task(log_view.server_id.clone(), log_view.path.clone());
            }
            Message::LogLoaded { id, result } => {
                let Some(log_view) = &mut self.log_view else {
                    return Task::none();
                };
                if log_view.server_id != id {
                    return Task::none();
                }
                log_view.loading = false;
                match result {
                    Ok(log) => {
                        log_view.path = log.path;
                        log_view.content = text_editor::Content::with_text(&log.content);
                        log_view
                            .content
                            .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
                        log_view.truncated = log.truncated;
                        self.status = locale.text(TextKey::Logs).into();
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::CopyLog => {
                let Some(log_view) = &self.log_view else {
                    return Task::none();
                };
                if log_view.content.is_empty() {
                    self.status = locale.text(TextKey::NoLogContent).into();
                    return Task::none();
                }
                self.status = locale.text(TextKey::LogCopied).into();
                return clipboard::write(
                    log_view
                        .content
                        .selection()
                        .filter(|selection| !selection.is_empty())
                        .unwrap_or_else(|| log_view.content.text()),
                );
            }
            Message::LogAction(action) => {
                if let Some(log_view) = &mut self.log_view {
                    apply_read_only_log_action(&mut log_view.content, action);
                }
            }
            Message::CloseLog => {
                self.log_view = None;
                if let Some(id) = self.log_window.take() {
                    return window::close(id);
                }
            }
            Message::CancelEditor => {
                if !self.editor_saving {
                    self.connection_draft = None;
                    self.settings_draft = None;
                    self.ssh_import_plan = None;
                    self.ssh_import_actions.clear();
                    self.screen = Screen::Connections;
                    self.status = self.locale().text(TextKey::Ready).into();
                    self.sync_tray();
                }
            }
            Message::ConnectionSourceChanged(source) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.source = source;
                    draft.apply_source_defaults();
                    if matches!(
                        source,
                        ConnectionSource::SshConfig | ConnectionSource::SshConfigBatch
                    ) {
                        if draft.ssh_config_path.trim().is_empty() {
                            draft.ssh_config_path = default_ssh_config_path().display().to_string();
                        }
                        return self.load_ssh_config();
                    }
                    self.ssh_import_plan = None;
                    self.ssh_import_actions.clear();
                }
            }
            Message::ConnectionFieldChanged(field, value) => {
                if let Some(draft) = &mut self.connection_draft {
                    match field {
                        ConnectionField::Name => draft.name = value,
                        ConnectionField::HostAlias => draft.host_alias = value,
                        ConnectionField::Host => draft.host = value,
                        ConnectionField::User => {
                            draft.user = value;
                            draft.apply_sai_name();
                        }
                        ConnectionField::Port => draft.port = value,
                        ConnectionField::KeyFile => draft.key_file = value,
                        ConnectionField::SshConfigPath => {
                            draft.ssh_config_path = value;
                            self.ssh_import_plan = None;
                            self.ssh_import_actions.clear();
                        }
                    }
                }
            }
            Message::RemoteBaseChanged(base) => {
                if let Some(draft) = &mut self.connection_draft {
                    let (_, suffix) = split_remote_path(&draft.remote_path);
                    draft.remote_path = compose_remote_path(&base, &suffix);
                }
            }
            Message::RemoteSuffixChanged(suffix) => {
                if let Some(draft) = &mut self.connection_draft {
                    let (base, _) = split_remote_path(&draft.remote_path);
                    draft.remote_path = compose_remote_path(&base, &suffix);
                }
            }
            Message::MountpointChoiceChanged(choice) => {
                if let Some(draft) = &mut self.connection_draft {
                    if mountpoint_choice(&draft.mountpoint) == "custom" {
                        self.connection_custom_mountpoint = draft.mountpoint.clone();
                    }
                    draft.mountpoint = match choice.as_str() {
                        "auto" => String::new(),
                        "home" => HOME_MOUNTPOINT_VALUE.into(),
                        "custom" => {
                            if self.connection_custom_mountpoint.is_empty() {
                                CUSTOM_MOUNTPOINT_PENDING.into()
                            } else {
                                self.connection_custom_mountpoint.clone()
                            }
                        }
                        drive => drive.to_owned(),
                    };
                }
            }
            Message::CustomMountpointChanged(value) => {
                self.connection_custom_mountpoint = value.clone();
                if let Some(draft) = &mut self.connection_draft {
                    draft.mountpoint = value;
                }
            }
            Message::BrowseMountpoint => {
                let title = match locale {
                    Locale::English => "Select the parent folder for this mount",
                    Locale::Chinese => "选择挂载点的父文件夹",
                };
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title(title)
                            .pick_folder()
                            .await
                            .map(|folder| folder.path().to_owned())
                    },
                    Message::MountpointPicked,
                );
            }
            Message::MountpointPicked(Some(parent)) => {
                let name = self
                    .connection_draft
                    .as_ref()
                    .map(|draft| draft.name.as_str())
                    .unwrap_or("mount");
                let path = suggested_mountpoint(&parent, name).display().to_string();
                self.connection_custom_mountpoint = path.clone();
                if let Some(draft) = &mut self.connection_draft {
                    draft.mountpoint = path;
                }
            }
            Message::MountpointPicked(None) => {}
            Message::ConnectionAuthChanged(auth) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.auth = auth;
                }
            }
            Message::ConnectionMethodChanged(method) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.connection_method = method;
                    if method == ConnectionMethod::Openssh {
                        draft.auth = AuthMethod::Key;
                    }
                }
            }
            Message::PasswordChanged(value) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.password = value.0;
                }
            }
            Message::KeyPassphraseChanged(value) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.key_passphrase = value.0;
                }
            }
            Message::ManagedSshChanged(value) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.ssh_config_managed = value;
                    if !value {
                        draft.copy_key_to_ssh_dir = false;
                    }
                }
            }
            Message::CopyKeyChanged(value) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.copy_key_to_ssh_dir = value;
                }
            }
            Message::LoadSshConfig => return self.load_ssh_config(),
            Message::BrowseSshConfig => {
                let title = locale.text(TextKey::SelectSshConfig);
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title(title)
                            .pick_file()
                            .await
                            .map(|file| file.path().to_owned())
                    },
                    Message::SshConfigPicked,
                );
            }
            Message::SshConfigPicked(Some(path)) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.ssh_config_path = path.display().to_string();
                    self.ssh_import_plan = None;
                    self.ssh_import_actions.clear();
                    return self.load_ssh_config();
                }
            }
            Message::SshConfigPicked(None) => {}
            Message::BrowsePrivateKey => {
                let title = locale.text(TextKey::SelectPrivateKey);
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title(title)
                            .pick_file()
                            .await
                            .map(|file| file.path().to_owned())
                    },
                    Message::PrivateKeyPicked,
                );
            }
            Message::PrivateKeyPicked(Some(path)) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.key_file = path.display().to_string();
                }
            }
            Message::PrivateKeyPicked(None) => {}
            Message::SshImportLoaded {
                config_path,
                result,
            } => {
                self.ssh_import_loading = false;
                let request_is_current = self.connection_draft.as_ref().is_some_and(|draft| {
                    matches!(
                        draft.source,
                        ConnectionSource::SshConfig | ConnectionSource::SshConfigBatch
                    ) && Path::new(draft.ssh_config_path.trim()) == config_path
                });
                if !request_is_current {
                    return Task::none();
                }
                match result {
                    Ok(plan) => {
                        self.ssh_import_actions = plan
                            .items
                            .iter()
                            .map(|item| item.default_action())
                            .collect();
                        if self
                            .connection_draft
                            .as_ref()
                            .is_some_and(|draft| draft.source == ConnectionSource::SshConfig)
                        {
                            let selected = self
                                .connection_draft
                                .as_ref()
                                .map(|draft| draft.host_alias.as_str())
                                .unwrap_or_default();
                            let server = plan
                                .items
                                .iter()
                                .filter_map(|item| item.server.as_ref())
                                .find(|server| server.host_alias == selected)
                                .or_else(|| plan.items.iter().find_map(|item| item.server.as_ref()))
                                .cloned();
                            if let (Some(draft), Some(server)) =
                                (&mut self.connection_draft, server)
                            {
                                draft.apply_imported_server(&server);
                            }
                        }
                        let valid = plan
                            .items
                            .iter()
                            .filter(|item| item.status != ImportStatus::Invalid)
                            .count();
                        self.status = locale.loaded_ssh_hosts(valid);
                        self.ssh_import_plan = Some(plan);
                    }
                    Err(error) => {
                        self.ssh_import_plan = None;
                        self.ssh_import_actions.clear();
                        self.status = error;
                    }
                }
            }
            Message::SshHostSelected(host_alias) => {
                let server = self
                    .ssh_import_plan
                    .as_ref()
                    .and_then(|plan| {
                        plan.items.iter().find_map(|item| {
                            item.server
                                .as_ref()
                                .filter(|server| server.host_alias == host_alias)
                        })
                    })
                    .cloned();
                if let (Some(draft), Some(server)) = (&mut self.connection_draft, server) {
                    draft.apply_imported_server(&server);
                }
            }
            Message::SshImportActionChanged(index, action) => {
                if let Some(selected) = self.ssh_import_actions.get_mut(index) {
                    *selected = action;
                }
            }
            Message::SaveConnection => return self.save_connection(),
            Message::ConnectionSaved(result) => {
                self.editor_saving = false;
                match result {
                    Ok(servers) => {
                        self.servers = servers;
                        self.connection_draft = None;
                        self.screen = Screen::Connections;
                        self.status = locale.text(TextKey::ConnectionSaved).into();
                        return self.status_task();
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::SettingsFieldChanged(field, value) => {
                if let Some(draft) = &mut self.settings_draft {
                    match field {
                        SettingsField::CacheRoot => draft.cache_root = value,
                    }
                }
            }
            Message::BrowseCacheRoot => {
                let title = locale.text(TextKey::SelectCacheDirectory);
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title(title)
                            .pick_folder()
                            .await
                            .map(|folder| folder.path().to_owned())
                    },
                    Message::CacheRootPicked,
                );
            }
            Message::CacheRootPicked(Some(path)) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.cache_root = path.display().to_string();
                }
            }
            Message::CacheRootPicked(None) => {}
            Message::CacheModeChanged(mode) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.cache_mode = mode;
                }
            }
            Message::SettingOptionChanged(option) => {
                if option.custom {
                    let current = self
                        .settings_draft
                        .as_ref()
                        .map(|draft| setting_value(draft, option.kind))
                        .unwrap_or_default();
                    let (digits, unit) = split_custom_setting(option.kind, current);
                    self.custom_setting = Some(CustomSettingDraft {
                        kind: option.kind,
                        digits,
                        unit,
                    });
                } else if let Some(draft) = &mut self.settings_draft {
                    set_setting_value(draft, option.kind, option.value);
                }
            }
            Message::CustomSettingDigitsChanged(value) => {
                if let Some(custom) = &mut self.custom_setting {
                    custom.digits = value
                        .chars()
                        .filter(|character| character.is_ascii_digit())
                        .collect();
                }
            }
            Message::CustomSettingUnitChanged(unit) => {
                if let Some(custom) = &mut self.custom_setting
                    && custom_units(custom.kind)
                        .iter()
                        .any(|allowed| *allowed == unit)
                {
                    custom.unit = unit;
                }
            }
            Message::SaveCustomSetting => {
                if let Some(custom) = self.custom_setting.take() {
                    if custom.digits.is_empty() {
                        self.status = match locale {
                            Locale::English => "Custom value must contain digits".into(),
                            Locale::Chinese => "自定义数值必须填写数字".into(),
                        };
                        self.custom_setting = Some(custom);
                    } else if let Some(draft) = &mut self.settings_draft {
                        set_setting_value(
                            draft,
                            custom.kind,
                            format!("{}{}", custom.digits, custom.unit),
                        );
                    }
                }
            }
            Message::CancelCustomSetting => self.custom_setting = None,
            Message::StartupAllChanged(value) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.startup_all = value;
                }
            }
            Message::AutoTransfersChanged(value) => {
                if value {
                    if let Some(draft) = &mut self.settings_draft {
                        draft.auto_show_transfers = true;
                    }
                } else {
                    let description = match locale {
                        Locale::English => {
                            "Uploads will continue in the background, but the transfer popup will no longer appear automatically. Files may still be waiting in the local cache after the file manager reports a copy as complete. Disable automatic transfer popups?"
                        }
                        Locale::Chinese => {
                            "上传仍会在后台继续，但传输弹窗将不再自动出现。文件管理器显示复制完成后，文件仍可能留在本地缓存等待上传。是否关闭自动显示传输弹窗？"
                        }
                    };
                    return Task::perform(
                        async move {
                            rfd::AsyncMessageDialog::new()
                                .set_title(APP_NAME)
                                .set_description(description)
                                .set_buttons(rfd::MessageButtons::YesNo)
                                .show()
                                .await
                        },
                        Message::AutoTransfersDecision,
                    );
                }
            }
            Message::AutoTransfersDecision(result) => {
                if result == rfd::MessageDialogResult::Yes
                    && let Some(draft) = &mut self.settings_draft
                {
                    draft.auto_show_transfers = false;
                }
            }
            Message::AutoUpdatesChanged(value) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.auto_check_updates = value;
                }
            }
            Message::CheckForUpdates => {
                if !self.update_checking && !self.update_downloading {
                    self.update_checking = true;
                    self.update_error = None;
                    return self.check_update_task(true);
                }
            }
            Message::UpdateChecked { manual, result } => {
                self.update_checking = false;
                match result {
                    Ok(info) if info.is_newer => {
                        self.status = match locale {
                            Locale::English => {
                                format!("Update {} is available", info.latest_version)
                            }
                            Locale::Chinese => format!("发现新版本 {}", info.latest_version),
                        };
                        self.update_info = Some(info);
                        self.update_error = None;
                    }
                    Ok(info) => {
                        self.update_info = None;
                        self.update_error = None;
                        if manual {
                            self.status = match locale {
                                Locale::English => {
                                    format!("SSH MountMate {} is up to date", info.current_version)
                                }
                                Locale::Chinese => {
                                    format!("SSH MountMate {} 已是最新版本", info.current_version)
                                }
                            };
                        }
                    }
                    Err(error) => {
                        if manual {
                            self.status = error.clone();
                            self.update_error = Some(error);
                        } else {
                            diagnostic_trace(&format!("automatic update check failed: {error}"));
                        }
                    }
                }
            }
            Message::DownloadUpdate => {
                if !self.update_downloading {
                    return self.prepare_update_task();
                }
            }
            Message::UpdatePrepared(result) => {
                self.update_downloading = false;
                match result {
                    Ok(prepared) => {
                        self.prepared_update = Some(prepared);
                        return self.confirm_prepared_update();
                    }
                    Err(error) => {
                        self.update_error = Some(error.clone());
                        self.status = error;
                    }
                }
            }
            Message::InstallUpdateDecision(install) => {
                if install {
                    return self.launch_prepared_update();
                }
                if let Some(prepared) = self.prepared_update.take() {
                    prepared.cancel();
                }
                self.status = match locale {
                    Locale::English => "Update installation was postponed".into(),
                    Locale::Chinese => "已暂缓安装更新".into(),
                };
            }
            Message::CheckDependencies => {
                if !self.dependency_checking {
                    self.dependency_checking = true;
                    return self.dependency_check_task();
                }
            }
            Message::DependenciesChecked(result) => {
                self.dependency_checking = false;
                match result {
                    Ok(status) => {
                        let missing = status.missing();
                        self.status = if missing.is_empty() {
                            match locale {
                                Locale::English => "All mount dependencies are available".into(),
                                Locale::Chinese => "挂载依赖均已就绪".into(),
                            }
                        } else {
                            match locale {
                                Locale::English => {
                                    format!("Missing dependencies: {}", missing.join(", "))
                                }
                                Locale::Chinese => {
                                    format!("缺少依赖：{}", missing.join("、"))
                                }
                            }
                        };
                        self.dependency_status = Some(status);
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::LanguageChanged(language) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.language = language;
                }
                self.status = self.locale().text(TextKey::Settings).into();
                self.sync_tray();
            }
            Message::RegisterFileManagerMenu => return self.file_manager_menu_task(true),
            Message::UnregisterFileManagerMenu => return self.file_manager_menu_task(false),
            Message::FileManagerMenuFinished(result) => match result {
                Ok(true) => self.status = locale.text(TextKey::FileManagerMenuRegistered).into(),
                Ok(false) => self.status = locale.text(TextKey::FileManagerMenuRemoved).into(),
                Err(error) => self.status = error,
            },
            Message::SaveSettings => return self.save_settings(),
            Message::SettingsSaved(result) => {
                self.editor_saving = false;
                match result {
                    Ok(settings) => {
                        self.settings = settings;
                        self.settings_draft = None;
                        self.screen = Screen::Connections;
                        self.status = locale.text(TextKey::SettingsSaved).into();
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::StartupReconciled(result) => {
                if let Err(error) = result {
                    diagnostic_trace(&format!("login startup reconciliation failed: {error}"));
                }
            }
            Message::Mount(id) => return self.start_mount_operation(id, None),
            Message::CancelPendingUnmount(id) => {
                self.pending_unmount_after_sync.remove(&id);
                self.status = match locale {
                    Locale::English => "Automatic unmount cancelled".into(),
                    Locale::Chinese => "已取消同步后自动卸载".into(),
                };
            }
            Message::UnmountWaitDecision { ids, result } => match result {
                rfd::MessageDialogResult::Yes => {
                    self.pending_unmount_after_sync.extend(ids);
                    self.status = match locale {
                        Locale::English => "Waiting for uploads to finish before unmounting".into(),
                        Locale::Chinese => "正在等待上传完成后自动取消挂载".into(),
                    };
                    return self.transfer_task();
                }
                rfd::MessageDialogResult::No => return self.confirm_immediate_unmount(ids),
                _ => {}
            },
            Message::UnmountNowDecision { ids, result } => {
                if result == rfd::MessageDialogResult::Yes {
                    self.confirmed_unmounts.extend(ids.iter().cloned());
                    return Task::batch(
                        ids.into_iter().map(|id| {
                            self.start_mount_operation(id, Some(MountOperation::Unmount))
                        }),
                    );
                }
            }
            Message::MountFinished {
                id,
                operation,
                result,
            } => {
                self.busy.remove(&id);
                let mut tasks = Vec::new();
                match result {
                    Ok(message) => {
                        self.mount_statuses.insert(
                            id.clone(),
                            match operation {
                                MountOperation::Mount => MountStatus::Mounted,
                                MountOperation::Unmount => MountStatus::Unmounted,
                            },
                        );
                        self.status = message;
                        if matches!(operation, MountOperation::Unmount) {
                            self.pending_unmount_after_sync.remove(&id);
                            self.confirmed_unmounts.remove(&id);
                            tasks.push(self.close_popups_for_server(&id));
                        } else {
                            tasks.push(self.capacity_task());
                            tasks.push(self.transfer_task());
                        }
                    }
                    Err(error) => {
                        self.status = error;
                        tasks.push(self.status_task());
                    }
                }
                if let Some(command) = self.take_pending_command(&id) {
                    tasks.push(self.handle_app_command(command));
                }
                tasks.push(set_native_global_progress(
                    self.main_window,
                    self.global_progress_state(),
                ));
                return Task::batch(tasks);
            }
            Message::Open(id) => return self.open_mountpoint(id),
            Message::OpenFinished(result) => match result {
                Ok(()) => self.status = locale.text(TextKey::OpenedMountpoint).into(),
                Err(error) => self.status = error,
            },
            Message::Edit(id) => {
                if self.can_modify(&id)
                    && let Some(server) = self.servers.iter().find(|server| server.id == id)
                {
                    self.connection_draft = Some(ConnectionDraft::from_server(server));
                    self.connection_custom_mountpoint = custom_mountpoint_value(&server.mountpoint);
                    if let Some(draft) = &mut self.connection_draft
                        && draft.ssh_config_path.trim().is_empty()
                    {
                        draft.ssh_config_path = default_ssh_config_path().display().to_string();
                    }
                    self.ssh_import_plan = None;
                    self.ssh_import_actions.clear();
                    self.screen = Screen::ConnectionEditor;
                    self.status = locale.editing(server.display_name());
                }
            }
            Message::Remove(id) => {
                if self.can_modify(&id) {
                    self.pending_delete = Some(id);
                }
            }
            Message::CancelRemove => self.pending_delete = None,
            Message::ConfirmRemove => {
                let Some(id) = self.pending_delete.take() else {
                    return Task::none();
                };
                if !self.can_modify(&id) {
                    self.status = locale.text(TextKey::UnmountBeforeRemove).into();
                    return Task::none();
                }
                self.editor_saving = true;
                let paths = self.paths.clone();
                let server = self.servers.iter().find(|server| server.id == id).cloned();
                self.status = locale.removing(&operation_display_name(server.as_ref(), &id));
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            if let Some(server) = server
                                && server.ssh_config_managed
                            {
                                remove_managed_ssh_server(&server)
                                    .map_err(|error| error.to_string())?;
                            }
                            storage::remove_server(&paths, &id).map_err(|error| error.to_string())
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                    },
                    Message::RemoveFinished,
                );
            }
            Message::RemoveFinished(result) => {
                self.editor_saving = false;
                match result {
                    Ok(servers) => {
                        self.servers = servers;
                        self.status = locale.text(TextKey::ConnectionRemoved).into();
                    }
                    Err(error) => self.status = error,
                }
            }
        }
        Task::none()
    }

    fn initialize_tray(&mut self) {
        if self.tray.is_some() || self.tray_error.is_some() {
            return;
        }
        match TrayController::new(self.locale(), self.tray_action_sender.clone()) {
            Ok(tray) => {
                self.tray = Some(tray);
                self.sync_tray();
                diagnostic_trace("tray initialized");
            }
            Err(error) => {
                diagnostic_trace(&format!("tray unavailable: {error}"));
                self.tray_error = Some(error.clone());
                self.status = self.locale().tray_unavailable(&error);
            }
        }
    }

    fn sync_tray(&mut self) {
        let locale = self.locale();
        let can_mount = self.servers.iter().any(|server| {
            !self.busy.contains(&server.id)
                && !matches!(
                    self.mount_statuses.get(&server.id),
                    Some(MountStatus::Mounted | MountStatus::Starting)
                )
        });
        let can_unmount = self.servers.iter().any(|server| {
            !self.busy.contains(&server.id) && self.paths.state_file(&server.id).exists()
        });
        if let Some(tray) = &mut self.tray {
            tray.sync(locale, can_mount, can_unmount);
        }
    }

    fn handle_tray_action(&mut self, action: TrayAction) -> Task<Message> {
        match action {
            TrayAction::ShowMain => self.show_main_window(),
            TrayAction::ShowTransfers => {
                self.screen = Screen::TransferCenter;
                self.status = self.locale().text(TextKey::TransferCenter).into();
                Task::batch([self.show_main_window(), self.transfer_task()])
            }
            TrayAction::MountAll => self.handle_app_command(AppCommand::MountAll),
            TrayAction::UnmountAll => self.handle_app_command(AppCommand::UnmountAll),
            TrayAction::Exit => self.request_exit(),
        }
    }

    fn handle_app_command(&mut self, command: AppCommand) -> Task<Message> {
        match command {
            AppCommand::ShowMain => self.show_main_window(),
            AppCommand::ShowTransfers => {
                self.screen = Screen::TransferCenter;
                self.status = self.locale().text(TextKey::TransferCenter).into();
                diagnostic_trace(&format!(
                    "transfer center shown with {} popup(s)",
                    usize::from(self.transfer_popup.is_some())
                ));
                Task::batch([self.show_main_window(), self.transfer_task()])
            }
            AppCommand::Mount { id } => self.handle_mount_command(id, MountOperation::Mount),
            AppCommand::Unmount { id } => self.handle_mount_command(id, MountOperation::Unmount),
            AppCommand::Open { id } => self.open_mountpoint(id),
            AppCommand::RefreshPath { path } => {
                self.status = self.locale().text(TextKey::Refreshing).into();
                let service = self.service.clone();
                let servers = self.servers.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            service
                                .refresh_path(&servers, &path)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                    },
                    Message::RefreshFinished,
                )
            }
            AppCommand::Refresh { id, relative_dir } => {
                if !self.servers.iter().any(|server| server.id == id) {
                    self.status = self.locale().text(TextKey::ConnectionGone).into();
                    return Task::none();
                }
                self.status = self.locale().text(TextKey::Refreshing).into();
                let service = self.service.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            service
                                .refresh(&id, &relative_dir, false)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                    },
                    Message::RefreshFinished,
                )
            }
            AppCommand::MountAll => {
                let ids = self
                    .servers
                    .iter()
                    .map(|server| server.id.clone())
                    .collect::<Vec<_>>();
                Task::batch(
                    ids.into_iter()
                        .map(|id| self.handle_mount_command(id, MountOperation::Mount)),
                )
            }
            AppCommand::UnmountAll => {
                let ids = self
                    .servers
                    .iter()
                    .filter(|server| self.paths.state_file(&server.id).exists())
                    .map(|server| server.id.clone())
                    .collect::<Vec<_>>();
                let (unsafe_ids, safe_ids): (Vec<_>, Vec<_>) = ids.into_iter().partition(|id| {
                    unmount_needs_confirmation(
                        self.transfers.get(id),
                        self.transfer_errors.contains_key(id),
                    )
                });
                let mut tasks = safe_ids
                    .into_iter()
                    .map(|id| self.handle_mount_command(id, MountOperation::Unmount))
                    .collect::<Vec<_>>();
                if !unsafe_ids.is_empty() {
                    tasks.push(self.confirm_waiting_unmount(unsafe_ids));
                }
                Task::batch(tasks)
            }
            AppCommand::ExitForReplacement => {
                if self.busy.is_empty() {
                    iced::exit()
                } else {
                    self.status = match self.locale() {
                        Locale::English => {
                            "Finish the active mount operation before replacing this version".into()
                        }
                        Locale::Chinese => "请等待当前挂载操作完成后再替换版本".into(),
                    };
                    Task::none()
                }
            }
        }
    }

    fn handle_mount_command(&mut self, id: String, operation: MountOperation) -> Task<Message> {
        if self.busy.contains(&id) {
            self.pending_commands.retain(|command| {
                !matches!(
                    command,
                    AppCommand::Mount { id: pending_id }
                        | AppCommand::Unmount { id: pending_id }
                        if pending_id.as_str() == id.as_str()
                )
            });
            let command = match operation {
                MountOperation::Mount => AppCommand::Mount { id },
                MountOperation::Unmount => AppCommand::Unmount { id },
            };
            self.pending_commands.push_back(command);
            return Task::none();
        }
        self.start_mount_operation(id, Some(operation))
    }

    fn take_pending_command(&mut self, id: &str) -> Option<AppCommand> {
        let position = self.pending_commands.iter().position(|command| {
            matches!(
                command,
                AppCommand::Mount { id: pending_id } | AppCommand::Unmount { id: pending_id }
                    if pending_id == id
            )
        })?;
        self.pending_commands.remove(position)
    }

    fn activate_main_window(&self) -> Task<Message> {
        window::minimize(self.main_window, false).chain(window::gain_focus(self.main_window))
    }

    fn show_main_window(&mut self) -> Task<Message> {
        if self.main_window_ready {
            diagnostic_trace("activating existing main window");
            self.activate_main_window()
        } else if self.main_window_opening {
            diagnostic_trace("main window already opening; activation queued");
            self.pending_main_activation = true;
            Task::none()
        } else {
            let (main_window, open_window) = window::open(main_window_settings());
            diagnostic_trace(&format!("opening replacement main window {main_window:?}"));
            self.main_window = main_window;
            self.main_window_opening = true;
            self.pending_main_activation = true;
            open_window.map(Message::MainWindowOpened)
        }
    }

    fn hide_main_window(&mut self) -> Task<Message> {
        if self.tray.is_none() {
            return self.request_exit();
        }
        self.status = self.locale().text(TextKey::RunningInBackground).into();
        let main_window = self.main_window;
        diagnostic_trace(&format!("closing main window to tray {main_window:?}"));
        self.main_window_ready = false;
        self.main_window_opening = false;
        window::close(main_window)
    }

    fn request_exit(&mut self) -> Task<Message> {
        if self.exit_confirmation_open {
            return Task::none();
        }
        let active = self
            .transfers
            .values()
            .filter(|snapshot| transfer_is_active(snapshot))
            .count();
        let unknown = self
            .servers
            .iter()
            .filter(|server| self.mount_statuses.get(&server.id) == Some(&MountStatus::Mounted))
            .filter(|server| {
                self.transfer_errors.contains_key(&server.id)
                    || !self.transfers.contains_key(&server.id)
            })
            .count();
        if active == 0 && unknown == 0 {
            return iced::exit();
        }
        self.exit_confirmation_open = true;
        let description = self.locale().exit_warning(active, unknown);
        Task::perform(
            async move {
                rfd::AsyncMessageDialog::new()
                    .set_title(APP_NAME)
                    .set_description(description)
                    .set_level(rfd::MessageLevel::Warning)
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    .await
            },
            |result| Message::ExitDecision(result == rfd::MessageDialogResult::Yes),
        )
    }

    fn can_modify(&self, id: &str) -> bool {
        !self.busy.contains(id)
            && self
                .mount_statuses
                .get(id)
                .copied()
                .unwrap_or(MountStatus::Unmounted)
                == MountStatus::Unmounted
    }

    fn save_connection(&mut self) -> Task<Message> {
        if self.editor_saving {
            return Task::none();
        }
        if self
            .connection_draft
            .as_ref()
            .is_some_and(|draft| draft.source == ConnectionSource::SshConfigBatch)
        {
            return self.save_ssh_batch();
        }
        let Some(draft) = &self.connection_draft else {
            return Task::none();
        };
        let validated = match draft.validate(&self.servers) {
            Ok(validated) => validated,
            Err(error) => {
                self.status = localize_draft_error(self.locale(), &error);
                return Task::none();
            }
        };
        self.editor_saving = true;
        self.status = self.locale().text(TextKey::SavingConnection).into();
        let service = self.service.clone();
        let paths = self.paths.clone();
        let locale = self.locale();
        let previous = draft
            .editing_id
            .as_deref()
            .and_then(|id| self.servers.iter().find(|server| server.id == id))
            .cloned();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let password = obscure_action(&service, &validated.password)?;
                    let key_passphrase = obscure_action(&service, &validated.key_passphrase)?;
                    let mut server = validated
                        .apply_secrets(password, key_passphrase)
                        .map_err(|error| localize_draft_error(locale, &error))?;
                    prepare_managed_ssh_server(&mut server, &Platform)
                        .map_err(|error| error.to_string())?;
                    let servers = storage::upsert_server(&paths, server.clone())
                        .map_err(|error| error.to_string())?;
                    if let Some(previous) = previous
                        && previous.ssh_config_managed
                        && (!server.ssh_config_managed
                            || previous.managed_ssh_config_path != server.managed_ssh_config_path)
                    {
                        remove_managed_ssh_server(&previous).map_err(|error| error.to_string())?;
                    }
                    Ok(servers)
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::ConnectionSaved,
        )
    }

    fn load_ssh_config(&mut self) -> Task<Message> {
        if self.ssh_import_loading {
            return Task::none();
        }
        let Some(draft) = &self.connection_draft else {
            return Task::none();
        };
        let config_path = PathBuf::from(draft.ssh_config_path.trim());
        if config_path.as_os_str().is_empty() {
            self.status = self.locale().text(TextKey::SshConfigPathRequired).into();
            return Task::none();
        }
        let existing = self.servers.clone();
        let protected: HashSet<_> = existing
            .iter()
            .filter(|server| !self.can_modify(&server.id))
            .map(|server| server.id.clone())
            .collect();
        let service = self.service.clone();
        let result_path = config_path.clone();
        self.ssh_import_loading = true;
        self.status = self
            .locale()
            .loading_path(&config_path.display().to_string());
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    service
                        .ssh_import_plan(&config_path, &existing, &protected)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::SshImportLoaded {
                config_path: result_path.clone(),
                result,
            },
        )
    }

    fn save_ssh_batch(&mut self) -> Task<Message> {
        let Some(plan) = &self.ssh_import_plan else {
            self.status = self.locale().text(TextKey::LoadSshBeforeImport).into();
            return Task::none();
        };
        let updates = match plan.apply(&self.ssh_import_actions, &self.servers) {
            Ok(updates) if !updates.is_empty() => updates,
            Ok(_) => {
                self.status = self.locale().text(TextKey::SelectSshHost).into();
                return Task::none();
            }
            Err(error) => {
                self.status = localize_draft_error(self.locale(), &error);
                return Task::none();
            }
        };
        self.editor_saving = true;
        self.status = self.locale().saving_connections(updates.len());
        let paths = self.paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    storage::upsert_servers(&paths, updates).map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::ConnectionSaved,
        )
    }

    fn save_settings(&mut self) -> Task<Message> {
        if self.editor_saving {
            return Task::none();
        }
        let Some(draft) = &self.settings_draft else {
            return Task::none();
        };
        let settings = match draft.build(&self.settings, self.locale()) {
            Ok(settings) => settings,
            Err(error) => {
                self.status = error;
                return Task::none();
            }
        };
        self.editor_saving = true;
        self.status = self.locale().text(TextKey::SavingSettings).into();
        let paths = self.paths.clone();
        let result_settings = settings.clone();
        let previous_settings = self.settings.clone();
        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                self.editor_saving = false;
                self.status = error.to_string();
                return Task::none();
            }
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    storage::save_settings(&paths, &settings).map_err(|error| error.to_string())?;
                    if let Err(error) =
                        Platform.set_login_startup(&executable, settings.startup_all)
                    {
                        let _ = storage::save_settings(&paths, &previous_settings);
                        let _ =
                            Platform.set_login_startup(&executable, previous_settings.startup_all);
                        return Err(error.to_string());
                    }
                    Ok(result_settings)
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::SettingsSaved,
        )
    }

    fn file_manager_menu_task(&mut self, register: bool) -> Task<Message> {
        self.status = self
            .locale()
            .text(if register {
                TextKey::RegisteringFileManagerMenu
            } else {
                TextKey::RemovingFileManagerMenu
            })
            .into();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    if register {
                        let executable = std::env::current_exe().map_err(|error| {
                            format!("Could not locate the current executable: {error}")
                        })?;
                        Platform
                            .register_file_manager_menu(&executable)
                            .map(|()| true)
                            .map_err(|error| error.to_string())
                    } else {
                        Platform
                            .unregister_file_manager_menu()
                            .map(|()| false)
                            .map_err(|error| error.to_string())
                    }
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::FileManagerMenuFinished,
        )
    }

    fn check_update_task(&self, manual: bool) -> Task<Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(|| {
                    check_for_updates(VERSION).map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::UpdateChecked { manual, result },
        )
    }

    fn reconcile_startup_task(&self) -> Task<Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
                    Platform
                        .set_login_startup(&executable, true)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::StartupReconciled,
        )
    }

    fn dependency_check_task(&self) -> Task<Message> {
        let paths = self.paths.clone();
        let app_root = application_root();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    check_dependencies(&paths, &app_root).map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::DependenciesChecked,
        )
    }

    fn prepare_update_task(&mut self) -> Task<Message> {
        let Some(asset) = self
            .update_info
            .as_ref()
            .and_then(|info| info.asset.clone())
        else {
            self.status = match self.locale() {
                Locale::English => "This release has no verified asset for this platform".into(),
                Locale::Chinese => "此版本没有适用于当前平台的已验证安装包".into(),
            };
            return Task::none();
        };
        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                self.status = error.to_string();
                return Task::none();
            }
        };
        if let Ok(mut progress) = self.update_progress.lock() {
            *progress = UpdateDownloadProgress::default();
        }
        self.update_downloading = true;
        self.update_error = None;
        self.status = match self.locale() {
            Locale::English => "Downloading and verifying update...".into(),
            Locale::Chinese => "正在下载并验证更新...".into(),
        };
        let paths = self.paths.clone();
        let progress = self.update_progress.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut report = |received, total| {
                        if let Ok(mut current) = progress.lock() {
                            *current = UpdateDownloadProgress { received, total };
                        }
                    };
                    prepare_update_install(
                        &paths,
                        &asset,
                        &executable,
                        vec!["--show-main".into()],
                        Some(&mut report),
                    )
                    .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::UpdatePrepared,
        )
    }

    fn confirm_prepared_update(&mut self) -> Task<Message> {
        let active = self
            .transfers
            .values()
            .filter(|snapshot| transfer_is_active(snapshot))
            .count();
        let unknown = self
            .servers
            .iter()
            .filter(|server| self.mount_statuses.get(&server.id) == Some(&MountStatus::Mounted))
            .filter(|server| {
                self.transfer_errors.contains_key(&server.id)
                    || !self.transfers.contains_key(&server.id)
            })
            .count();
        if active == 0 && unknown == 0 {
            return self.launch_prepared_update();
        }
        let description = match self.locale() {
            Locale::English => format!(
                "Installing restarts SSH MountMate while leaving rclone mounts running. {active} connection(s) still have pending transfer work and {unknown} mounted connection(s) have unknown transfer state. Install now?"
            ),
            Locale::Chinese => format!(
                "安装更新会重启 SSH MountMate，但 rclone 挂载会继续运行。当前有 {active} 个连接仍有待处理传输，另有 {unknown} 个已挂载连接的传输状态未知。现在安装吗？"
            ),
        };
        Task::perform(
            async move {
                rfd::AsyncMessageDialog::new()
                    .set_title(APP_NAME)
                    .set_description(description)
                    .set_level(rfd::MessageLevel::Warning)
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    .await
            },
            |result| Message::InstallUpdateDecision(result == rfd::MessageDialogResult::Yes),
        )
    }

    fn launch_prepared_update(&mut self) -> Task<Message> {
        let Some(prepared) = self.prepared_update.take() else {
            return Task::none();
        };
        match prepared.launch() {
            Ok(_) => {
                self.status = match self.locale() {
                    Locale::English => "Restarting into the verified update...".into(),
                    Locale::Chinese => "正在重启并应用已验证更新...".into(),
                };
                iced::exit()
            }
            Err(error) => {
                self.update_error = Some(error.to_string());
                self.status = error.to_string();
                Task::none()
            }
        }
    }

    fn status_task(&self) -> Task<Message> {
        let service = self.service.clone();
        let ids: Vec<_> = self
            .servers
            .iter()
            .map(|server| server.id.clone())
            .collect();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    ids.into_iter()
                        .map(|id| {
                            let result = service.status(&id).map_err(|error| error.to_string());
                            (id, result)
                        })
                        .collect()
                })
                .await
                .unwrap_or_else(|error| vec![(String::new(), Err(error.to_string()))])
            },
            Message::StatusesLoaded,
        )
    }

    fn capacity_task(&mut self) -> Task<Message> {
        if self.capacity_refreshing {
            self.capacity_refresh_pending = true;
            return Task::none();
        }
        let servers: Vec<_> = self
            .servers
            .iter()
            .filter(|server| {
                self.mount_statuses.get(&server.id) == Some(&MountStatus::Mounted)
                    && !self.busy.contains(&server.id)
            })
            .cloned()
            .collect();
        if servers.is_empty() {
            self.capacity_refreshing = false;
            self.capacity_refresh_pending = false;
            return Task::none();
        }
        for server in &servers {
            if !self.capacities.contains_key(&server.id) {
                self.capacity_errors.remove(&server.id);
            }
        }
        self.capacity_refreshing = true;
        let service = self.service.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    std::thread::scope(|scope| {
                        let tasks: Vec<_> = servers
                            .into_iter()
                            .map(|server| {
                                let service = service.clone();
                                scope.spawn(move || {
                                    let id = server.id.clone();
                                    let result = service
                                        .capacity(&server)
                                        .map_err(|error| error.to_string());
                                    (id, result)
                                })
                            })
                            .collect();
                        tasks
                            .into_iter()
                            .filter_map(|task| task.join().ok())
                            .collect()
                    })
                })
                .await
                .unwrap_or_else(|error| vec![(String::new(), Err(error.to_string()))])
            },
            Message::CapacitiesLoaded,
        )
    }

    fn transfer_task(&mut self) -> Task<Message> {
        if self.transfer_refreshing {
            return Task::none();
        }
        let ids: Vec<_> = self
            .servers
            .iter()
            .filter(|server| {
                self.mount_statuses.get(&server.id) == Some(&MountStatus::Mounted)
                    && !self.busy.contains(&server.id)
            })
            .map(|server| server.id.clone())
            .collect();
        if ids.is_empty() {
            return set_native_global_progress(self.main_window, self.global_progress_state());
        }
        self.transfer_refreshing = true;
        let service = self.service.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    std::thread::scope(|scope| {
                        let tasks: Vec<_> = ids
                            .into_iter()
                            .map(|id| {
                                let service = service.clone();
                                scope.spawn(move || {
                                    let result = service
                                        .transfer_snapshot(&id)
                                        .map_err(|error| error.to_string());
                                    (id, result)
                                })
                            })
                            .collect();
                        tasks
                            .into_iter()
                            .filter_map(|task| task.join().ok())
                            .collect()
                    })
                })
                .await
                .unwrap_or_default()
            },
            Message::TransfersLoaded,
        )
    }

    fn reconcile_transfer_popups(&mut self) -> Task<Message> {
        let active = self.active_transfer_ids();
        let mut tasks = Vec::new();

        retain_dismissed_transfers(
            &mut self.dismissed_popups,
            &active,
            &self.notification_tracker.observed_active,
        );

        if active.is_empty() {
            if self.transfer_popup.is_some()
                && !self.notification_tracker.observed_active.is_empty()
            {
                return Task::none();
            }
            self.dismissed_popups.clear();
            self.transfer_popup_expanded = false;
            if let Some(popup) = self.transfer_popup.take() {
                diagnostic_trace(&format!("shared transfer popup completed {popup:?}"));
                tasks.push(window::close(popup));
            }
            return Task::batch(tasks);
        }

        if self.settings.auto_show_transfers
            && self.transfer_popup.is_none()
            && active.iter().any(|id| !self.dismissed_popups.contains(id))
        {
            let (popup, open) = window::open(transfer_window_settings());
            self.transfer_popup = Some(popup);
            self.transfer_popup_expanded = false;
            tasks.push(open.map(Message::PopupOpened));
        }

        Task::batch(tasks)
    }

    fn active_transfer_ids(&self) -> HashSet<String> {
        self.transfers
            .iter()
            .filter(|(id, snapshot)| {
                transfer_is_active(snapshot)
                    && self.mount_statuses.get(*id) == Some(&MountStatus::Mounted)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn close_popups_for_server(&mut self, server_id: &str) -> Task<Message> {
        self.dismissed_popups.remove(server_id);
        self.transfers.remove(server_id);
        self.transfer_errors.remove(server_id);
        self.transfer_failures.remove(server_id);
        self.synced_polls.remove(server_id);
        self.notification_tracker.forget(server_id);
        self.reconcile_transfer_popups()
    }

    fn global_progress_state(&self) -> GlobalProgressState {
        let mounted = self
            .servers
            .iter()
            .filter(|server| self.mount_statuses.get(&server.id) == Some(&MountStatus::Mounted));
        let totals = transfer_totals(mounted.map(|server| {
            if self.transfer_errors.contains_key(&server.id) {
                None
            } else {
                self.transfers.get(&server.id)
            }
        }));
        let out_of_space = self.transfers.iter().any(|(id, snapshot)| {
            self.mount_statuses.get(id) == Some(&MountStatus::Mounted) && snapshot.out_of_space
        });

        global_progress_state(&totals, out_of_space)
    }

    fn start_mount_operation(
        &mut self,
        id: String,
        requested: Option<MountOperation>,
    ) -> Task<Message> {
        let current_status = self.mount_statuses.get(&id).copied();
        let mounted = matches!(
            current_status,
            Some(MountStatus::Mounted | MountStatus::Starting)
        );
        let operation = requested.unwrap_or(if mounted {
            MountOperation::Unmount
        } else {
            MountOperation::Mount
        });
        if operation == MountOperation::Unmount
            && !self.confirmed_unmounts.remove(&id)
            && unmount_needs_confirmation(
                self.transfers.get(&id),
                self.transfer_errors.contains_key(&id),
            )
        {
            return self.confirm_waiting_unmount(vec![id]);
        }
        if !self.busy.insert(id.clone()) {
            return Task::none();
        }
        if (operation == MountOperation::Mount && mounted)
            || (operation == MountOperation::Unmount
                && current_status == Some(MountStatus::Unmounted))
        {
            self.busy.remove(&id);
            return Task::none();
        }
        self.mount_statuses
            .insert(id.clone(), MountStatus::Starting);
        let server = self.servers.iter().find(|server| server.id == id).cloned();
        let display_name = operation_display_name(server.as_ref(), &id);
        self.status = match operation {
            MountOperation::Mount => self.locale().mounting(&display_name),
            MountOperation::Unmount => self.locale().unmounting(&display_name),
        };
        let service = self.service.clone();
        let settings = self.settings.clone();
        let locale = self.locale();
        let result_id = id.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || match operation {
                    MountOperation::Mount => {
                        let server = server
                            .ok_or_else(|| locale.text(TextKey::ConnectionGone).to_owned())?;
                        service
                            .mount(&server, &settings)
                            .map(|state| match locale {
                                Locale::English => format!(
                                    "Mounted {} at {}",
                                    display_name,
                                    state.mountpoint.display()
                                ),
                                Locale::Chinese => format!(
                                    "已将 {} 挂载到 {}",
                                    display_name,
                                    state.mountpoint.display()
                                ),
                            })
                            .map_err(|error| error.to_string())
                    }
                    MountOperation::Unmount => service
                        .unmount(&id)
                        .map(|()| match locale {
                            Locale::English => format!("Unmounted {display_name}"),
                            Locale::Chinese => format!("已卸载 {display_name}"),
                        })
                        .map_err(|error| error.to_string()),
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::MountFinished {
                id: result_id.clone(),
                operation,
                result,
            },
        )
    }

    fn confirm_waiting_unmount(&self, ids: Vec<String>) -> Task<Message> {
        let locale = self.locale();
        let names = ids
            .iter()
            .map(|id| {
                operation_display_name(self.servers.iter().find(|server| server.id == *id), id)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let description = match locale {
            Locale::English => format!(
                "Uploads are pending or their state is unknown for: {names}.\n\nChoose Yes to wait and unmount automatically after synchronization. Choose No to review the immediate-unmount warning."
            ),
            Locale::Chinese => format!(
                "以下挂载仍有待上传内容，或传输状态未知：{names}。\n\n选择“是”将在同步完成后自动取消挂载；选择“否”可继续查看立即取消挂载的风险提示。"
            ),
        };
        let result_ids = ids.clone();
        Task::perform(
            async move {
                rfd::AsyncMessageDialog::new()
                    .set_title(APP_NAME)
                    .set_description(description)
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    .await
            },
            move |result| Message::UnmountWaitDecision {
                ids: result_ids.clone(),
                result,
            },
        )
    }

    fn confirm_immediate_unmount(&self, ids: Vec<String>) -> Task<Message> {
        let description = match self.locale() {
            Locale::English => {
                "Unmounting now can leave files only in the local VFS cache. A later remount may resume them, but that is not guaranteed after cache cleanup, configuration changes, or disk failure. Unmount now?"
            }
            Locale::Chinese => {
                "立即取消挂载可能使文件只留在本地 VFS 缓存中。之后重新挂载有时能够续传，但缓存清理、配置变化或磁盘故障后无法保证恢复。是否立即取消挂载？"
            }
        };
        let result_ids = ids.clone();
        Task::perform(
            async move {
                rfd::AsyncMessageDialog::new()
                    .set_title(APP_NAME)
                    .set_description(description)
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    .await
            },
            move |result| Message::UnmountNowDecision {
                ids: result_ids.clone(),
                result,
            },
        )
    }

    fn open_mountpoint(&mut self, id: String) -> Task<Message> {
        let state_file = self.paths.state_file(&id);
        let locale = self.locale();
        let display_name =
            operation_display_name(self.servers.iter().find(|server| server.id == id), &id);
        self.status = locale.opening(&display_name);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let state: MountState =
                        read_json(&state_file).map_err(|error| error.to_string())?;
                    open_path(&state.mountpoint, locale)
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::OpenFinished,
        )
    }

    fn open_log(&mut self, id: String) -> Task<Message> {
        let Some(server) = self.servers.iter().find(|server| server.id == id) else {
            self.status = self.locale().text(TextKey::ConnectionGone).into();
            return Task::none();
        };
        let display_name = server.display_name().to_owned();
        let path = self.paths.mount_log(server.remote_name());
        self.log_view = Some(MountLogView {
            server_id: id.clone(),
            display_name,
            path: path.clone(),
            content: text_editor::Content::new(),
            truncated: false,
            loading: true,
        });
        self.status = self.locale().text(TextKey::LoadingLog).into();
        let load = load_log_task(id, path);
        if let Some(window) = self.log_window {
            Task::batch([load, window::gain_focus(window)])
        } else {
            let (window, open) = window::open(log_window_settings());
            self.log_window = Some(window);
            Task::batch([load, open.map(Message::LogWindowOpened)])
        }
    }

    fn open_log_window(&mut self, initial: Option<String>) -> Task<Message> {
        if let Some(id) = initial {
            return self.open_log(id);
        }
        if let Some(window) = self.log_window {
            return window::gain_focus(window);
        }
        let (window, open) = window::open(log_window_settings());
        self.log_window = Some(window);
        open.map(Message::LogWindowOpened)
    }

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        if window == self.main_window {
            match self.screen {
                Screen::Connections => self.main_view(),
                Screen::TransferCenter => self.transfer_center_view(),
                Screen::ConnectionEditor => self.connection_editor_view(),
                Screen::Settings => self.settings_view(),
            }
        } else if self.log_window == Some(window) {
            self.log_viewer_view()
        } else {
            self.transfer_popup_view(window)
        }
    }

    fn main_view(&self) -> Element<'_, Message> {
        let locale = self.locale();
        let active_transfers = self
            .servers
            .iter()
            .filter(|server| {
                self.mount_statuses.get(&server.id) == Some(&MountStatus::Mounted)
                    && self
                        .transfers
                        .get(&server.id)
                        .is_some_and(transfer_is_active)
            })
            .count();
        let transfers_label = if active_transfers == 0 {
            locale.text(TextKey::Transfers).into()
        } else {
            format!("{} ({active_transfers})", locale.text(TextKey::Transfers))
        };
        let can_mount_all = self.servers.iter().any(|server| {
            !self.busy.contains(&server.id)
                && !matches!(
                    self.mount_statuses.get(&server.id),
                    Some(MountStatus::Mounted | MountStatus::Starting)
                )
        });
        let can_unmount_all = self.servers.iter().any(|server| {
            !self.busy.contains(&server.id)
                && (matches!(
                    self.mount_statuses.get(&server.id),
                    Some(MountStatus::Mounted | MountStatus::Starting)
                ) || self.paths.state_file(&server.id).exists())
        });
        let toolbar = row![
            text(APP_NAME).size(28),
            Space::new().width(Fill),
            button(locale.text(TextKey::Refresh)).on_press(Message::Refresh),
            button(text(transfers_label)).on_press(Message::OpenTransfers),
            button(locale.text(TextKey::AddConnection)).on_press(Message::AddConnection),
            button(locale.text(TextKey::Settings)).on_press(Message::OpenSettings),
        ]
        .spacing(10)
        .align_y(Center);
        let batch_actions = row![
            button(locale.text(TextKey::MountAll))
                .on_press_maybe(can_mount_all.then_some(Message::AppCommand(AppCommand::MountAll))),
            button(locale.text(TextKey::UnmountAll)).on_press_maybe(
                can_unmount_all.then_some(Message::AppCommand(AppCommand::UnmountAll))
            ),
        ]
        .spacing(10);

        let mut connections = column![].spacing(8);
        if self.servers.is_empty() {
            connections = connections.push(
                container(text(locale.text(TextKey::NoSavedConnections)).size(20))
                    .padding(28)
                    .width(Fill)
                    .center_x(Fill),
            );
        } else {
            for server in &self.servers {
                connections = connections.push(connection_card(
                    server,
                    ConnectionCardState {
                        status: self
                            .mount_statuses
                            .get(&server.id)
                            .copied()
                            .unwrap_or(MountStatus::Unmounted),
                        busy: self.busy.contains(&server.id),
                        transfer: self.transfers.get(&server.id),
                        transfer_unavailable: self.transfer_errors.contains_key(&server.id),
                        capacity: self.capacities.get(&server.id),
                        capacity_checking: self.capacity_refreshing
                            && !self.capacities.contains_key(&server.id)
                            && !self.capacity_errors.contains(&server.id),
                        can_modify: self.can_modify(&server.id),
                        confirming_remove: self.pending_delete.as_deref() == Some(&server.id),
                        waiting_unmount: self.pending_unmount_after_sync.contains(&server.id),
                    },
                    locale,
                ));
            }
        }

        container(
            column![
                toolbar,
                batch_actions,
                scrollable(connections).height(Fill),
                row![text(&self.status), Space::new().width(Fill), text(VERSION)],
            ]
            .spacing(14),
        )
        .padding(18)
        .width(Fill)
        .height(Fill)
        .into()
    }

    fn transfer_center_view(&self) -> Element<'_, Message> {
        let locale = self.locale();
        let mounted: Vec<_> = self
            .servers
            .iter()
            .filter(|server| self.mount_statuses.get(&server.id) == Some(&MountStatus::Mounted))
            .collect();
        let has_mounted_connections = !mounted.is_empty();
        let unavailable_connections = mounted
            .iter()
            .filter(|server| self.transfer_errors.contains_key(&server.id))
            .count();
        let totals = transfer_totals(mounted.iter().map(|server| {
            if self.transfer_errors.contains_key(&server.id) {
                None
            } else {
                self.transfers.get(&server.id)
            }
        }));
        let summary = if mounted.is_empty() {
            locale.text(TextKey::NoMountedConnections).into()
        } else if totals.out_of_space {
            match locale {
                Locale::English => "A local VFS cache is out of space".into(),
                Locale::Chinese => "本地 VFS 缓存空间不足".into(),
            }
        } else if totals.errors > 0 {
            match locale {
                Locale::English => format!(
                    "{} upload error(s) across {} mounted connection(s)",
                    totals.errors,
                    mounted.len()
                ),
                Locale::Chinese => format!(
                    "{} 个上传错误，涉及 {} 个已挂载连接",
                    totals.errors,
                    mounted.len()
                ),
            }
        } else if unavailable_connections > 0 && totals.pending_files > 0 {
            match locale {
                Locale::English => format!(
                    "{} file(s) pending; transfer state unavailable for {} connection(s)",
                    totals.pending_files, unavailable_connections
                ),
                Locale::Chinese => format!(
                    "{} 个文件待传；{} 个连接的传输状态不可用",
                    totals.pending_files, unavailable_connections
                ),
            }
        } else if unavailable_connections > 0 {
            match locale {
                Locale::English => format!(
                    "Transfer state unavailable for {} mounted connection(s)",
                    unavailable_connections
                ),
                Locale::Chinese => {
                    format!("{} 个已挂载连接的传输状态不可用", unavailable_connections)
                }
            }
        } else if totals.pending_files > 0 && totals.progress_available {
            match locale {
                Locale::English => format!(
                    "{} file(s) pending - {} of {} uploaded",
                    totals.pending_files,
                    format_bytes(totals.transferred_bytes),
                    format_bytes(totals.total_bytes)
                ),
                Locale::Chinese => format!(
                    "{} 个文件待传 - 已上传 {} / {}",
                    totals.pending_files,
                    format_bytes(totals.transferred_bytes),
                    format_bytes(totals.total_bytes)
                ),
            }
        } else if totals.pending_files > 0 {
            match locale {
                Locale::English => format!(
                    "{} file(s) pending - exact overall progress unavailable",
                    totals.pending_files
                ),
                Locale::Chinese => {
                    format!("{} 个文件待传 - 暂无精确总体进度", totals.pending_files)
                }
            }
        } else if totals.unknown_connections > unavailable_connections {
            match locale {
                Locale::English => format!(
                    "Checking {} mounted connection(s)",
                    totals.unknown_connections - unavailable_connections
                ),
                Locale::Chinese => format!(
                    "正在检查 {} 个已挂载连接",
                    totals.unknown_connections - unavailable_connections
                ),
            }
        } else {
            locale.text(TextKey::AllCloudSynced).into()
        };
        let header = row![
            text(locale.text(TextKey::TransferCenter)).size(28),
            Space::new().width(Fill),
            button(if self.transfer_refreshing {
                locale.text(TextKey::Refreshing)
            } else {
                locale.text(TextKey::RefreshNow)
            })
            .on_press_maybe((!self.transfer_refreshing).then_some(Message::TransferTick)),
            button(locale.text(TextKey::Back)).on_press(Message::CloseTransfers),
        ]
        .spacing(10)
        .align_y(Center);

        let mut overview = column![text(summary).size(18)].spacing(8);
        if totals.pending_files > 0 || totals.errors > 0 || totals.out_of_space {
            overview = overview
                .push(progress_bar(0.0..=100.0, totals.percentage as f32))
                .push(text(if totals.progress_available {
                match locale {
                    Locale::English => format!(
                        "{} uploading, {} queued, {} error(s)",
                        totals.uploading, totals.queued, totals.errors
                    ),
                    Locale::Chinese => format!(
                        "上传中 {}，排队 {}，错误 {}",
                        totals.uploading, totals.queued, totals.errors
                    ),
                }
            } else {
                match locale {
                    Locale::English => format!(
                        "{} uploading, {} queued, {} error(s) - overall percentage unavailable",
                        totals.uploading, totals.queued, totals.errors
                    ),
                    Locale::Chinese => format!(
                        "上传中 {}，排队 {}，错误 {} - 暂无总体百分比",
                        totals.uploading, totals.queued, totals.errors
                    ),
                }
                })
                .size(13));
        }

        let mut connections = column![].spacing(10);
        for server in mounted {
            connections = connections.push(transfer_connection_view(
                server,
                self.transfers.get(&server.id),
                self.transfer_errors.get(&server.id),
                locale,
            ));
        }
        if !has_mounted_connections {
            connections = connections.push(
                container(text(locale.text(TextKey::MountConnectionForTransfers)).size(16))
                    .padding(28)
                    .width(Fill)
                    .center_x(Fill),
            );
        }

        container(
            column![
                header,
                overview,
                scrollable(connections).height(Fill),
                row![text(&self.status), Space::new().width(Fill), text(VERSION)],
            ]
            .spacing(16),
        )
        .padding(18)
        .width(Fill)
        .height(Fill)
        .into()
    }

    fn connection_editor_view(&self) -> Element<'_, Message> {
        let locale = self.locale();
        let Some(draft) = &self.connection_draft else {
            return container(text(locale.text(TextKey::ConnectionEditorUnavailable))).into();
        };
        let title = if draft.source == ConnectionSource::SshConfigBatch {
            locale.text(TextKey::ImportSshConfig)
        } else if draft.editing_id.is_some() {
            locale.text(TextKey::EditConnection)
        } else {
            locale.text(TextKey::AddConnection)
        };
        let header = row![
            text(title).size(28),
            Space::new().width(Fill),
            button(locale.text(TextKey::Cancel))
                .on_press_maybe((!self.editor_saving).then_some(Message::CancelEditor)),
            button(if self.editor_saving {
                locale.text(TextKey::Saving)
            } else if draft.source == ConnectionSource::SshConfigBatch {
                locale.text(TextKey::Import)
            } else {
                locale.text(TextKey::Save)
            })
            .on_press_maybe((!self.editor_saving).then_some(Message::SaveConnection)),
        ]
        .spacing(10)
        .align_y(Center);

        let source_options = localized_choices(
            ConnectionSource::ALL.into_iter().filter(|source| {
                draft.editing_id.is_none() || *source != ConnectionSource::SshConfigBatch
            }),
            locale,
            Locale::connection_source,
        );
        let source = labeled_control(
            locale.text(TextKey::Source),
            pick_list(
                source_options,
                Some(locale.choice(draft.source, locale.connection_source(draft.source))),
                |source| Message::ConnectionSourceChanged(source.value),
            )
            .width(Fill),
        );
        let mut ssh_config_controls = column![].spacing(12);
        if matches!(
            draft.source,
            ConnectionSource::SshConfig | ConnectionSource::SshConfigBatch
        ) {
            ssh_config_controls = ssh_config_controls.push(
                row![
                    connection_input(
                        locale.text(TextKey::SshConfigFile),
                        &draft.ssh_config_path,
                        ConnectionField::SshConfigPath,
                    ),
                    button(locale.text(TextKey::Browse)).on_press(Message::BrowseSshConfig),
                    button(if self.ssh_import_loading {
                        locale.text(TextKey::Loading)
                    } else {
                        locale.text(TextKey::Load)
                    })
                    .on_press_maybe(
                        (!self.ssh_import_loading && !self.editor_saving)
                            .then_some(Message::LoadSshConfig),
                    ),
                ]
                .spacing(12)
                .align_y(Center),
            );
        }
        if draft.source == ConnectionSource::SshConfigBatch {
            let mut items = column![].spacing(8);
            if let Some(plan) = &self.ssh_import_plan {
                for (index, item) in plan.items.iter().enumerate() {
                    let allowed_values = if item.status == ImportStatus::New {
                        vec![ImportAction::Import, ImportAction::Ignore]
                    } else if item.can_overwrite {
                        vec![ImportAction::Ignore, ImportAction::Overwrite]
                    } else {
                        vec![ImportAction::Ignore]
                    };
                    let action = self
                        .ssh_import_actions
                        .get(index)
                        .copied()
                        .unwrap_or(ImportAction::Ignore);
                    let allowed = localized_choices(allowed_values, locale, Locale::import_action);
                    let selected_action = locale.choice(action, locale.import_action(action));
                    let reason = localized_import_reason(
                        locale,
                        item.status,
                        item.overwrite_protected,
                        &item.reason,
                    );
                    let target = item
                        .server
                        .as_ref()
                        .map(|server| format!("{}@{}:{}", server.user, server.host, server.port))
                        .unwrap_or_else(|| reason.clone());
                    let mut details = column![
                        text(&item.host_alias).size(17),
                        text(target).size(13),
                        text(locale.import_status(item.status)).size(12),
                    ]
                    .spacing(3)
                    .width(Fill);
                    if !reason.is_empty() {
                        details = details.push(text(reason).size(12));
                    }
                    items = items.push(
                        container(
                            row![
                                details,
                                pick_list(allowed, Some(selected_action), move |action| {
                                    Message::SshImportActionChanged(index, action.value)
                                })
                                .width(Length::Fixed(150.0)),
                            ]
                            .spacing(12)
                            .align_y(Center),
                        )
                        .padding(12)
                        .width(Fill)
                        .style(container::rounded_box),
                    );
                }
            }
            let content = column![source, ssh_config_controls, items]
                .spacing(16)
                .max_width(900);
            return editor_shell(header, scrollable(content), &self.status);
        }
        if draft.source == ConnectionSource::SshConfig {
            let hosts: Vec<_> = self
                .ssh_import_plan
                .as_ref()
                .into_iter()
                .flat_map(|plan| &plan.items)
                .filter_map(|item| item.server.as_ref())
                .map(|server| server.host_alias.clone())
                .collect();
            if !hosts.is_empty() {
                ssh_config_controls = ssh_config_controls.push(labeled_control(
                    locale.text(TextKey::SshHost),
                    pick_list(
                        hosts,
                        Some(draft.host_alias.clone()),
                        Message::SshHostSelected,
                    )
                    .width(Fill),
                ));
            }
        }
        let identity = row![
            connection_input(
                locale.text(TextKey::Name),
                &draft.name,
                ConnectionField::Name
            ),
            connection_input(
                locale.text(TextKey::SshHostAlias),
                &draft.host_alias,
                ConnectionField::HostAlias,
            ),
        ]
        .spacing(12);
        let target = row![
            connection_input(
                locale.text(TextKey::IpHost),
                &draft.host,
                ConnectionField::Host
            ),
            connection_input(
                locale.text(TextKey::User),
                &draft.user,
                ConnectionField::User
            ),
            connection_input(
                locale.text(TextKey::Port),
                &draft.port,
                ConnectionField::Port
            )
            .width(Length::Fixed(150.0)),
        ]
        .spacing(12);
        let authentication: Element<'_, Message> =
            if draft.connection_method == ConnectionMethod::Openssh {
                container(text(locale.text(TextKey::ManagedByOpenSsh)))
                    .padding(10)
                    .width(Fill)
                    .into()
            } else {
                pick_list(
                    localized_choices(AuthMethod::ALL, locale, Locale::auth_method),
                    Some(locale.choice(draft.auth, locale.auth_method(draft.auth))),
                    |auth| Message::ConnectionAuthChanged(auth.value),
                )
                .width(Fill)
                .into()
            };
        let transport = row![
            labeled_control(
                locale.text(TextKey::Transport),
                pick_list(
                    localized_choices(ConnectionMethod::ALL, locale, Locale::connection_method,),
                    Some(locale.choice(
                        draft.connection_method,
                        locale.connection_method(draft.connection_method),
                    )),
                    |method| Message::ConnectionMethodChanged(method.value),
                )
                .width(Fill),
            ),
            labeled_control(locale.text(TextKey::Authentication), authentication),
        ]
        .spacing(12);

        let mut auth_fields = column![].spacing(12);
        if draft.connection_method == ConnectionMethod::Native {
            match draft.auth {
                AuthMethod::Password => {
                    auth_fields = auth_fields.push(labeled_control(
                        locale.text(TextKey::Password),
                        text_input(locale.text(TextKey::PasswordRequired), &draft.password)
                            .secure(true)
                            .on_input(|value| Message::PasswordChanged(SecretInput(value)))
                            .width(Fill),
                    ));
                }
                AuthMethod::Key => {
                    auth_fields = auth_fields.push(
                        row![
                            connection_file_input(
                                locale.text(TextKey::PrivateKeyFile),
                                &draft.key_file,
                                ConnectionField::KeyFile,
                                Message::BrowsePrivateKey,
                                locale.text(TextKey::Browse),
                            ),
                            labeled_control(
                                locale.text(TextKey::KeyPassphrase),
                                text_input(locale.text(TextKey::Optional), &draft.key_passphrase)
                                    .secure(true)
                                    .on_input(|value| Message::KeyPassphraseChanged(SecretInput(
                                        value
                                    )))
                                    .width(Fill),
                            ),
                        ]
                        .spacing(12),
                    );
                }
            }
        }
        let mut managed_fields = column![].spacing(10);
        if matches!(
            draft.source,
            ConnectionSource::Manual | ConnectionSource::SaiCluster
        ) {
            managed_fields = managed_fields.push(
                checkbox(draft.ssh_config_managed)
                    .label(locale.text(TextKey::WriteManagedProfile))
                    .on_toggle(Message::ManagedSshChanged),
            );
            if draft.ssh_config_managed && draft.auth == AuthMethod::Key {
                managed_fields = managed_fields.push(
                    checkbox(draft.copy_key_to_ssh_dir)
                        .label(locale.text(TextKey::CopyPrivateKey))
                        .on_toggle(Message::CopyKeyChanged),
                );
            }
        }
        let (remote_base, remote_suffix) = split_remote_path(&draft.remote_path);
        let remote_path = column![
            text(locale.text(TextKey::RemotePath)).size(13),
            row![
                pick_list(
                    vec!["$HOME".to_owned(), "/".to_owned()],
                    Some(remote_base),
                    Message::RemoteBaseChanged,
                )
                .width(Length::Fixed(120.0)),
                text_input(locale.text(TextKey::RemotePath), &remote_suffix)
                    .on_input(Message::RemoteSuffixChanged)
                    .width(Fill),
            ]
            .spacing(8),
        ]
        .spacing(5)
        .width(Fill);
        let mountpoint_choice = mountpoint_choice(&draft.mountpoint);
        let custom_mountpoint = mountpoint_choice == "custom";
        let mut mountpoint = column![
            row![
                text(locale.text(TextKey::Mountpoint)).size(13),
                settings_help(mountpoint_help(locale)),
            ]
            .spacing(5),
            pick_list(
                mountpoint_options(locale),
                Some(mountpoint_option_label(&mountpoint_choice, locale)),
                move |label| Message::MountpointChoiceChanged(mountpoint_option_value(
                    &label, locale
                )),
            )
            .width(Fill),
        ]
        .spacing(5)
        .width(Fill);
        if custom_mountpoint {
            let custom_value = if draft.mountpoint == CUSTOM_MOUNTPOINT_PENDING {
                &self.connection_custom_mountpoint
            } else {
                &draft.mountpoint
            };
            mountpoint = mountpoint.push(
                row![
                    text_input(locale.text(TextKey::Mountpoint), custom_value,)
                        .on_input(Message::CustomMountpointChanged)
                        .width(Fill),
                    button(locale.text(TextKey::Browse)).on_press(Message::BrowseMountpoint),
                ]
                .spacing(8),
            );
        }
        let paths = row![remote_path, mountpoint].spacing(12);
        let content = column![
            source,
            ssh_config_controls,
            identity,
            target,
            transport,
            auth_fields,
            managed_fields,
            paths
        ]
        .spacing(16)
        .max_width(900);
        editor_shell(header, scrollable(content), &self.status)
    }

    fn settings_view(&self) -> Element<'_, Message> {
        let locale = self.locale();
        let Some(draft) = &self.settings_draft else {
            return container(text(locale.text(TextKey::SettingsUnavailable))).into();
        };
        let header = row![
            text(locale.text(TextKey::Settings)).size(28),
            Space::new().width(Fill),
            button(locale.text(TextKey::Cancel))
                .on_press_maybe((!self.editor_saving).then_some(Message::CancelEditor)),
            button(if self.editor_saving {
                locale.text(TextKey::Saving)
            } else {
                locale.text(TextKey::Save)
            })
            .on_press_maybe((!self.editor_saving).then_some(Message::SaveSettings)),
        ]
        .spacing(10)
        .align_y(Center);
        let cache_profile = row![
            settings_folder_input(
                locale.text(TextKey::CacheRoot),
                &draft.cache_root,
                SettingsField::CacheRoot,
                Message::BrowseCacheRoot,
                locale.text(TextKey::Browse),
            ),
            column![
                row![
                    text(locale.text(TextKey::VfsCacheMode)).size(14),
                    settings_help(vfs_cache_mode_help(locale)),
                ]
                .spacing(5),
                pick_list(
                    localized_choices(CacheMode::ALL, locale, Locale::cache_mode),
                    Some(locale.choice(draft.cache_mode, locale.cache_mode(draft.cache_mode),)),
                    |mode| Message::CacheModeChanged(mode.value)
                )
                .width(Fill),
            ]
            .spacing(5),
        ]
        .spacing(12);
        let cache_limits = row![
            setting_picker(
                SettingKind::MaxSize,
                locale.text(TextKey::MaximumSize),
                &draft.max_size,
                locale,
            ),
            setting_picker(
                SettingKind::MaxAge,
                locale.text(TextKey::MaximumAge),
                &draft.max_age,
                locale,
            ),
            setting_picker(
                SettingKind::MinFreeSpace,
                locale.text(TextKey::MinimumFreeSpace),
                &draft.min_free_space,
                locale,
            ),
        ]
        .spacing(12);
        let cache_timing = row![
            setting_picker(
                SettingKind::WriteBack,
                locale.text(TextKey::WriteBackDelay),
                &draft.write_back,
                locale,
            ),
            setting_picker(
                SettingKind::DirCacheTime,
                locale.text(TextKey::DirectoryCacheTime),
                &draft.dir_cache_time,
                locale,
            ),
            setting_picker(
                SettingKind::BufferSize,
                locale.text(TextKey::BufferSize),
                &draft.buffer_size,
                locale,
            ),
        ]
        .spacing(12);
        let behavior = column![
            toggler(draft.startup_all)
                .label(locale.text(TextKey::MountAllAtLogin))
                .on_toggle(Message::StartupAllChanged),
            row![
                toggler(draft.auto_show_transfers)
                    .label(locale.text(TextKey::ShowTransferPopup))
                    .on_toggle(Message::AutoTransfersChanged),
                settings_help(auto_transfer_help(locale)),
            ]
            .spacing(5)
            .align_y(Center),
            toggler(draft.auto_check_updates)
                .label(locale.text(TextKey::CheckUpdatesAutomatically))
                .on_toggle(Message::AutoUpdatesChanged),
            labeled_control(
                locale.text(TextKey::Language),
                pick_list(
                    localized_choices(Language::ALL, locale, Locale::language),
                    Some(locale.choice(draft.language, locale.language(draft.language))),
                    |language| Message::LanguageChanged(language.value)
                )
                .width(Fill),
            ),
        ]
        .spacing(14)
        .max_width(440);
        let tray_capability = self
            .tray_error
            .as_ref()
            .map(|error| text(locale.tray_unavailable(error)).size(14));
        let file_manager = if file_manager_settings_visible(std::env::consts::OS) {
            column![
                text(locale.text(TextKey::FileManagerIntegration)).size(20),
                text(locale.text(TextKey::FileManagerIntegrationHelp)).size(14),
                row![
                    button(locale.text(TextKey::RegisterFileManagerMenu))
                        .on_press(Message::RegisterFileManagerMenu),
                    button(locale.text(TextKey::RemoveFileManagerMenu))
                        .on_press(Message::UnregisterFileManagerMenu),
                ]
                .spacing(10),
            ]
            .spacing(8)
        } else {
            column![]
        };
        let logs = container(
            row![
                column![
                    text(locale.text(TextKey::Logs)).size(20),
                    text(locale.text(TextKey::LogsHelp)).size(14),
                ]
                .spacing(4)
                .width(Fill),
                button(locale.text(TextKey::ViewLog)).on_press(Message::OpenLogChooser),
            ]
            .spacing(12)
            .align_y(Center),
        )
        .max_width(640);
        let mut dependency_section = column![
            text(match locale {
                Locale::English => "Mount dependencies",
                Locale::Chinese => "挂载依赖",
            })
            .size(20)
        ]
        .spacing(6)
        .max_width(640);
        if self.dependency_checking {
            dependency_section = dependency_section.push(text(match locale {
                Locale::English => "Checking dependencies...",
                Locale::Chinese => "正在检查依赖...",
            }));
        } else if let Some(dependencies) = &self.dependency_status {
            let available = |value: bool| match (locale, value) {
                (Locale::English, true) => "Available",
                (Locale::English, false) => "Missing",
                (Locale::Chinese, true) => "已就绪",
                (Locale::Chinese, false) => "缺失",
            };
            dependency_section = dependency_section
                .push(text(format!(
                    "rclone: {}",
                    available(dependencies.rclone.is_some())
                )))
                .push(text(format!(
                    "{}: {}",
                    dependencies.mount_dependency,
                    available(dependencies.mount_dependency_installed)
                )))
                .push(text(format!(
                    "OpenSSH: {}",
                    available(dependencies.openssh.is_some())
                )));
        }
        dependency_section = dependency_section.push(
            button(match locale {
                Locale::English => "Check again",
                Locale::Chinese => "重新检查",
            })
            .on_press_maybe((!self.dependency_checking).then_some(Message::CheckDependencies)),
        );
        let update_title = match locale {
            Locale::English => "Software updates",
            Locale::Chinese => "软件更新",
        };
        let current_version = match locale {
            Locale::English => format!("Current version: {VERSION}"),
            Locale::Chinese => format!("当前版本：{VERSION}"),
        };
        let mut update_section =
            column![text(update_title).size(20), text(current_version).size(14)]
                .spacing(8)
                .max_width(640);
        if self.update_checking {
            update_section = update_section.push(text(match locale {
                Locale::English => "Checking for updates...",
                Locale::Chinese => "正在检查更新...",
            }));
        } else if let Some(info) = &self.update_info {
            update_section = update_section.push(text(match locale {
                Locale::English => format!("{} is available", info.latest_version),
                Locale::Chinese => format!("可更新至 {}", info.latest_version),
            }));
            if info.asset.is_none() {
                update_section = update_section.push(text(match locale {
                    Locale::English => "A verified package is not available for this platform",
                    Locale::Chinese => "当前平台暂无已验证的安装包",
                }));
            }
        }
        if self.update_downloading {
            let progress = self
                .update_progress
                .lock()
                .map(|progress| *progress)
                .unwrap_or_default();
            let percentage = if progress.total == 0 {
                0.0
            } else {
                progress.received as f32 * 100.0 / progress.total as f32
            };
            update_section = update_section
                .push(progress_bar(0.0..=100.0, percentage.clamp(0.0, 100.0)))
                .push(text(match locale {
                    Locale::English => format!(
                        "Downloaded {} of {}",
                        format_bytes(progress.received),
                        if progress.total == 0 {
                            "unknown".into()
                        } else {
                            format_bytes(progress.total)
                        }
                    ),
                    Locale::Chinese => format!(
                        "已下载 {} / {}",
                        format_bytes(progress.received),
                        if progress.total == 0 {
                            "未知大小".into()
                        } else {
                            format_bytes(progress.total)
                        }
                    ),
                }));
        }
        if let Some(error) = &self.update_error {
            update_section = update_section.push(text(error).size(13));
        }
        let check_label = match locale {
            Locale::English => "Check now",
            Locale::Chinese => "立即检查",
        };
        let install_label = match locale {
            Locale::English => "Download and install",
            Locale::Chinese => "下载并安装",
        };
        let check = button(check_label).on_press_maybe(
            (!self.update_checking && !self.update_downloading).then_some(Message::CheckForUpdates),
        );
        let install = button(install_label).on_press_maybe(
            self.update_info
                .as_ref()
                .is_some_and(|info| info.asset.is_some())
                .then_some(Message::DownloadUpdate)
                .filter(|_| !self.update_downloading),
        );
        update_section = update_section.push(row![check, install].spacing(10));
        let content = column![
            cache_profile,
            cache_limits,
            cache_timing,
            behavior,
            logs,
            dependency_section,
            update_section,
            tray_capability,
            file_manager
        ]
        .spacing(18)
        .max_width(900);
        let base = editor_shell(header, scrollable(content), &self.status);
        if let Some(custom) = &self.custom_setting {
            let units = custom_units(custom.kind)
                .iter()
                .map(|unit| (*unit).to_owned())
                .collect::<Vec<_>>();
            let title = match locale {
                Locale::English => "Custom setting",
                Locale::Chinese => "自定义设置",
            };
            let dialog = container(
                column![
                    text(title).size(22),
                    row![
                        text_input("0", &custom.digits)
                            .on_input(Message::CustomSettingDigitsChanged)
                            .width(Length::Fixed(180.0)),
                        pick_list(
                            units,
                            Some(custom.unit.clone()),
                            Message::CustomSettingUnitChanged,
                        )
                        .width(Length::Fixed(120.0)),
                    ]
                    .spacing(10),
                    row![
                        button(locale.text(TextKey::Cancel)).on_press(Message::CancelCustomSetting),
                        button(locale.text(TextKey::Save)).on_press(Message::SaveCustomSetting),
                    ]
                    .spacing(10),
                ]
                .spacing(14),
            )
            .padding(20)
            .width(Length::Fixed(380.0))
            .style(container::rounded_box);
            stack![
                base,
                container(dialog)
                    .width(Fill)
                    .height(Fill)
                    .center_x(Fill)
                    .center_y(Fill),
            ]
            .into()
        } else {
            base
        }
    }

    fn log_viewer_view(&self) -> Element<'_, Message> {
        let locale = self.locale();
        let selected = self.log_view.as_ref().map(|log| log.server_id.clone());
        let choices = self
            .servers
            .iter()
            .map(|server| LogChoice {
                id: server.id.clone(),
                label: server.display_name().to_owned(),
            })
            .collect::<Vec<_>>();
        let selected_choice = selected
            .as_ref()
            .and_then(|id| choices.iter().find(|choice| &choice.id == id).cloned());
        let selector = pick_list(choices, selected_choice, |choice| {
            Message::OpenLog(choice.id)
        })
        .placeholder(match locale {
            Locale::English => "Select a mount log",
            Locale::Chinese => "选择挂载日志",
        })
        .width(Length::Fixed(260.0));
        let Some(log_view) = &self.log_view else {
            return editor_shell(
                row![
                    text(locale.text(TextKey::Logs)).size(28),
                    Space::new().width(Fill),
                    selector,
                    button("x").on_press(Message::CloseLog),
                ]
                .spacing(10)
                .align_y(Center),
                container(text(locale.text(TextKey::NoLogContent)))
                    .center_x(Fill)
                    .center_y(Fill),
                &self.status,
            );
        };
        let header = row![
            text(format!(
                "{} — {}",
                locale.text(TextKey::Logs),
                log_view.display_name
            ))
            .size(28),
            Space::new().width(Fill),
            selector,
            button(locale.text(TextKey::Refresh))
                .on_press_maybe((!log_view.loading).then_some(Message::ReloadLog)),
            button(locale.text(TextKey::CopyLog))
                .on_press_maybe((!log_view.content.is_empty()).then_some(Message::CopyLog)),
            button("x").on_press(Message::CloseLog),
        ]
        .spacing(10)
        .align_y(Center);
        let mut details = column![text(log_view.path.display().to_string()).size(13)].spacing(6);
        if log_view.loading {
            details = details.push(text(locale.text(TextKey::LoadingLog)));
        }
        if log_view.truncated {
            details = details.push(text(locale.text(TextKey::LogTruncated)).size(13));
        }
        let log_content: Element<'_, Message> = if log_view.content.is_empty() {
            container(text(locale.text(TextKey::NoLogContent)).size(13))
                .padding(12)
                .width(Fill)
                .height(Fill)
                .style(container::rounded_box)
                .into()
        } else {
            text_editor(&log_view.content)
                .on_action(Message::LogAction)
                .size(13)
                .height(Fill)
                .into()
        };
        let content = column![details, log_content].spacing(10).height(Fill);
        editor_shell(header, content, &self.status)
    }

    fn transfer_popup_view(&self, window: window::Id) -> Element<'_, Message> {
        let locale = self.locale();
        if self.transfer_popup != Some(window) {
            return container(text(locale.text(TextKey::TransferCompleted)))
                .padding(16)
                .width(Fill)
                .height(Fill)
                .into();
        }
        let active: Vec<_> = self
            .servers
            .iter()
            .filter(|server| {
                self.mount_statuses.get(&server.id) == Some(&MountStatus::Mounted)
                    && self
                        .transfers
                        .get(&server.id)
                        .is_some_and(transfer_is_active)
            })
            .collect();
        let totals = transfer_totals(active.iter().map(|server| self.transfers.get(&server.id)));
        let summary = popup_transfer_summary(locale, &totals, active.len());
        let current_file = active
            .iter()
            .find_map(|server| {
                self.transfers.get(&server.id).and_then(|snapshot| {
                    snapshot
                        .files
                        .iter()
                        .find(|file| file.uploading)
                        .map(|file| {
                            let filename = Path::new(&file.name)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy();
                            format!(
                                "{}: {} - {}/s",
                                server.display_name(),
                                filename,
                                format_bytes(file.speed.max(0.0) as u64)
                            )
                        })
                })
            })
            .unwrap_or_else(|| {
                if active.iter().any(|server| {
                    self.transfers
                        .get(&server.id)
                        .is_some_and(|snapshot| snapshot.queued > 0 && snapshot.uploading == 0)
                }) {
                    match locale {
                        Locale::English => {
                            "Queued locally; waiting for write-back delay or upload slot".into()
                        }
                        Locale::Chinese => "已在本地排队，等待写回延迟或上传槽位".into(),
                    }
                } else {
                    locale.text(TextKey::WaitingRemoteConfirmation).into()
                }
            });

        let header = row![
            text(locale.text(TextKey::FileTransfer)).size(18),
            Space::new().width(Fill),
            button(if self.transfer_popup_expanded {
                locale.text(TextKey::HideDetails)
            } else {
                locale.text(TextKey::ShowDetails)
            })
            .on_press(Message::TogglePopupDetails),
            button("x").on_press(Message::ClosePopup(window)),
        ]
        .spacing(8)
        .align_y(Center);
        let mut content = column![
            header,
            text(summary).size(14),
            progress_bar(0.0..=100.0, totals.percentage as f32),
            text(current_file).size(12),
        ]
        .spacing(8);

        if self.transfer_popup_expanded {
            let mut details = column![].spacing(8);
            for server in active {
                details = details.push(transfer_connection_view(
                    server,
                    self.transfers.get(&server.id),
                    self.transfer_errors.get(&server.id),
                    locale,
                ));
            }
            content = content.push(scrollable(details).height(Fill));
        }

        container(content)
            .padding(14)
            .width(Fill)
            .height(Fill)
            .into()
    }
}

struct ConnectionCardState<'a> {
    status: MountStatus,
    busy: bool,
    transfer: Option<&'a TransferSnapshot>,
    transfer_unavailable: bool,
    capacity: Option<&'a CapacityInfo>,
    capacity_checking: bool,
    can_modify: bool,
    confirming_remove: bool,
    waiting_unmount: bool,
}

fn capacity_progress_view(
    capacity: Option<&CapacityInfo>,
    checking: bool,
    locale: Locale,
) -> Element<'static, Message> {
    let (percentage, label) = capacity_progress_state(capacity, checking, locale);
    stack![
        progress_bar(0.0..=100.0, percentage).girth(Length::Fixed(22.0)),
        container(text(label).size(12))
            .width(Fill)
            .height(Length::Fixed(22.0))
            .center_x(Fill)
            .center_y(Length::Fixed(22.0)),
    ]
    .width(Fill)
    .height(Length::Fixed(22.0))
    .into()
}

fn capacity_progress_state(
    capacity: Option<&CapacityInfo>,
    checking: bool,
    locale: Locale,
) -> (f32, String) {
    if let Some(capacity) = capacity {
        (
            capacity.percent as f32,
            match locale {
                Locale::English => format!(
                    "Capacity: {} / {} used ({}%)",
                    format_bytes(capacity.used),
                    format_bytes(capacity.total),
                    capacity.percent
                ),
                Locale::Chinese => format!(
                    "容量：已用 {} / {}（{}%）",
                    format_bytes(capacity.used),
                    format_bytes(capacity.total),
                    capacity.percent
                ),
            },
        )
    } else if checking {
        (
            0.0,
            match locale {
                Locale::English => "Capacity: checking...".into(),
                Locale::Chinese => "容量：查询中...".into(),
            },
        )
    } else {
        (
            0.0,
            match locale {
                Locale::English => "Capacity: unknown".into(),
                Locale::Chinese => "容量：未知".into(),
            },
        )
    }
}

fn connection_card<'a>(
    server: &'a ServerConfig,
    state: ConnectionCardState<'a>,
    locale: Locale,
) -> Element<'a, Message> {
    let ConnectionCardState {
        status,
        busy,
        transfer,
        transfer_unavailable,
        capacity,
        capacity_checking,
        can_modify,
        confirming_remove,
        waiting_unmount,
    } = state;
    let id = server.id.clone();
    let host = format!("{}@{}:{}", server.user, server.host, server.port);
    let remote = if server.remote_path.is_empty() {
        "~".to_owned()
    } else {
        server.remote_path.clone()
    };
    let operation_label = if waiting_unmount {
        match locale {
            Locale::English => "Cancel pending unmount",
            Locale::Chinese => "取消等待卸载",
        }
    } else if matches!(status, MountStatus::Mounted | MountStatus::Starting) {
        locale.text(TextKey::Unmount)
    } else {
        locale.text(TextKey::Mount)
    };
    let mut operation = button(operation_label);
    if waiting_unmount {
        operation = operation.on_press(Message::CancelPendingUnmount(id.clone()));
    } else if !busy {
        operation = operation.on_press(Message::Mount(id.clone()));
    }
    let mut open = button(locale.text(TextKey::Open));
    if status == MountStatus::Mounted && !busy {
        open = open.on_press(Message::Open(id.clone()));
    }
    let mut details = column![
        text(server.display_name()).size(22),
        text(host).size(15),
        text(format!(
            "{}  ->  {}",
            remote,
            display_mountpoint(server, locale)
        ))
        .size(14),
        text(status_label(locale, status)).size(13),
    ]
    .spacing(4)
    .width(Fill);
    if status == MountStatus::Mounted {
        details = details.push(capacity_progress_view(capacity, capacity_checking, locale));
        if transfer_unavailable {
            details = details.push(text(locale.text(TextKey::TransferStateUnavailable)).size(13));
        } else if let Some(snapshot) = transfer.filter(|snapshot| transfer_is_active(snapshot)) {
            details = details
                .push(text(transfer_label(locale, snapshot)).size(13))
                .push(progress_bar(0.0..=100.0, snapshot.percentage as f32));
        }
    }
    let edit = button(locale.text(TextKey::Edit))
        .on_press_maybe(can_modify.then(|| Message::Edit(id.clone())));
    let actions: Element<'_, Message> = if confirming_remove {
        row![
            button(locale.text(TextKey::Cancel)).on_press(Message::CancelRemove),
            button(locale.text(TextKey::ConfirmRemove)).on_press(Message::ConfirmRemove),
        ]
        .spacing(8)
        .into()
    } else {
        row![
            edit,
            button(locale.text(TextKey::Remove))
                .on_press_maybe(can_modify.then_some(Message::Remove(id))),
        ]
        .spacing(8)
        .into()
    };
    container(
        row![details, operation, open, actions]
            .spacing(8)
            .align_y(Center),
    )
    .padding(16)
    .width(Fill)
    .style(container::rounded_box)
    .into()
}

fn connection_input<'a>(
    label: &'a str,
    value: &'a str,
    field: ConnectionField,
) -> iced::widget::Column<'a, Message> {
    labeled_control(
        label,
        text_input(label, value)
            .on_input(move |value| Message::ConnectionFieldChanged(field, value))
            .width(Fill),
    )
}

fn connection_file_input<'a>(
    label: &'a str,
    value: &'a str,
    field: ConnectionField,
    browse: Message,
    browse_label: &'a str,
) -> iced::widget::Column<'a, Message> {
    labeled_control(
        label,
        row![
            text_input(label, value)
                .on_input(move |value| Message::ConnectionFieldChanged(field, value))
                .width(Fill),
            button(browse_label).on_press(browse),
        ]
        .spacing(8),
    )
}

fn setting_picker<'a>(
    kind: SettingKind,
    label: &'a str,
    value: &'a str,
    locale: Locale,
) -> iced::widget::Column<'a, Message> {
    let options = setting_options(kind, locale);
    let selected = options
        .iter()
        .find(|option| !option.custom && option.value == value)
        .cloned()
        .or_else(|| options.iter().find(|option| option.custom).cloned());
    column![
        row![
            text(label).size(13),
            settings_help(setting_help(kind, locale))
        ]
        .spacing(5),
        pick_list(options, selected, Message::SettingOptionChanged).width(Fill),
        text(if setting_presets(kind).contains(&value) {
            ""
        } else {
            value
        })
        .size(12),
    ]
    .spacing(5)
    .width(Fill)
}

fn setting_options(kind: SettingKind, locale: Locale) -> Vec<SettingOption> {
    let mut options = setting_presets(kind)
        .iter()
        .map(|value| SettingOption {
            kind,
            value: (*value).to_owned(),
            label: preset_label(kind, value, locale),
            custom: false,
        })
        .collect::<Vec<_>>();
    options.push(SettingOption {
        kind,
        value: String::new(),
        label: match locale {
            Locale::English => "Custom...".into(),
            Locale::Chinese => "自定义...".into(),
        },
        custom: true,
    });
    options
}

fn setting_presets(kind: SettingKind) -> &'static [&'static str] {
    match kind {
        SettingKind::MaxSize => &["", "10G", "50G", "100G"],
        SettingKind::MaxAge => &["30m", "1h", "6h", "24h"],
        SettingKind::MinFreeSpace => &["", "5G", "10G", "20G"],
        SettingKind::WriteBack => &["0s", "5s", "30s", "1m"],
        SettingKind::DirCacheTime => &["30s", "5m", "15m", "1h"],
        SettingKind::BufferSize => &["", "0", "16Mi", "64Mi"],
    }
}

fn preset_label(kind: SettingKind, value: &str, locale: Locale) -> String {
    if value.is_empty() {
        return match (kind, locale) {
            (SettingKind::MaxSize, Locale::English) => "No size limit".into(),
            (SettingKind::MaxSize, Locale::Chinese) => "不限制大小".into(),
            (SettingKind::MinFreeSpace, Locale::English) => "Off".into(),
            (SettingKind::MinFreeSpace, Locale::Chinese) => "关闭".into(),
            (_, Locale::English) => "rclone default".into(),
            (_, Locale::Chinese) => "rclone 默认值".into(),
        };
    }
    value.to_owned()
}

fn setting_value(draft: &SettingsDraft, kind: SettingKind) -> &str {
    match kind {
        SettingKind::MaxSize => &draft.max_size,
        SettingKind::MaxAge => &draft.max_age,
        SettingKind::MinFreeSpace => &draft.min_free_space,
        SettingKind::WriteBack => &draft.write_back,
        SettingKind::DirCacheTime => &draft.dir_cache_time,
        SettingKind::BufferSize => &draft.buffer_size,
    }
}

fn set_setting_value(draft: &mut SettingsDraft, kind: SettingKind, value: String) {
    match kind {
        SettingKind::MaxSize => draft.max_size = value,
        SettingKind::MaxAge => draft.max_age = value,
        SettingKind::MinFreeSpace => draft.min_free_space = value,
        SettingKind::WriteBack => draft.write_back = value,
        SettingKind::DirCacheTime => draft.dir_cache_time = value,
        SettingKind::BufferSize => draft.buffer_size = value,
    }
}

fn custom_units(kind: SettingKind) -> &'static [&'static str] {
    match kind {
        SettingKind::MaxSize | SettingKind::MinFreeSpace => &["Mi", "Gi", "Ti"],
        SettingKind::BufferSize => &["Ki", "Mi", "Gi"],
        SettingKind::MaxAge | SettingKind::WriteBack | SettingKind::DirCacheTime => {
            &["s", "m", "h", "d"]
        }
    }
}

fn split_custom_setting(kind: SettingKind, value: &str) -> (String, String) {
    let digits = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    let suffix = value.get(digits.len()..).unwrap_or_default();
    let unit = custom_units(kind)
        .iter()
        .find(|unit| **unit == suffix)
        .copied()
        .unwrap_or(custom_units(kind)[0])
        .to_owned();
    (digits, unit)
}

fn setting_help(kind: SettingKind, locale: Locale) -> &'static str {
    match (kind, locale) {
        (SettingKind::MaxSize, Locale::English) => {
            "Maximum local VFS cache usage. No limit leaves eviction to age and free-space rules."
        }
        (SettingKind::MaxSize, Locale::Chinese) => {
            "限制本地 VFS 缓存占用；不限制时由寿命和剩余空间规则负责清理。"
        }
        (SettingKind::MaxAge, Locale::English) => {
            "How long cached objects may remain before rclone can evict them."
        }
        (SettingKind::MaxAge, Locale::Chinese) => "缓存对象保留多久后允许被 rclone 清理。",
        (SettingKind::MinFreeSpace, Locale::English) => {
            "Local disk space reserved for other applications."
        }
        (SettingKind::MinFreeSpace, Locale::Chinese) => "为其他程序保留的本地磁盘空间。",
        (SettingKind::WriteBack, Locale::English) => {
            "Delay after a file closes before it becomes eligible for upload."
        }
        (SettingKind::WriteBack, Locale::Chinese) => "文件关闭后等待多久才进入可上传状态。",
        (SettingKind::DirCacheTime, Locale::English) => {
            "How long remote directory listings and metadata remain cached."
        }
        (SettingKind::DirCacheTime, Locale::Chinese) => "远端目录列表和元数据的缓存时间。",
        (SettingKind::BufferSize, Locale::English) => "Memory read buffer allocated per open file.",
        (SettingKind::BufferSize, Locale::Chinese) => "每个打开文件使用的内存读取缓冲。",
    }
}

fn settings_help<'a>(help: &'a str) -> Element<'a, Message> {
    tooltip(
        container(text("?").size(13)).padding([0, 5]),
        container(text(help).size(12)).padding(8),
        tooltip::Position::FollowCursor,
    )
    .into()
}

fn vfs_cache_mode_help(locale: Locale) -> &'static str {
    match locale {
        Locale::English => {
            "Full is recommended: it supports read caching and compatible writes. Writes caches modified files only; minimal/off reduce compatibility."
        }
        Locale::Chinese => {
            "推荐 full：支持读取缓存和完整写入兼容。writes 只缓存修改文件，minimal/off 的兼容性更低。"
        }
    }
}

fn auto_transfer_help(locale: Locale) -> &'static str {
    match locale {
        Locale::English => {
            "Shows the shared transfer popup when uploads are queued. Hiding it does not stop uploads."
        }
        Locale::Chinese => "有待上传任务时显示共享传输弹窗。隐藏弹窗不会停止上传。",
    }
}

fn split_remote_path(value: &str) -> (String, String) {
    let path = value.trim();
    if path.starts_with('/') {
        ("/".into(), path.trim_start_matches('/').into())
    } else {
        ("$HOME".into(), path.trim_matches('/').into())
    }
}

fn compose_remote_path(base: &str, suffix: &str) -> String {
    let suffix = suffix.trim().replace('\\', "/");
    let suffix = suffix.trim_matches('/');
    if base == "/" {
        if suffix.is_empty() {
            "/".into()
        } else {
            format!("/{suffix}")
        }
    } else {
        suffix.into()
    }
}

fn is_drive_mountpoint(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn custom_mountpoint_value(value: &str) -> String {
    if value.is_empty()
        || value == HOME_MOUNTPOINT_VALUE
        || value == CUSTOM_MOUNTPOINT_PENDING
        || is_drive_mountpoint(value)
    {
        String::new()
    } else {
        value.into()
    }
}

fn mountpoint_choice(value: &str) -> String {
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        "auto".into()
    } else if value == HOME_MOUNTPOINT_VALUE {
        "home".into()
    } else if is_drive_mountpoint(value) {
        value.to_owned()
    } else {
        "custom".into()
    }
}

fn mountpoint_option_label(value: &str, locale: Locale) -> String {
    match (value, locale) {
        ("auto", Locale::English) => "Auto".into(),
        ("auto", Locale::Chinese) => "自动".into(),
        ("home", Locale::English) => "User folder (~/mnt/name)".into(),
        ("home", Locale::Chinese) => "用户文件夹 (~/mnt/名称)".into(),
        ("custom", Locale::English) => "Custom folder...".into(),
        ("custom", Locale::Chinese) => "自定义文件夹...".into(),
        _ => value.into(),
    }
}

fn mountpoint_option_value(label: &str, locale: Locale) -> String {
    for value in ["auto", "home", "custom"] {
        if label == mountpoint_option_label(value, locale) {
            return value.into();
        }
    }
    label.into()
}

fn mountpoint_options(locale: Locale) -> Vec<String> {
    let mut options = vec![
        mountpoint_option_label("auto", locale),
        mountpoint_option_label("home", locale),
    ];
    if cfg!(windows) {
        options.extend(
            "ZYXWVUTSRQPONMLKJIHGFED"
                .chars()
                .map(|letter| format!("{letter}:"))
                .filter(|drive| !PathBuf::from(format!("{drive}\\")).exists()),
        );
    }
    options.push(mountpoint_option_label("custom", locale));
    options
}

fn suggested_mountpoint(parent: &Path, name: &str) -> PathBuf {
    let cleaned = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    parent.join(if cleaned.is_empty() {
        "mount"
    } else {
        &cleaned
    })
}

fn mountpoint_help(locale: Locale) -> &'static str {
    match locale {
        Locale::English => {
            "Auto chooses a free drive or user folder. Custom folder selection creates a child mountpoint under the selected parent."
        }
        Locale::Chinese => {
            "自动会选择可用盘符或用户目录。选择自定义文件夹时，会在所选父目录下生成子挂载点。"
        }
    }
}

fn file_manager_settings_visible(os: &str) -> bool {
    matches!(os, "windows" | "linux" | "macos")
}

fn settings_folder_input<'a>(
    label: &'a str,
    value: &'a str,
    field: SettingsField,
    browse: Message,
    browse_label: &'a str,
) -> iced::widget::Column<'a, Message> {
    labeled_control(
        label,
        row![
            text_input(label, value)
                .on_input(move |value| Message::SettingsFieldChanged(field, value))
                .width(Fill),
            button(browse_label).on_press(browse),
        ]
        .spacing(8),
    )
}

fn labeled_control<'a>(
    label: &'a str,
    control: impl Into<Element<'a, Message>>,
) -> iced::widget::Column<'a, Message> {
    column![text(label).size(13), control.into()]
        .spacing(5)
        .width(Fill)
}

fn localized_choices<T: Copy>(
    values: impl IntoIterator<Item = T>,
    locale: Locale,
    label: fn(Locale, T) -> &'static str,
) -> Vec<Choice<T>> {
    values
        .into_iter()
        .map(|value| locale.choice(value, label(locale, value)))
        .collect()
}

fn localized_import_reason(
    locale: Locale,
    status: ImportStatus,
    protected: bool,
    original: &str,
) -> String {
    if locale == Locale::English {
        return original.into();
    }
    if protected {
        return "匹配的连接正在挂载或执行任务".into();
    }
    match status {
        ImportStatus::New => String::new(),
        ImportStatus::Same => "相同配置已存在".into(),
        ImportStatus::SameHost => "相同的 SSH Host 已存在".into(),
        ImportStatus::SameTarget => "相同的 HostName、用户和端口已存在".into(),
        ImportStatus::Invalid if original.is_empty() => "无法解析 SSH Host".into(),
        ImportStatus::Invalid => format!("无法解析 SSH Host：{original}"),
    }
}

fn localize_draft_error(locale: Locale, error: &DraftError) -> String {
    if locale == Locale::English {
        return error.to_string();
    }
    match error {
        DraftError::Required(field) => format!("必须填写{}", localized_draft_field(field)),
        DraftError::InvalidScalar(field) => {
            format!("{}不能包含空白字符或控制字符", localized_draft_field(field))
        }
        DraftError::InvalidPort => "端口必须是 1 到 65535 之间的数字".into(),
        DraftError::KeyRequired => "请选择私钥文件".into(),
        DraftError::KeyMissing(path) => format!("找不到私钥文件：{path}"),
        DraftError::PublicKey => "请选择私钥文件，而不是 .pub 公钥文件".into(),
        DraftError::InvalidHostAlias => "SSH Host 无效".into(),
        DraftError::InvalidMountpoint => "自定义挂载点必须是绝对路径或以 ~ 开头".into(),
        DraftError::Duplicate(target) => format!("相同目标的连接已存在：{target}"),
        DraftError::PasswordRequired => "必须填写密码".into(),
        DraftError::SecretNotObscured => "无法安全地加密保存凭据".into(),
        DraftError::InvalidImportPlan => "SSH 导入计划不一致".into(),
        DraftError::InvalidImportAction(host) => {
            format!("SSH Host {host} 不允许使用所选导入操作")
        }
    }
}

fn localized_draft_field(field: &str) -> &str {
    match field {
        "Name" => "名称",
        "IP/Host" => "IP / 主机名",
        "User" => "用户",
        _ => field,
    }
}

fn editor_shell<'a>(
    header: impl Into<Element<'a, Message>>,
    content: impl Into<Element<'a, Message>>,
    status: &'a str,
) -> Element<'a, Message> {
    container(
        column![
            header.into(),
            container(content.into()).height(Fill).width(Fill),
            row![text(status), Space::new().width(Fill), text(VERSION)],
        ]
        .spacing(16),
    )
    .padding(18)
    .width(Fill)
    .height(Fill)
    .into()
}

const LOG_VIEW_LIMIT: u64 = 2 * 1024 * 1024;

fn load_log_task(id: String, path: PathBuf) -> Task<Message> {
    let result_id = id.clone();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || read_mount_log(path))
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
        },
        move |result| Message::LogLoaded {
            id: result_id.clone(),
            result,
        },
    )
}

fn read_mount_log(path: PathBuf) -> Result<LoadedMountLog, String> {
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedMountLog {
                path,
                content: String::new(),
                truncated: false,
            });
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let length = file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?
        .len();
    let start = length.saturating_sub(LOG_VIEW_LIMIT);
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let mut bytes = Vec::with_capacity((length - start).min(LOG_VIEW_LIMIT) as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if start > 0
        && let Some(newline) = bytes.iter().position(|byte| *byte == b'\n')
    {
        bytes.drain(..=newline);
    }
    Ok(LoadedMountLog {
        path,
        content: String::from_utf8_lossy(&bytes).into_owned(),
        truncated: start > 0,
    })
}

fn apply_read_only_log_action(content: &mut text_editor::Content, action: text_editor::Action) {
    if !action.is_edit() {
        content.perform(action);
    }
}

fn validate_setting_value(
    name: &str,
    value: &str,
    required: bool,
    locale: Locale,
) -> Result<(), String> {
    let value = value.trim();
    if required && value.is_empty() {
        return Err(match locale {
            Locale::English => format!("{name} is required"),
            Locale::Chinese => format!("必须填写{name}"),
        });
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(match locale {
            Locale::English => format!("{name} must not contain whitespace"),
            Locale::Chinese => format!("{name}不能包含空白字符"),
        });
    }
    Ok(())
}

fn obscure_action(service: &MountService, action: &SecretAction) -> Result<Option<String>, String> {
    match action {
        SecretAction::Obscure(secret) => service
            .obscure_secret(secret)
            .map(Some)
            .map_err(|error| error.to_string()),
        SecretAction::Clear | SecretAction::Keep(_) => Ok(None),
    }
}

fn display_mountpoint(server: &ServerConfig, locale: Locale) -> &str {
    if server.mountpoint.is_empty() {
        locale.text(TextKey::AutoMountpoint)
    } else {
        &server.mountpoint
    }
}

fn operation_display_name(server: Option<&ServerConfig>, id: &str) -> String {
    server
        .map(|server| server.display_name().to_owned())
        .unwrap_or_else(|| id.to_owned())
}

fn status_label(locale: Locale, status: MountStatus) -> &'static str {
    match (locale, status) {
        (Locale::English, MountStatus::Mounted) => "Mounted",
        (Locale::English, MountStatus::Unmounted) => "Unmounted",
        (Locale::English, MountStatus::Starting) => "Starting",
        (Locale::English, MountStatus::Stale) => "Stale state",
        (Locale::Chinese, MountStatus::Mounted) => "已挂载",
        (Locale::Chinese, MountStatus::Unmounted) => "未挂载",
        (Locale::Chinese, MountStatus::Starting) => "正在处理",
        (Locale::Chinese, MountStatus::Stale) => "状态已失效",
    }
}

fn transfer_label(locale: Locale, snapshot: &TransferSnapshot) -> String {
    if snapshot.out_of_space {
        match locale {
            Locale::English => "Local VFS cache is out of space".into(),
            Locale::Chinese => "本地 VFS 缓存空间不足".into(),
        }
    } else if snapshot.errors > 0 {
        match locale {
            Locale::English => format!("{} upload error(s)", snapshot.errors),
            Locale::Chinese => format!("{} 个上传错误", snapshot.errors),
        }
    } else if snapshot.uploading > 0 {
        if snapshot.files.is_empty() {
            match locale {
                Locale::English => format!(
                    "Uploading {} file(s) - progress unavailable",
                    snapshot.uploading
                ),
                Locale::Chinese => {
                    format!("正在上传 {} 个文件 - 暂无进度", snapshot.uploading)
                }
            }
        } else {
            match locale {
                Locale::English => format!(
                    "Uploading {} file(s) - {:.0}%",
                    snapshot.uploading, snapshot.percentage
                ),
                Locale::Chinese => format!(
                    "正在上传 {} 个文件 - {:.0}%",
                    snapshot.uploading, snapshot.percentage
                ),
            }
        }
    } else if snapshot.queued > 0 {
        match locale {
            Locale::English => format!(
                "{} file(s) queued locally - waiting for write-back delay or upload slot - {}",
                snapshot.queued,
                format_bytes(snapshot.queued_bytes)
            ),
            Locale::Chinese => format!(
                "{} 个文件已进入本地队列，等待写回延迟或上传槽位 - {}",
                snapshot.queued,
                format_bytes(snapshot.queued_bytes)
            ),
        }
    } else if snapshot.synced {
        locale.text(TextKey::CloudSynced).into()
    } else {
        locale.text(TextKey::CheckingCloudState).into()
    }
}

fn transfer_is_active(snapshot: &TransferSnapshot) -> bool {
    snapshot.queued > 0 || snapshot.uploading > 0 || snapshot.errors > 0 || snapshot.out_of_space
}

fn transfer_failure_is_visible(consecutive_failures: u8) -> bool {
    consecutive_failures >= 3
}

fn unmount_needs_confirmation(
    snapshot: Option<&TransferSnapshot>,
    transfer_unavailable: bool,
) -> bool {
    transfer_unavailable || !snapshot.is_some_and(|snapshot| snapshot.synced)
}

fn retain_dismissed_transfers(
    dismissed: &mut HashSet<String>,
    active: &HashSet<String>,
    awaiting_confirmation: &HashSet<String>,
) {
    dismissed.retain(|id| active.contains(id) || awaiting_confirmation.contains(id));
}

fn popup_transfer_summary(
    locale: Locale,
    totals: &TransferTotals,
    connection_count: usize,
) -> String {
    if connection_count == 0 {
        return match locale {
            Locale::English => "Waiting for remote confirmation".into(),
            Locale::Chinese => "等待远端确认".into(),
        };
    }
    if totals.out_of_space {
        return match locale {
            Locale::English => "A local VFS cache is out of space".into(),
            Locale::Chinese => "本地 VFS 缓存空间不足".into(),
        };
    }
    if totals.errors > 0 {
        return match locale {
            Locale::English => format!(
                "{} upload error(s) across {connection_count} connection(s)",
                totals.errors
            ),
            Locale::Chinese => {
                format!(
                    "{} 个上传错误，涉及 {connection_count} 个挂载",
                    totals.errors
                )
            }
        };
    }
    if totals.progress_available && totals.total_bytes > 0 {
        match locale {
            Locale::English => format!(
                "{} file(s) pending - {} of {} uploaded",
                totals.pending_files,
                format_bytes(totals.transferred_bytes),
                format_bytes(totals.total_bytes)
            ),
            Locale::Chinese => format!(
                "{} 个文件待传 - 已上传 {} / {}",
                totals.pending_files,
                format_bytes(totals.transferred_bytes),
                format_bytes(totals.total_bytes)
            ),
        }
    } else {
        match locale {
            Locale::English => format!(
                "{} file(s) pending across {connection_count} connection(s)",
                totals.pending_files
            ),
            Locale::Chinese => {
                format!(
                    "{} 个文件待传，涉及 {connection_count} 个挂载",
                    totals.pending_files
                )
            }
        }
    }
}

fn native_integration_smoke_enabled() -> bool {
    std::env::var_os("SSH_MOUNTMATE_E2E_NATIVE_SMOKE").is_some()
}

fn native_integration_smoke_notification() -> NativeNotification {
    NativeNotification {
        id: "native-integration-smoke".into(),
        title: "SSH MountMate native integration".into(),
        body: "Native notification delivery is active.".into(),
        progress: None,
        level: NativeNotificationLevel::Info,
    }
}

fn transfer_complete_notification(
    locale: Locale,
    server_id: &str,
    display_name: &str,
) -> NativeNotification {
    NativeNotification {
        id: format!("transfer-complete-{server_id}"),
        title: match locale {
            Locale::English => "Upload complete".into(),
            Locale::Chinese => "上传完成".into(),
        },
        body: match locale {
            Locale::English => {
                format!("{display_name} is synchronized with the remote server.")
            }
            Locale::Chinese => format!("{display_name} 已与远端服务器同步。"),
        },
        progress: None,
        level: NativeNotificationLevel::Info,
    }
}

fn background_transfer_notification(locale: Locale) -> NativeNotification {
    NativeNotification {
        id: "transfer-view-hidden".into(),
        title: match locale {
            Locale::English => "Transfers continue in the background".into(),
            Locale::Chinese => "传输仍在后台继续".into(),
        },
        body: match locale {
            Locale::English => {
                "Keep the mount active until the transfer center reports cloud synchronization."
                    .into()
            }
            Locale::Chinese => "请保持挂载，直到传输中心确认云端已同步。".into(),
        },
        progress: None,
        level: NativeNotificationLevel::Info,
    }
}

fn transfer_error_notification(
    locale: Locale,
    server_id: &str,
    display_name: &str,
    snapshot: &TransferSnapshot,
) -> NativeNotification {
    let body = if snapshot.out_of_space {
        match locale {
            Locale::English => format!("The local VFS cache for {display_name} is out of space."),
            Locale::Chinese => format!("{display_name} 的本地 VFS 缓存空间不足。"),
        }
    } else {
        match locale {
            Locale::English => format!(
                "{display_name} reports {} upload error(s). Open the transfer center for details.",
                snapshot.errors
            ),
            Locale::Chinese => format!(
                "{display_name} 报告 {} 个上传错误，请打开传输中心查看详情。",
                snapshot.errors
            ),
        }
    };
    NativeNotification {
        id: format!("transfer-error-{server_id}"),
        title: match locale {
            Locale::English => "Upload needs attention".into(),
            Locale::Chinese => "上传需要处理".into(),
        },
        body,
        progress: None,
        level: NativeNotificationLevel::Error,
    }
}

fn global_progress_state(totals: &TransferTotals, out_of_space: bool) -> GlobalProgressState {
    if totals.errors > 0 || out_of_space {
        return GlobalProgressState::Error {
            completed: totals.transferred_bytes,
            total: totals.total_bytes.max(1),
        };
    }
    if totals.pending_files == 0 {
        return if totals.unknown_connections > 0 {
            GlobalProgressState::Indeterminate
        } else {
            GlobalProgressState::Hidden
        };
    }
    if !totals.progress_available {
        return GlobalProgressState::Indeterminate;
    }
    GlobalProgressState::Normal {
        completed: totals.transferred_bytes,
        total: totals.total_bytes.max(1),
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn application_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|path| mountmate_core::rclone_binary::application_root(&path))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn diagnostic_trace(message: &str) {
    use std::io::Write;

    let Some(path) = std::env::var_os("SSH_MOUNTMATE_TRACE_FILE") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{message}");
    }
}

fn main_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(980.0, 720.0),
        position: window::Position::Centered,
        exit_on_close_request: false,
        ..window::Settings::default()
    }
}

fn log_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(900.0, 620.0),
        position: window::Position::Centered,
        exit_on_close_request: false,
        ..window::Settings::default()
    }
}

fn transfer_window_settings() -> window::Settings {
    let settings = window::Settings {
        size: transfer_popup_size(false),
        position: window::Position::SpecificWith(bottom_right_position),
        visible: !cfg!(windows),
        resizable: false,
        minimizable: false,
        decorations: true,
        level: window::Level::AlwaysOnTop,
        ..window::Settings::default()
    };
    #[cfg(windows)]
    let settings = {
        let mut settings = settings;
        settings.platform_specific.skip_taskbar = true;
        settings
    };
    settings
}

fn transfer_popup_size(expanded: bool) -> Size {
    if expanded {
        Size::new(560.0, 460.0)
    } else {
        Size::new(420.0, 180.0)
    }
}

fn bottom_right_position(window: Size, monitor: Size) -> Point {
    Point::new(
        (monitor.width - window.width - 20.0).max(0.0),
        (monitor.height - window.height - 64.0).max(0.0),
    )
}

fn configure_popup_window(id: window::Id, size: Size) -> Task<Message> {
    window::monitor_size(id).then(move |monitor| {
        let monitor = monitor.unwrap_or(Size::new(1920.0, 1080.0));
        let position = bottom_right_position(size, monitor);
        #[cfg(windows)]
        {
            window::run(id, move |window| {
                configure_native_popup(window, position);
            })
            .discard()
        }
        #[cfg(not(windows))]
        {
            window::move_to(id, position)
        }
    })
}

#[cfg(windows)]
fn set_native_global_progress(id: window::Id, state: GlobalProgressState) -> Task<Message> {
    use window::raw_window_handle::RawWindowHandle;

    window::run(id, move |window| {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        match Platform.set_global_progress(Some(NativeWindowHandle(handle.hwnd.get())), state) {
            Ok(()) => diagnostic_trace(&format!("taskbar progress updated: {state:?}")),
            Err(error) => diagnostic_trace(&format!("taskbar progress failed: {error}")),
        }
    })
    .discard()
}

fn show_native_notification(notification: NativeNotification) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                Platform
                    .show_notification(&notification)
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())?
        },
        Message::NotificationFinished,
    )
}

#[cfg(target_os = "macos")]
fn set_native_global_progress(_id: window::Id, state: GlobalProgressState) -> Task<Message> {
    match Platform.set_global_progress(None, state) {
        Ok(()) => diagnostic_trace(&format!("dock progress updated: {state:?}")),
        Err(error) => diagnostic_trace(&format!("dock progress failed: {error}")),
    }
    Task::none()
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn set_native_global_progress(_id: window::Id, _state: GlobalProgressState) -> Task<Message> {
    Task::none()
}

#[cfg(windows)]
const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
#[cfg(windows)]
const WS_EX_NOACTIVATE: isize = 0x0800_0000;

#[cfg(windows)]
fn movable_popup_extended_style(style: isize) -> isize {
    (style & !WS_EX_NOACTIVATE) | WS_EX_TOOLWINDOW
}

#[cfg(windows)]
fn configure_native_popup(window: &dyn window::Window, position: Point) {
    use window::raw_window_handle::RawWindowHandle;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowLongPtrW(window: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(window: isize, index: i32, value: isize) -> isize;
        fn SetWindowPos(
            window: isize,
            insert_after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
        fn ShowWindow(window: isize, command: i32) -> i32;
    }

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let window = handle.hwnd.get();
    const GWL_EXSTYLE: i32 = -20;
    const HWND_TOPMOST: isize = -1;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const SW_SHOWNOACTIVATE: i32 = 4;

    // The handle belongs to Iced for this callback; these calls only adjust window styles.
    unsafe {
        let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
        SetWindowLongPtrW(window, GWL_EXSTYLE, movable_popup_extended_style(style));
        SetWindowPos(
            window,
            HWND_TOPMOST,
            position.x.round() as i32,
            position.y.round() as i32,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        );
        ShowWindow(window, SW_SHOWNOACTIVATE);
    }
}

fn open_path(path: &Path, locale: Locale) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("explorer.exe");
        command.arg(path);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(path);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    };
    command.spawn().map(|_| ()).map_err(|error| match locale {
        Locale::English => format!("Could not open {}: {error}", path.display()),
        Locale::Chinese => format!("无法打开 {}：{error}", path.display()),
    })
}

#[cfg(test)]
mod localization_tests {
    use super::*;

    #[test]
    fn draft_errors_are_localized_structurally() {
        assert_eq!(
            localize_draft_error(Locale::Chinese, &DraftError::Required("Name")),
            "必须填写名称"
        );
        assert_eq!(
            localize_draft_error(Locale::Chinese, &DraftError::InvalidPort),
            "端口必须是 1 到 65535 之间的数字"
        );
        assert_eq!(
            localize_draft_error(
                Locale::Chinese,
                &DraftError::InvalidImportAction("cluster".into())
            ),
            "SSH Host cluster 不允许使用所选导入操作"
        );
    }

    #[test]
    fn import_reasons_distinguish_duplicates_and_protected_connections() {
        assert_eq!(
            localized_import_reason(Locale::Chinese, ImportStatus::SameTarget, false, "ignored"),
            "相同的 HostName、用户和端口已存在"
        );
        assert_eq!(
            localized_import_reason(Locale::Chinese, ImportStatus::Same, true, "ignored"),
            "匹配的连接正在挂载或执行任务"
        );
    }

    #[test]
    fn settings_help_and_custom_units_cover_all_compact_fields() {
        for kind in [
            SettingKind::MaxSize,
            SettingKind::MaxAge,
            SettingKind::MinFreeSpace,
            SettingKind::WriteBack,
            SettingKind::DirCacheTime,
            SettingKind::BufferSize,
        ] {
            assert!(!setting_help(kind, Locale::English).is_empty());
            assert!(!setting_help(kind, Locale::Chinese).is_empty());
            assert!(!custom_units(kind).is_empty());
        }
        assert!(custom_units(SettingKind::MaxSize).contains(&"Gi"));
        assert!(custom_units(SettingKind::MaxAge).contains(&"m"));
    }

    #[test]
    fn platform_settings_are_hidden_outside_supported_desktop_targets() {
        assert!(file_manager_settings_visible("windows"));
        assert!(file_manager_settings_visible("linux"));
        assert!(file_manager_settings_visible("macos"));
        assert!(!file_manager_settings_visible("freebsd"));
        assert!(!file_manager_settings_visible("android"));
    }

    #[test]
    fn mount_status_messages_use_the_display_name_not_the_internal_id() {
        let server = ServerConfig {
            id: "NAS".into(),
            name: "jzj".into(),
            ..ServerConfig::default()
        };
        assert_eq!(operation_display_name(Some(&server), &server.id), "jzj");
        assert_eq!(operation_display_name(None, "missing-id"), "missing-id");
        assert_eq!(
            Locale::Chinese.mounting(server.display_name()),
            "正在挂载 jzj..."
        );
    }

    #[test]
    fn log_view_is_bounded_copyable_text_and_handles_missing_files() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.log");
        let empty = read_mount_log(missing.clone()).unwrap();
        assert_eq!(empty.path, missing);
        assert!(empty.content.is_empty());
        assert!(!empty.truncated);

        let path = temp.path().join("large.log");
        let mut bytes = vec![b'x'; LOG_VIEW_LIMIT as usize + 32];
        bytes.extend_from_slice(b"\nfinal log line\n");
        std::fs::write(&path, bytes).unwrap();
        let loaded = read_mount_log(path.clone()).unwrap();
        assert_eq!(loaded.path, path);
        assert!(loaded.truncated);
        assert_eq!(loaded.content, "final log line\n");
    }

    #[test]
    fn log_editor_allows_navigation_but_rejects_edits() {
        let mut content = text_editor::Content::with_text("first\nsecond");
        apply_read_only_log_action(
            &mut content,
            text_editor::Action::Edit(text_editor::Edit::Insert('x')),
        );
        assert_eq!(content.text(), "first\nsecond");

        apply_read_only_log_action(&mut content, text_editor::Action::SelectAll);
        assert_eq!(content.selection().as_deref(), Some("first\nsecond"));
    }

    #[test]
    fn shared_transfer_popup_expands_and_summarizes_all_connections() {
        let compact = transfer_popup_size(false);
        let expanded = transfer_popup_size(true);
        assert!(expanded.width > compact.width);
        assert!(expanded.height > compact.height);
        let settings = transfer_window_settings();
        assert!(settings.decorations);
        assert!(!settings.resizable);

        let totals = TransferTotals {
            pending_files: 3,
            total_bytes: 400,
            transferred_bytes: 100,
            progress_available: true,
            percentage: 25.0,
            ..TransferTotals::default()
        };
        assert_eq!(
            popup_transfer_summary(Locale::English, &totals, 2),
            "3 file(s) pending - 100 B of 400 B uploaded"
        );
        assert_eq!(
            popup_transfer_summary(Locale::Chinese, &totals, 2),
            "3 个文件待传 - 已上传 100 B / 400 B"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_popup_style_is_activatable_and_keeps_tool_window_identity() {
        let style = movable_popup_extended_style(WS_EX_NOACTIVATE);
        assert_eq!(style & WS_EX_NOACTIVATE, 0);
        assert_eq!(style & WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW);
    }

    #[test]
    fn dismissed_popup_survives_a_transient_empty_queue_until_confirmation() {
        let mut dismissed = HashSet::from(["alpha".to_owned()]);
        retain_dismissed_transfers(
            &mut dismissed,
            &HashSet::new(),
            &HashSet::from(["alpha".to_owned()]),
        );
        assert!(dismissed.contains("alpha"));

        retain_dismissed_transfers(&mut dismissed, &HashSet::new(), &HashSet::new());
        assert!(dismissed.is_empty());
    }

    #[test]
    fn capacity_progress_is_persistent_for_known_checking_and_unknown_states() {
        let capacity = CapacityInfo {
            used: 256 * 1024,
            total: 1024 * 1024,
            percent: 25,
            source: mountmate_core::capacity::CapacitySource::RemoteFilesystem,
        };
        assert_eq!(
            capacity_progress_state(Some(&capacity), false, Locale::English),
            (25.0, "Capacity: 256.0 KB / 1.0 MB used (25%)".into())
        );
        assert_eq!(
            capacity_progress_state(None, true, Locale::Chinese),
            (0.0, "容量：查询中...".into())
        );
        assert_eq!(
            capacity_progress_state(None, false, Locale::Chinese),
            (0.0, "容量：未知".into())
        );
    }

    #[test]
    fn taskbar_progress_uses_truthful_transfer_totals() {
        let normal = TransferTotals {
            pending_files: 2,
            total_bytes: 400,
            transferred_bytes: 100,
            progress_available: true,
            ..TransferTotals::default()
        };
        assert_eq!(
            global_progress_state(&normal, false),
            GlobalProgressState::Normal {
                completed: 100,
                total: 400,
            }
        );

        let unknown = TransferTotals {
            pending_files: 1,
            progress_available: false,
            ..TransferTotals::default()
        };
        assert_eq!(
            global_progress_state(&unknown, false),
            GlobalProgressState::Indeterminate
        );

        let error = TransferTotals {
            errors: 1,
            ..TransferTotals::default()
        };
        assert_eq!(
            global_progress_state(&error, false),
            GlobalProgressState::Error {
                completed: 0,
                total: 1,
            }
        );
        assert_eq!(
            global_progress_state(&TransferTotals::default(), false),
            GlobalProgressState::Hidden
        );
    }

    #[test]
    fn exhausted_cache_is_visible_as_active_transfer_work() {
        let snapshot = TransferSnapshot {
            files: Vec::new(),
            queued: 0,
            uploading: 0,
            queued_bytes: 0,
            transferred_bytes: 0,
            percentage: 0.0,
            errors: 0,
            out_of_space: true,
            synced: false,
        };

        assert!(transfer_is_active(&snapshot));
    }

    #[test]
    fn transfer_error_requires_three_consecutive_failures() {
        assert!(!transfer_failure_is_visible(1));
        assert!(!transfer_failure_is_visible(2));
        assert!(transfer_failure_is_visible(3));
        assert!(transfer_failure_is_visible(u8::MAX));
    }

    #[test]
    fn compact_setting_presets_preserve_custom_values() {
        assert_eq!(
            setting_presets(SettingKind::MaxSize),
            &["", "10G", "50G", "100G"]
        );
        assert_eq!(
            setting_presets(SettingKind::WriteBack),
            &["0s", "5s", "30s", "1m"]
        );
        assert_eq!(
            split_custom_setting(SettingKind::MaxSize, "250Gi"),
            ("250".into(), "Gi".into())
        );
        assert_eq!(
            split_custom_setting(SettingKind::MaxAge, "1h30m"),
            ("1".into(), "s".into())
        );
    }

    #[test]
    fn structured_remote_path_round_trips_home_and_root() {
        assert_eq!(
            split_remote_path("project/data"),
            ("$HOME".into(), "project/data".into())
        );
        assert_eq!(compose_remote_path("$HOME", "project/data"), "project/data");
        assert_eq!(
            split_remote_path("/srv/data"),
            ("/".into(), "srv/data".into())
        );
        assert_eq!(compose_remote_path("/", "srv/data"), "/srv/data");
        assert_eq!(compose_remote_path("/", ""), "/");
    }

    #[test]
    fn mountpoint_presets_do_not_misclassify_paths_containing_drive_text() {
        assert_eq!(mountpoint_choice(""), "auto");
        assert_eq!(mountpoint_choice(HOME_MOUNTPOINT_VALUE), "home");
        assert_eq!(mountpoint_choice("Z:"), "Z:");
        assert_eq!(mountpoint_choice(CUSTOM_MOUNTPOINT_PENDING), "custom");
        assert_eq!(mountpoint_choice("/data/Z:/folder"), "custom");
    }

    #[test]
    fn unmount_is_immediate_only_after_confirmed_sync() {
        let synced = TransferSnapshot {
            files: Vec::new(),
            queued: 0,
            uploading: 0,
            queued_bytes: 0,
            transferred_bytes: 0,
            percentage: 100.0,
            errors: 0,
            out_of_space: false,
            synced: true,
        };
        assert!(!unmount_needs_confirmation(Some(&synced), false));
        assert!(unmount_needs_confirmation(Some(&synced), true));
        assert!(unmount_needs_confirmation(None, false));
        let mut pending = synced.clone();
        pending.synced = false;
        pending.queued = 1;
        assert!(unmount_needs_confirmation(Some(&pending), false));
    }

    #[test]
    fn native_notifications_require_real_state_transitions() {
        let mut tracker = TransferNotificationTracker::default();
        let active = TransferSnapshot {
            files: Vec::new(),
            queued: 1,
            uploading: 1,
            queued_bytes: 100,
            transferred_bytes: 40,
            percentage: 40.0,
            errors: 0,
            out_of_space: false,
            synced: false,
        };
        assert!(
            tracker
                .observe("alpha", "Alpha", &active, 0, Locale::English)
                .is_empty()
        );

        let synced = TransferSnapshot {
            files: Vec::new(),
            queued: 0,
            uploading: 0,
            queued_bytes: 0,
            transferred_bytes: 0,
            percentage: 100.0,
            errors: 0,
            out_of_space: false,
            synced: true,
        };
        assert!(
            tracker
                .observe("alpha", "Alpha", &synced, 1, Locale::English)
                .is_empty()
        );
        let completed = tracker.observe("alpha", "Alpha", &synced, 2, Locale::English);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].level, NativeNotificationLevel::Info);
        assert!(
            tracker
                .observe("alpha", "Alpha", &synced, 3, Locale::English)
                .is_empty()
        );

        let mut failed = active.clone();
        failed.errors = 1;
        let first_error = tracker.observe("alpha", "Alpha", &failed, 0, Locale::English);
        assert_eq!(first_error.len(), 1);
        assert_eq!(first_error[0].level, NativeNotificationLevel::Error);
        assert!(
            tracker
                .observe("alpha", "Alpha", &failed, 0, Locale::English)
                .is_empty()
        );
        tracker.observe("alpha", "Alpha", &active, 0, Locale::English);
        assert_eq!(
            tracker
                .observe("alpha", "Alpha", &failed, 0, Locale::English)
                .len(),
            1
        );
    }
}
