#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::widget::{
    Space, button, checkbox, column, container, keyed_column, pick_list, progress_bar, responsive,
    row, scrollable, stack, text, text_editor, text_input, toggler, tooltip,
};
use iced::{
    Center, Color, Element, Fill, Length, Point, Size, Subscription, Task, Theme, clipboard,
    theme::Palette, window,
};
use mountmate_core::app_command::{
    AppCommand, AppCommandError, AppCommandServer, InstanceLock, running_instance,
    same_instance_build, send_command_retry,
};
use mountmate_core::capacity::CapacityInfo;
use mountmate_core::connection::{
    ConnectionDraft, ConnectionSource, DraftError, ImportAction, ImportStatus,
    PreservedSecretState, SecretAction, SshImportPlan,
};
use mountmate_core::credential::{
    CredentialChange, CredentialError, CredentialKind, CredentialMigration, CredentialStore,
    SystemCredentialStore, credential_reference, delete_credential_references,
    delete_server_credentials, prepare_server_to_obscure, prepare_server_to_system,
    replace_verified, rollback_change,
};
use mountmate_core::dependency::{DependencyStatus, check_dependencies};
use mountmate_core::interactive_ssh::{
    InteractiveSshError, InteractiveSshLoginCommand, InteractiveSshSession,
};
use mountmate_core::model::{
    MAX_CONNECTION_TAGS, MAX_TAG_CHARS, MAX_VFS_UPLOAD_TRANSFERS, MIN_VFS_UPLOAD_TRANSFERS,
};
use mountmate_core::mountpoint::{HOME_MOUNTPOINT_VALUE, preflight_custom_mountpoint};
use mountmate_core::paths::AppPaths;
use mountmate_core::plink_binary::resolve_plink;
use mountmate_core::process::MountStatus;
use mountmate_core::rclone::clear_rclone_remote_secrets;
use mountmate_core::rclone_binary::resolve_rclone;
use mountmate_core::service::{MountService, ServiceError};
use mountmate_core::ssh::{
    default_ssh_config_path, prepare_managed_ssh_server, remove_managed_ssh_server,
};
use mountmate_core::storage::{self, read_json};
use mountmate_core::transfer::TransferSnapshot;
use mountmate_core::update::{UpdateInfo, check_for_updates};
use mountmate_core::update_helper::{
    UpdateHealthAuthorization, run_update_helper, write_update_health_marker,
};
use mountmate_core::update_manifest::UpdateTrustError;
use mountmate_core::update_workflow::{PreparedUpdateLaunch, prepare_update_install};
use mountmate_core::{
    APP_NAME, AccentColor, AppearanceMode, AuthMethod, ConnectionMethod, CredentialStorage,
    FontScale, MountBackend, MountState, ServerConfig, Settings, VERSION,
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
use tray::{TrayAction, TrayController, TrayError};

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
                        .map_or("unavailable", |asset| asset.name())
                );
                if let Some(error) = &info.trust_error {
                    println!("Automatic installation blocked: {error}");
                }
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
        LaunchAction::PlinkPath => {
            let paths = AppPaths::discover();
            let resolved = resolve_plink(&paths, &application_root())
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "verified Plink is not available".to_owned())?;
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
        LaunchAction::RunSshConnector { program, arguments } => {
            let mut command = Command::new(program);
            command
                .args(arguments)
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            let status = command.status().map_err(|error| error.to_string())?;
            std::process::exit(status.code().unwrap_or(1));
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
                .scale_factor(|app, _window| app.effective_font_scale().factor())
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
        AppCommand::MountStartup => {
            let selected = startup_servers(&settings, &servers);
            run_headless_batch(&service, &selected, |service, server| {
                service
                    .mount(server, &settings)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })?
        }
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
                (
                    server.display_name().to_owned(),
                    scope.spawn(move || {
                        operation(service, server)
                            .err()
                            .map(|error| format!("{}: {error}", server.display_name()))
                    }),
                )
            })
            .collect();
        tasks
            .into_iter()
            .filter_map(|(name, task)| match task.join() {
                Ok(failure) => failure,
                Err(_) => Some(format!("{name}: batch worker panicked")),
            })
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
    startup_integration_lock: Arc<Mutex<()>>,
    startup_notice: Option<String>,
    system_locale: Locale,
    system_theme_dark: bool,
    servers: Vec<ServerConfig>,
    connection_search: String,
    connection_sort: ConnectionSort,
    connection_tag_filter: Option<String>,
    connection_list_mode: ConnectionListMode,
    selected_connections: HashSet<String>,
    batch_tag_input: String,
    batch_existing_tag: Option<String>,
    reorder_original: Option<Vec<String>>,
    connection_list_saving: bool,
    service: MountService,
    mount_statuses: HashMap<String, MountStatus>,
    busy: HashSet<String>,
    transfers: HashMap<String, TransferSnapshot>,
    transfer_errors: HashMap<String, String>,
    operation_errors: HashMap<String, ConnectionOperationError>,
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
    tray_retry_at: Option<Instant>,
    tray_retry_delay: Duration,
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
    connection_tags_input: String,
    connection_custom_mountpoint: String,
    mountpoint_preflight: MountpointPreflight,
    mountpoint_preflight_generation: u64,
    settings_draft: Option<SettingsDraft>,
    log_view: Option<MountLogView>,
    log_window: Option<window::Id>,
    terminal_window: Option<window::Id>,
    terminal_server_id: Option<String>,
    interactive_terminals: HashMap<String, InteractiveTerminalSession>,
    next_terminal_generation: u64,
    terminal_error: Option<(String, String)>,
    custom_setting: Option<CustomSettingDraft>,
    editor_saving: bool,
    ssh_import_loading: bool,
    ssh_import_plan: Option<SshImportPlan>,
    ssh_import_actions: Vec<ImportAction>,
    pending_delete: Option<String>,
    settings_recovery_dialog: Option<String>,
    status: String,
    status_generation: u64,
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
    existed: bool,
}

#[derive(Debug, Clone)]
struct MountLogView {
    server_id: String,
    display_name: String,
    path: PathBuf,
    content: text_editor::Content,
    truncated: bool,
    loading: bool,
    existed: bool,
}

#[derive(Debug, Clone)]
struct ConnectionOperationError {
    operation: MountOperation,
    cause: String,
}

/// iced_term events may contain terminal input bytes (including passwords).
/// Keep the event usable by the update loop, but make all application Debug
/// output intentionally opaque.
#[derive(Clone)]
struct RedactedTerminalEvent(iced_term::Event);

impl fmt::Debug for RedactedTerminalEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TerminalEvent(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InteractiveTerminalLifecycle {
    Starting,
    Ready,
    Exited,
    Failed,
}

struct InteractiveTerminalSession {
    generation: u64,
    ssh: InteractiveSshSession,
    terminal: iced_term::Terminal,
    lifecycle: InteractiveTerminalLifecycle,
    queued_mount: bool,
    resume_sent: bool,
    readiness_check_in_flight: bool,
}

/// Keep a failed mount from turning a connection card into a full-page error
/// while retaining enough context to identify the immediate cause. The
/// dedicated read-only log window remains the source for complete details.
const MOUNT_ERROR_SUMMARY_MAX_LINES: usize = 2;
const MOUNT_ERROR_SUMMARY_MAX_CHARS: usize = 240;
const MOUNT_ERROR_SUMMARY_LINE_MAX_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MountErrorAction {
    Retry,
    ViewLog,
    Dismiss,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum MountpointPreflight {
    #[default]
    NotRequired,
    Checking(String),
    Valid(String),
    Invalid {
        value: String,
        error: String,
    },
}

impl MountpointPreflight {
    fn allows_save(&self) -> bool {
        matches!(self, Self::NotRequired | Self::Valid(_))
    }
}

fn mountpoint_preflight_result_is_current(
    result_generation: u64,
    current_generation: u64,
    result_value: &str,
    current_value: Option<&str>,
) -> bool {
    result_generation == current_generation && current_value == Some(result_value)
}

fn mount_error_summary(locale: Locale, cause: &str) -> String {
    let prefix = match locale {
        Locale::English => "Last operation failed: ",
        Locale::Chinese => "上次操作失败：",
    };
    let mut lines = Vec::new();
    let mut truncated = false;
    for raw_line in cause.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if lines.len() == MOUNT_ERROR_SUMMARY_MAX_LINES {
            truncated = true;
            break;
        }
        let clipped = line
            .chars()
            .take(MOUNT_ERROR_SUMMARY_LINE_MAX_CHARS)
            .collect::<String>();
        if clipped.chars().count() < line.chars().count() {
            truncated = true;
        }
        lines.push(clipped);
    }
    if lines.is_empty() {
        lines.push(match locale {
            Locale::English => "Unknown mount error".into(),
            Locale::Chinese => "未知挂载错误".into(),
        });
    }

    let mut summary = format!("{prefix}{}", lines.join("\n"));
    if summary.chars().count() > MOUNT_ERROR_SUMMARY_MAX_CHARS {
        summary = summary
            .chars()
            .take(MOUNT_ERROR_SUMMARY_MAX_CHARS.saturating_sub(1))
            .collect();
        truncated = true;
    }
    if truncated {
        if summary.chars().count() >= MOUNT_ERROR_SUMMARY_MAX_CHARS {
            summary = summary
                .chars()
                .take(MOUNT_ERROR_SUMMARY_MAX_CHARS.saturating_sub(1))
                .collect();
        }
        summary.push('…');
    }
    summary
}

fn mount_error_message(id: String, operation: MountOperation, action: MountErrorAction) -> Message {
    match action {
        MountErrorAction::Retry => Message::RetryOperation(id, operation),
        MountErrorAction::ViewLog => Message::OpenOperationLog(id),
        MountErrorAction::Dismiss => Message::DismissOperationError(id),
    }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ConnectionListMode {
    #[default]
    Browse,
    Batch,
    Tags,
    Reorder,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ConnectionSort {
    #[default]
    SavedOrder,
    Name,
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionSortMenuAction {
    Sort(ConnectionSort),
    AdjustSavedOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusPublishPolicy {
    Initial,
    UserRefresh,
    Silent,
}

impl ConnectionSort {
    const ALL: [Self; 3] = [Self::SavedOrder, Self::Name, Self::Host];
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
    Transfers,
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
    raw_value: Option<String>,
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
    upload_transfers: String,
    mount_backend: MountBackend,
    credential_storage: CredentialStorage,
    startup_all: bool,
    connection_preferences: Vec<ConnectionPreferenceDraft>,
    connection_preferences_expanded: bool,
    auto_show_transfers: bool,
    auto_check_updates: bool,
    language: Language,
    appearance_mode: AppearanceMode,
    accent_color: AccentColor,
    font_scale: FontScale,
}

#[derive(Debug, Clone)]
struct ConnectionPreferenceDraft {
    id: String,
    name: String,
    tags: Vec<String>,
    auto_mount_at_login: bool,
    startup_available: bool,
}

impl ConnectionPreferenceDraft {
    fn from_server(server: &ServerConfig, legacy_startup_all: bool) -> Self {
        Self {
            id: server.id.clone(),
            name: server.display_name().to_owned(),
            tags: server.tags.clone(),
            auto_mount_at_login: legacy_startup_all || server.auto_mount_at_login,
            startup_available: server.connection_method != ConnectionMethod::Interactive,
        }
    }
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
            upload_transfers: settings.vfs_upload_transfers.to_string(),
            mount_backend: settings.macos_mount_backend,
            credential_storage: settings.credential_storage,
            startup_all: settings.startup_all,
            connection_preferences: Vec::new(),
            connection_preferences_expanded: false,
            auto_show_transfers: settings.auto_show_transfers,
            auto_check_updates: settings.auto_check_updates,
            language: Language::from_value(&settings.language),
            appearance_mode: settings.appearance_mode,
            accent_color: settings.accent_color,
            font_scale: settings.font_scale,
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
        let upload_transfers = validate_upload_transfers(&self.upload_transfers, locale)?;
        let mut settings = original.clone();
        settings.cache_root = PathBuf::from(self.cache_root.trim());
        settings.vfs_cache_mode = self.cache_mode.value().into();
        settings.vfs_cache_max_size = self.max_size.trim().into();
        settings.vfs_cache_max_age = self.max_age.trim().into();
        settings.vfs_cache_min_free_space = self.min_free_space.trim().into();
        settings.vfs_write_back = self.write_back.trim().into();
        settings.dir_cache_time = self.dir_cache_time.trim().into();
        settings.buffer_size = self.buffer_size.trim().into();
        settings.vfs_upload_transfers = upload_transfers;
        settings.macos_mount_backend = self.mount_backend;
        settings.credential_storage = self.credential_storage;
        settings.startup_all = self.startup_all;
        settings.auto_show_transfers = self.auto_show_transfers;
        settings.auto_check_updates = self.auto_check_updates;
        settings.language = self.language.value().into();
        settings.appearance_mode = self.appearance_mode;
        settings.accent_color = self.accent_color;
        settings.font_scale = self.font_scale;
        Ok(settings)
    }
}

#[derive(Debug, Clone)]
enum Message {
    AppCommand(AppCommand),
    TrayAction(TrayAction),
    TrayTick,
    InteractiveTick,
    InteractiveReadinessChecked {
        id: String,
        generation: u64,
        result: Result<bool, String>,
    },
    TerminalEvent(RedactedTerminalEvent),
    TerminalWindowOpened(window::Id),
    OpenInteractiveTerminal(String),
    HideTerminal,
    EndInteractiveSession,
    RetryTerminal,
    MainWindowOpened(window::Id),
    SettingsRecoveryDialogClosed,
    Refresh,
    RefreshFinished {
        generation: u64,
        expected_status: String,
        result: Result<mountmate_core::rc::RefreshResult, String>,
    },
    StatusesLoaded {
        policy: StatusPublishPolicy,
        generation: Option<u64>,
        expected_status: Option<String>,
        results: Vec<(String, Result<MountStatus, String>)>,
    },
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
    ConnectionSearchChanged(String),
    ConnectionSortMenuChanged(ConnectionSortMenuAction),
    ConnectionTagFilterChanged(Option<String>),
    ConnectionListModeChanged(ConnectionListMode),
    BatchSelectionChanged(String, bool),
    BatchSelectAllChanged(bool),
    BatchTagSelectionChanged(String, bool),
    BatchTagInputChanged(String),
    BatchExistingTagChanged(String),
    BatchAddTag,
    RemoveConnectionTag(String, String),
    RemoveTagEverywhere(String),
    RemoveTagEverywhereDecision {
        tag: String,
        result: rfd::MessageDialogResult,
    },
    BatchTagsSaved(Result<Vec<ServerConfig>, String>),
    BatchMountSelected,
    BatchUnmountSelected,
    BatchStartupChanged(String, bool),
    BatchStartupSaved(Result<ServerMutation, String>),
    BatchDeleteSelected,
    BatchDeleteDecision {
        ids: Vec<String>,
        result: rfd::MessageDialogResult,
    },
    BatchRemoved {
        ids: Vec<String>,
        result: Result<ServerMutation, String>,
    },
    MoveConnection(String, i8),
    SaveConnectionOrder,
    CancelConnectionOrder,
    ConnectionsReordered(Result<Vec<ServerConfig>, String>),
    SettingsConnectionStartupChanged(String, bool),
    ToggleSettingsConnectionPreferences,
    OpenBatchManagement,
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
    ConnectionTagsChanged(String),
    RemoteBaseChanged(String),
    RemoteSuffixChanged(String),
    MountpointChoiceChanged(String),
    CustomMountpointChanged(String),
    BrowseMountpoint,
    MountpointPicked(Option<PathBuf>),
    MountpointPreflightFinished {
        generation: u64,
        value: String,
        result: Result<(), String>,
    },
    ConnectionAuthChanged(AuthMethod),
    ConnectionMethodChanged(ConnectionMethod),
    PasswordChanged(SecretInput),
    KeyPassphraseChanged(SecretInput),
    ClearSecret(CredentialKind),
    ManagedSshChanged(bool),
    CopyKeyChanged(bool),
    ConnectionStartupChanged(bool),
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
    ConnectionSaved(Result<ServerMutation, String>),
    SaveConnectionPreferences,
    ConnectionPreferencesSaved(Result<SettingsMutation, String>),
    SettingsFieldChanged(SettingsField, String),
    BrowseCacheRoot,
    CacheRootPicked(Option<PathBuf>),
    CacheModeChanged(CacheMode),
    MountBackendChanged(MountBackend),
    CredentialStorageChanged(CredentialStorage),
    CredentialStorageDecision {
        target: CredentialStorage,
        result: rfd::MessageDialogResult,
    },
    SettingOptionChanged(SettingOption),
    CustomSettingDigitsChanged(String),
    CustomSettingUnitChanged(String),
    SaveCustomSetting,
    CancelCustomSetting,
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
    AppearanceModeChanged(AppearanceMode),
    AccentColorChanged(AccentColor),
    FontScaleChanged(FontScale),
    RegisterFileManagerMenu,
    UnregisterFileManagerMenu,
    FileManagerMenuFinished(Result<bool, String>),
    SaveSettings,
    SettingsSaved(Result<SettingsMutation, String>),
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
    RetryOperation(String, MountOperation),
    DismissOperationError(String),
    OpenOperationLog(String),
    Open(String),
    OpenFinished(Result<(), String>),
    Edit(String),
    Remove(String),
    CancelRemove,
    ConfirmRemove,
    RemoveFinished {
        id: String,
        result: Result<ServerMutation, String>,
    },
}

#[derive(Debug, Clone)]
struct ServerMutation {
    servers: Vec<ServerConfig>,
    warning: Option<String>,
}

#[derive(Debug, Clone)]
struct SettingsMutation {
    settings: Settings,
    servers: Vec<ServerConfig>,
    warning: Option<String>,
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
        } else if self.terminal_window == Some(window) {
            let name = self
                .terminal_server_id
                .as_deref()
                .and_then(|id| self.servers.iter().find(|server| server.id == id))
                .map(|server| server.display_name().to_owned())
                .unwrap_or_else(|| self.locale().text(TextKey::InteractiveTerminal).into());
            format!(
                "{name} - {}",
                self.locale().text(TextKey::InteractiveTerminal)
            )
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
        let (mode, accent) = effective_appearance(&self.settings, self.settings_draft.as_ref());
        application_theme(mode, accent, self.system_theme_dark)
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions: Vec<Subscription<Message>> = vec![
            Subscription::run_with(self.command_receiver.clone(), command_stream)
                .map(Message::AppCommand),
            Subscription::run_with(self.tray_actions.clone(), tray_stream).map(Message::TrayAction),
            iced::time::every(Duration::from_millis(100)).map(|_| Message::TrayTick),
            iced::time::every(Duration::from_millis(500)).map(|_| Message::InteractiveTick),
            iced::time::every(Duration::from_secs(1)).map(|_| Message::TransferTick),
            iced::time::every(Duration::from_secs(30)).map(|_| Message::CapacityTick),
            window::close_requests().map(Message::CloseRequested),
            window::close_events().map(Message::WindowClosed),
        ];
        subscriptions.extend(self.interactive_terminals.values().map(|session| {
            session
                .terminal
                .subscription()
                .map(|event| Message::TerminalEvent(RedactedTerminalEvent(event)))
        }));
        Subscription::batch(subscriptions)
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
        let system_locale = Locale::system();
        let system_theme_dark = system_prefers_dark();
        let service = MountService::new(paths.clone(), application_root());
        let recovered_settings = storage::load_settings_recovering(&paths);
        let mut settings_recovery_notice =
            settings_recovery_message(&recovered_settings, system_locale);
        let settings_recovery_dialog = settings_recovery_dialog_message(
            &recovered_settings,
            &paths.settings_file(),
            system_locale,
        );
        let settings_recovery_incomplete = recovered_settings.failure_stage.is_some();
        let mut settings = recovered_settings.settings;
        let locale =
            Locale::from_preference(Language::from_value(&settings.language), system_locale);
        let (mut servers, server_status, servers_loaded) = match storage::load_servers(&paths) {
            Ok(servers) => (
                servers,
                locale.text(TextKey::LoadingMountStatus).into(),
                true,
            ),
            Err(error) => (
                Vec::new(),
                match locale {
                    Locale::English => format!("Could not load existing configuration: {error}"),
                    Locale::Chinese => format!("无法加载现有配置：{error}"),
                },
                false,
            ),
        };
        if settings_need_system_credential_inference(&settings, &servers) {
            settings.credential_storage = CredentialStorage::System;
            if settings_recovery_incomplete {
                let detail = match locale {
                    Locale::English => {
                        "System credential references were detected. Their storage mode is active in memory, but it was not written automatically because settings recovery is incomplete; use Save after reviewing the recovery dialog."
                    }
                    Locale::Chinese => {
                        "检测到系统凭据引用；当前已在内存中启用对应存储模式，但因设置恢复尚未完成而未自动写入。请查看恢复弹窗后再主动点击“保存”。"
                    }
                };
                settings_recovery_notice = Some(
                    settings_recovery_notice
                        .map(|notice| format!("{notice}; {detail}"))
                        .unwrap_or_else(|| detail.into()),
                );
            } else if let Err(error) = storage::save_settings(&paths, &settings) {
                let detail = match locale {
                    Locale::English => format!(
                        "System credential references were detected, but their recovered storage setting could not be persisted: {error}"
                    ),
                    Locale::Chinese => {
                        format!("检测到系统凭据引用，但无法持久化恢复后的凭据存储设置：{error}")
                    }
                };
                settings_recovery_notice = Some(
                    settings_recovery_notice
                        .map(|notice| format!("{notice}; {detail}"))
                        .unwrap_or(detail),
                );
            }
        }
        let startup_migration_notice = if legacy_startup_migration_needed(&settings, servers_loaded)
        {
            match migrate_legacy_startup_preferences(&paths, &settings, &servers) {
                Ok((migrated_settings, migrated_servers)) => {
                    settings = migrated_settings;
                    servers = migrated_servers;
                    None
                }
                Err(error) => Some(match locale {
                    Locale::English => format!(
                        "Could not migrate the previous login startup selection; the legacy behavior remains active: {error}"
                    ),
                    Locale::Chinese => {
                        format!("无法迁移旧版登录自启选择，当前仍保留旧行为：{error}")
                    }
                }),
            }
        } else {
            None
        };
        let startup_notice = settings_recovery_notice
            .or(startup_migration_notice)
            .map(|notice| {
                if servers_loaded {
                    notice
                } else {
                    format!("{notice}; {server_status}")
                }
            });
        let status = startup_notice.clone().unwrap_or(server_status);
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
            startup_integration_lock: Arc::new(Mutex::new(())),
            startup_notice,
            system_locale,
            system_theme_dark,
            servers,
            connection_search: String::new(),
            connection_sort: ConnectionSort::default(),
            connection_tag_filter: None,
            connection_list_mode: ConnectionListMode::Browse,
            selected_connections: HashSet::new(),
            batch_tag_input: String::new(),
            batch_existing_tag: None,
            reorder_original: None,
            connection_list_saving: false,
            service,
            mount_statuses: HashMap::new(),
            busy: HashSet::new(),
            transfers: HashMap::new(),
            transfer_errors: HashMap::new(),
            operation_errors: HashMap::new(),
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
            tray_retry_at: None,
            tray_retry_delay: Duration::from_secs(1),
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
            connection_tags_input: String::new(),
            connection_custom_mountpoint: String::new(),
            mountpoint_preflight: MountpointPreflight::NotRequired,
            mountpoint_preflight_generation: 0,
            settings_draft: None,
            log_view: None,
            log_window: None,
            terminal_window: None,
            terminal_server_id: None,
            interactive_terminals: HashMap::new(),
            next_terminal_generation: 1,
            terminal_error: None,
            custom_setting: None,
            editor_saving: false,
            ssh_import_loading: false,
            ssh_import_plan: None,
            ssh_import_actions: Vec::new(),
            pending_delete: None,
            settings_recovery_dialog,
            status,
            status_generation: 0,
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
            app.status_task(StatusPublishPolicy::Initial),
        ];
        if screen == Screen::TransferCenter {
            tasks.push(app.transfer_task());
        }
        if app.settings.auto_check_updates {
            app.update_checking = true;
            tasks.push(app.check_update_task(false));
        }
        tasks.push(app.reconcile_startup_task());
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
            Message::InteractiveTick => return self.poll_interactive_terminals(),
            Message::InteractiveReadinessChecked {
                id,
                generation,
                result,
            } => {
                let Some(session) = self.interactive_terminals.get_mut(&id) else {
                    return Task::none();
                };
                if session.generation != generation {
                    return Task::none();
                }
                session.readiness_check_in_flight = false;
                if !interactive_readiness_result_is_current(session.lifecycle, session.queued_mount)
                {
                    return Task::none();
                }
                match result {
                    Ok(true) => {
                        session.lifecycle = InteractiveTerminalLifecycle::Ready;
                        self.status = locale.text(TextKey::InteractiveTerminalReady).into();
                        if interactive_mount_resume_once(
                            session.queued_mount,
                            session.resume_sent,
                            true,
                        ) {
                            session.resume_sent = true;
                            session.queued_mount = false;
                            return self.start_mount_operation(id, Some(MountOperation::Mount));
                        }
                    }
                    Ok(false) => {}
                    Err(error) => {
                        diagnostic_trace(&format!(
                            "interactive readiness check failed for {id} generation {generation}: {error}"
                        ));
                        session.lifecycle = InteractiveTerminalLifecycle::Failed;
                        session.queued_mount = false;
                        self.terminal_error = Some((id, error));
                        self.status = locale.text(TextKey::InteractiveTerminalFailed).into();
                    }
                }
            }
            Message::TerminalEvent(event) => return self.handle_terminal_event(event),
            Message::TerminalWindowOpened(id) => {
                if self.terminal_window == Some(id) {
                    diagnostic_trace(&format!("interactive terminal window opened {id:?}"));
                    return Task::none();
                }
            }
            Message::OpenInteractiveTerminal(id) => return self.open_terminal_window(id),
            Message::HideTerminal => {
                if let Some(id) = self.terminal_window.take() {
                    self.terminal_server_id = None;
                    return window::close(id);
                }
            }
            Message::EndInteractiveSession => return self.end_interactive_session(),
            Message::RetryTerminal => return self.retry_interactive_terminal(),
            Message::TrayTick => {
                if self.tray.is_some() {
                    TrayController::desktop_iteration();
                    self.sync_tray();
                } else if self.main_window_ready {
                    self.initialize_tray();
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
                    if let Some(description) = self.settings_recovery_dialog.take() {
                        tasks.push(Task::perform(
                            async move {
                                rfd::AsyncMessageDialog::new()
                                    .set_title(APP_NAME)
                                    .set_level(rfd::MessageLevel::Error)
                                    .set_description(description)
                                    .set_buttons(rfd::MessageButtons::Ok)
                                    .show()
                                    .await;
                            },
                            |_| Message::SettingsRecoveryDialogClosed,
                        ));
                    }
                    if self.pending_main_activation {
                        self.pending_main_activation = false;
                        tasks.push(self.activate_main_window());
                    }
                    if let Some(server_id) = self.terminal_server_id.clone() {
                        tasks.push(self.open_terminal_window(server_id));
                    }
                    return Task::batch(tasks);
                }
            }
            Message::SettingsRecoveryDialogClosed => {}
            Message::Refresh if self.connection_list_mode == ConnectionListMode::Reorder => {
                self.status = match locale {
                    Locale::English => "Save or cancel the current order before refreshing".into(),
                    Locale::Chinese => "请先保存或取消当前排序，再刷新".into(),
                };
            }
            Message::Refresh => match storage::load_servers(&self.paths) {
                Ok(servers) => {
                    self.servers = servers;
                    self.selected_connections
                        .retain(|id| self.servers.iter().any(|server| server.id == *id));
                    self.status = locale.text(TextKey::RefreshingMountStatus).into();
                    return self.status_task(StatusPublishPolicy::UserRefresh);
                }
                Err(error) => self.status = error.to_string(),
            },
            Message::RefreshFinished {
                generation,
                expected_status,
                result,
            } => {
                if !status_publication_is_current(
                    generation,
                    self.status_generation,
                    &expected_status,
                    &self.status,
                ) {
                    return Task::none();
                }
                match result {
                    Ok(result) => self.status = locale.refresh_complete(&result),
                    Err(error) => self.status = error,
                }
            }
            Message::StatusesLoaded {
                policy,
                generation,
                expected_status,
                results,
            } => {
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
                let can_publish = match (generation, expected_status.as_deref()) {
                    (Some(generation), Some(expected_status)) => status_publication_is_current(
                        generation,
                        self.status_generation,
                        expected_status,
                        &self.status,
                    ),
                    (None, None) => true,
                    _ => false,
                };
                if let Some(status) = status_completion_message(
                    policy,
                    &errors,
                    &mut self.startup_notice,
                    locale,
                    can_publish,
                ) {
                    self.status = status;
                }
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
                            && self.synced_polls.get(*id).copied().unwrap_or(0) >= 2
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
            Message::CloseRequested(id) if self.terminal_window == Some(id) => {
                self.terminal_window = None;
                self.terminal_server_id = None;
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
                } else if self.terminal_window == Some(id) {
                    self.terminal_window = None;
                    self.terminal_server_id = None;
                }
            }
            Message::AddConnection => {
                if self.connection_list_saving || self.editor_saving {
                    return Task::none();
                }
                let mut draft = ConnectionDraft::default();
                draft.ssh_config_path = default_ssh_config_path().display().to_string();
                self.connection_draft = Some(draft);
                self.connection_tags_input.clear();
                self.connection_custom_mountpoint.clear();
                self.mountpoint_preflight = MountpointPreflight::NotRequired;
                self.ssh_import_plan = None;
                self.ssh_import_actions.clear();
                self.screen = Screen::ConnectionEditor;
                self.status = locale.text(TextKey::NewConnection).into();
            }
            Message::ConnectionSearchChanged(value) => self.connection_search = value,
            Message::ConnectionSortMenuChanged(action) => match action {
                ConnectionSortMenuAction::Sort(value) => self.connection_sort = value,
                ConnectionSortMenuAction::AdjustSavedOrder => {
                    return self.update(Message::ConnectionListModeChanged(
                        ConnectionListMode::Reorder,
                    ));
                }
            },
            Message::ConnectionTagFilterChanged(tag) => self.connection_tag_filter = tag,
            Message::ConnectionListModeChanged(mode) => {
                if self.connection_list_saving || self.editor_saving {
                    return Task::none();
                }
                if mode == ConnectionListMode::Reorder {
                    self.reorder_original = Some(
                        self.servers
                            .iter()
                            .map(|server| server.id.clone())
                            .collect(),
                    );
                    self.connection_sort = ConnectionSort::SavedOrder;
                    self.connection_search.clear();
                    self.connection_tag_filter = None;
                } else if self.connection_list_mode == ConnectionListMode::Reorder {
                    self.reorder_original = None;
                }
                self.connection_list_mode = mode;
                self.selected_connections.clear();
                self.batch_tag_input.clear();
                self.batch_existing_tag = None;
            }
            Message::BatchSelectionChanged(id, selected) => {
                if selected {
                    self.selected_connections.insert(id);
                } else {
                    self.selected_connections.remove(&id);
                }
            }
            Message::BatchSelectAllChanged(selected) => {
                let visible_ids = visible_connections(
                    &self.servers,
                    &self.connection_search,
                    self.connection_tag_filter.as_deref(),
                    self.connection_sort,
                )
                .into_iter()
                .map(|server| server.id.clone())
                .collect::<Vec<_>>();
                if selected {
                    self.selected_connections.extend(visible_ids);
                } else {
                    for id in visible_ids {
                        self.selected_connections.remove(&id);
                    }
                }
            }
            Message::BatchTagSelectionChanged(tag, selected) => {
                for server in &self.servers {
                    if server.tags.iter().any(|candidate| candidate == &tag) {
                        if selected {
                            self.selected_connections.insert(server.id.clone());
                        } else {
                            self.selected_connections.remove(&server.id);
                        }
                    }
                }
            }
            Message::BatchTagInputChanged(value) => self.batch_tag_input = value,
            Message::BatchExistingTagChanged(tag) => self.batch_existing_tag = Some(tag),
            Message::BatchAddTag => {
                let tag = match batch_tag_to_add(
                    &self.batch_tag_input,
                    self.batch_existing_tag.as_deref(),
                    locale,
                ) {
                    Ok(tag) => tag,
                    Err(error) => {
                        self.status = error;
                        return Task::none();
                    }
                };
                let updates = self
                    .servers
                    .iter()
                    .filter(|server| self.selected_connections.contains(&server.id))
                    .map(|server| -> Result<_, String> {
                        let mut tags = server.tags.clone();
                        if !tags.iter().any(|candidate| candidate == &tag) {
                            tags.push(tag.clone());
                        }
                        let tags = validated_connection_tags(&tags, locale)?;
                        Ok(storage::ServerPreferenceUpdate {
                            id: server.id.clone(),
                            tags: Some(tags),
                            auto_mount_at_login: None,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>();
                let updates = match updates {
                    Ok(updates) => updates,
                    Err(error) => {
                        self.status = error;
                        return Task::none();
                    }
                };
                return self.save_batch_tag_updates(updates);
            }
            Message::RemoveConnectionTag(id, tag) => {
                let Some(server) = self.servers.iter().find(|server| server.id == id) else {
                    return Task::none();
                };
                let tags = server
                    .tags
                    .iter()
                    .filter(|candidate| *candidate != &tag)
                    .cloned()
                    .collect();
                return self.save_batch_tag_updates(vec![storage::ServerPreferenceUpdate {
                    id,
                    tags: Some(tags),
                    auto_mount_at_login: None,
                }]);
            }
            Message::RemoveTagEverywhere(tag) => {
                if self.editor_saving || self.connection_list_saving {
                    return Task::none();
                }
                let description = match locale {
                    Locale::English => {
                        format!(
                            "Remove tag '{tag}' from every connection? Connections will not be deleted."
                        )
                    }
                    Locale::Chinese => {
                        format!("从所有连接中移除标签“{tag}”？连接本身不会被删除。")
                    }
                };
                let decision_tag = tag.clone();
                self.connection_list_saving = true;
                return Task::perform(
                    async move {
                        rfd::AsyncMessageDialog::new()
                            .set_title(APP_NAME)
                            .set_description(description)
                            .set_buttons(rfd::MessageButtons::YesNo)
                            .show()
                            .await
                    },
                    move |result| Message::RemoveTagEverywhereDecision {
                        tag: decision_tag.clone(),
                        result,
                    },
                );
            }
            Message::RemoveTagEverywhereDecision { tag, result } => {
                if result != rfd::MessageDialogResult::Yes {
                    self.connection_list_saving = false;
                    return Task::none();
                }
                if self.editor_saving || !self.connection_list_saving {
                    self.connection_list_saving = false;
                    return Task::none();
                }
                let updates = self
                    .servers
                    .iter()
                    .filter(|server| server.tags.iter().any(|candidate| candidate == &tag))
                    .map(|server| storage::ServerPreferenceUpdate {
                        id: server.id.clone(),
                        tags: Some(
                            server
                                .tags
                                .iter()
                                .filter(|candidate| *candidate != &tag)
                                .cloned()
                                .collect(),
                        ),
                        auto_mount_at_login: None,
                    })
                    .collect();
                self.connection_list_saving = false;
                return self.save_batch_tag_updates(updates);
            }
            Message::BatchTagsSaved(result) => match result {
                Ok(servers) => {
                    self.connection_list_saving = false;
                    self.servers = servers;
                    if self.connection_tag_filter.as_ref().is_some_and(|filter| {
                        !self
                            .servers
                            .iter()
                            .any(|server| server.tags.contains(filter))
                    }) {
                        self.connection_tag_filter = None;
                    }
                    self.batch_tag_input.clear();
                    self.batch_existing_tag = None;
                    self.status = match locale {
                        Locale::English => "Connection changes saved".into(),
                        Locale::Chinese => "连接修改已保存".into(),
                    };
                }
                Err(error) => {
                    self.connection_list_saving = false;
                    self.status = error;
                }
            },
            Message::BatchMountSelected => {
                let ids = selected_server_ids(&self.servers, &self.selected_connections);
                let mut tasks = Vec::new();
                for id in ids {
                    tasks.push(self.handle_mount_command(id, MountOperation::Mount));
                }
                return Task::batch(tasks);
            }
            Message::BatchUnmountSelected => {
                let ids = selected_server_ids(&self.servers, &self.selected_connections)
                    .into_iter()
                    .filter(|id| self.paths.state_file(id).exists())
                    .collect::<Vec<_>>();
                let (unsafe_ids, safe_ids): (Vec<_>, Vec<_>) = ids.into_iter().partition(|id| {
                    unmount_needs_confirmation(
                        self.transfers.get(id),
                        self.transfer_errors.contains_key(id),
                        self.synced_polls.get(id).copied().unwrap_or(0),
                    )
                });
                let mut tasks = Vec::new();
                for id in safe_ids {
                    tasks.push(self.handle_mount_command(id, MountOperation::Unmount));
                }
                if !unsafe_ids.is_empty() {
                    tasks.push(self.confirm_waiting_unmount(unsafe_ids));
                }
                return Task::batch(tasks);
            }
            Message::BatchStartupChanged(id, enabled) => {
                if self.editor_saving || self.connection_list_saving {
                    return Task::none();
                }
                let updates = vec![storage::ServerPreferenceUpdate {
                    id,
                    tags: None,
                    auto_mount_at_login: Some(enabled),
                }];
                let startup = self.startup_integration_lock.clone();
                let paths = self.paths.clone();
                self.connection_list_saving = true;
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let servers =
                                storage::update_server_preferences_batch(&paths, &updates)
                                    .map_err(|error| error.to_string())?;
                            let warning = reconcile_login_startup(&paths, &startup)
                                .err()
                                .map(|error| match locale {
                                    Locale::English => format!(
                                        "Login startup preferences were saved, but system startup integration will be retried: {error}"
                                    ),
                                    Locale::Chinese => format!(
                                        "登录自启设置已保存，但系统自启集成失败，将稍后重试：{error}"
                                    ),
                                });
                            Ok::<_, String>(ServerMutation { servers, warning })
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                    },
                    Message::BatchStartupSaved,
                );
            }
            Message::BatchStartupSaved(result) => {
                self.connection_list_saving = false;
                match result {
                    Ok(outcome) => {
                        self.servers = outcome.servers;
                        self.status = match outcome.warning {
                            Some(warning) => warning,
                            None => match locale {
                                Locale::English => "Login startup preferences saved".into(),
                                Locale::Chinese => "登录自启设置已保存".into(),
                            },
                        };
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::BatchDeleteSelected => {
                if self.editor_saving || self.connection_list_saving {
                    return Task::none();
                }
                let ids = selected_server_ids(&self.servers, &self.selected_connections);
                if ids.is_empty() {
                    return Task::none();
                }
                if ids.iter().any(|id| !self.can_modify(id)) {
                    self.status = locale.text(TextKey::UnmountBeforeRemove).into();
                    return Task::none();
                }
                let mut selected_names = ids
                    .iter()
                    .filter_map(|id| self.servers.iter().find(|server| server.id == *id))
                    .map(|server| server.display_name().to_owned())
                    .take(8)
                    .collect::<Vec<_>>();
                if ids.len() > selected_names.len() {
                    selected_names.push(match locale {
                        Locale::English => format!(
                            "... and {} more",
                            ids.len().saturating_sub(selected_names.len())
                        ),
                        Locale::Chinese => format!(
                            "……以及另外 {} 个",
                            ids.len().saturating_sub(selected_names.len())
                        ),
                    });
                }
                let selected_names = selected_names.join("\n");
                let description = match locale {
                    Locale::English => format!(
                        "Permanently remove {} selected connection(s)? Saved credentials and managed SSH entries will also be cleaned up.\n\nSelected:\n{selected_names}",
                        ids.len()
                    ),
                    Locale::Chinese => format!(
                        "永久删除选中的 {} 个连接？关联凭据和托管 SSH 配置也会被清理。\n\n已选择：\n{selected_names}",
                        ids.len()
                    ),
                };
                let decision_ids = ids.clone();
                self.connection_list_saving = true;
                return Task::perform(
                    async move {
                        rfd::AsyncMessageDialog::new()
                            .set_title(APP_NAME)
                            .set_level(rfd::MessageLevel::Warning)
                            .set_description(description)
                            .set_buttons(rfd::MessageButtons::YesNo)
                            .show()
                            .await
                    },
                    move |result| Message::BatchDeleteDecision {
                        ids: decision_ids.clone(),
                        result,
                    },
                );
            }
            Message::BatchDeleteDecision { ids, result } => {
                if result != rfd::MessageDialogResult::Yes {
                    self.connection_list_saving = false;
                    return Task::none();
                }
                if self.editor_saving
                    || !self.connection_list_saving
                    || ids.iter().any(|id| !self.can_modify(id))
                {
                    self.connection_list_saving = false;
                    return Task::none();
                }
                let removed_servers = ids
                    .iter()
                    .filter_map(|id| self.servers.iter().find(|server| server.id == *id))
                    .cloned()
                    .collect::<Vec<_>>();
                self.busy.extend(ids.iter().cloned());
                self.status = match locale {
                    Locale::English => format!("Removing {} connection(s)...", ids.len()),
                    Locale::Chinese => format!("正在删除 {} 个连接…", ids.len()),
                };
                let paths = self.paths.clone();
                let result_ids = ids.clone();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let servers = storage::remove_servers(&paths, &ids)
                                .map_err(|error| error.to_string())?;
                            let mut cleanup_errors = Vec::new();
                            for server in &removed_servers {
                                if server.ssh_config_managed
                                    && let Err(error) = remove_managed_ssh_server(server)
                                {
                                    cleanup_errors.push(format!(
                                        "{}: managed SSH cleanup failed: {error}",
                                        server.display_name()
                                    ));
                                }
                                if let Err(error) =
                                    delete_server_credentials(server, &SystemCredentialStore)
                                {
                                    cleanup_errors.push(format!(
                                        "{}: credential cleanup failed: {error}",
                                        server.display_name()
                                    ));
                                }
                            }
                            let warning = (!cleanup_errors.is_empty()).then(|| match locale {
                                Locale::English => {
                                    format!("Connections removed; {}", cleanup_errors.join("; "))
                                }
                                Locale::Chinese => {
                                    format!("连接已删除，但{}", cleanup_errors.join("；"))
                                }
                            });
                            if let Some(warning) = &warning {
                                diagnostic_trace(warning);
                            }
                            Ok(ServerMutation { servers, warning })
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                    },
                    move |result| Message::BatchRemoved {
                        ids: result_ids.clone(),
                        result,
                    },
                );
            }
            Message::BatchRemoved { ids, result } => {
                self.connection_list_saving = false;
                for id in &ids {
                    self.busy.remove(id);
                }
                match result {
                    Ok(outcome) => {
                        let terminal_task = self.reconcile_interactive_sessions(&outcome.servers);
                        self.servers = outcome.servers;
                        self.selected_connections
                            .retain(|id| self.servers.iter().any(|server| server.id == *id));
                        self.status = outcome.warning.unwrap_or_else(|| match locale {
                            Locale::English => "Selected connections removed".into(),
                            Locale::Chinese => "所选连接已删除".into(),
                        });
                        return Task::batch([terminal_task, self.reconcile_startup_task()]);
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::MoveConnection(id, direction) => {
                if self.connection_list_saving || self.editor_saving {
                    return Task::none();
                }
                let Some(order) = moved_connection_order(&self.servers, &id, direction) else {
                    return Task::none();
                };
                let by_id = self
                    .servers
                    .iter()
                    .cloned()
                    .map(|server| (server.id.clone(), server))
                    .collect::<HashMap<_, _>>();
                self.servers = order
                    .into_iter()
                    .filter_map(|id| by_id.get(&id).cloned())
                    .collect();
            }
            Message::SaveConnectionOrder => {
                if self.connection_list_mode != ConnectionListMode::Reorder
                    || self.connection_list_saving
                {
                    return Task::none();
                }
                let paths = self.paths.clone();
                let order = self
                    .servers
                    .iter()
                    .map(|server| server.id.clone())
                    .collect::<Vec<_>>();
                self.connection_list_saving = true;
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            storage::reorder_servers(&paths, &order)
                        })
                        .await
                        .unwrap_or_else(|error| {
                            Err(storage::StorageError::InvalidPreferenceUpdate(
                                error.to_string(),
                            ))
                        })
                        .map_err(|error| error.to_string())
                    },
                    Message::ConnectionsReordered,
                );
            }
            Message::CancelConnectionOrder => {
                if self.connection_list_saving {
                    return Task::none();
                }
                if let Some(original) = self.reorder_original.take() {
                    let by_id = self
                        .servers
                        .iter()
                        .cloned()
                        .map(|server| (server.id.clone(), server))
                        .collect::<HashMap<_, _>>();
                    self.servers = original
                        .into_iter()
                        .filter_map(|id| by_id.get(&id).cloned())
                        .collect();
                }
                self.connection_list_mode = ConnectionListMode::Browse;
            }
            Message::ConnectionsReordered(result) => match result {
                Ok(servers) => {
                    self.connection_list_saving = false;
                    self.servers = servers;
                    self.reorder_original = None;
                    self.connection_list_mode = ConnectionListMode::Browse;
                    self.status = match locale {
                        Locale::English => "Custom order saved".into(),
                        Locale::Chinese => "自定义排序已保存".into(),
                    };
                }
                Err(error) => {
                    self.connection_list_saving = false;
                    self.status = error;
                }
            },
            Message::SettingsConnectionStartupChanged(id, value) => {
                if let Some(preference) = self.settings_draft.as_mut().and_then(|draft| {
                    draft
                        .connection_preferences
                        .iter_mut()
                        .find(|preference| preference.id == id)
                }) && preference.startup_available
                {
                    preference.auto_mount_at_login = value;
                }
            }
            Message::ToggleSettingsConnectionPreferences => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.connection_preferences_expanded = !draft.connection_preferences_expanded;
                }
            }
            Message::OpenBatchManagement => {
                if self.editor_saving {
                    return Task::none();
                }
                self.settings_draft = None;
                self.screen = Screen::Connections;
                self.connection_list_mode = ConnectionListMode::Batch;
                self.selected_connections.clear();
                self.status = match locale {
                    Locale::English => "Batch connection management".into(),
                    Locale::Chinese => "批量管理连接".into(),
                };
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
                if self.connection_list_saving || self.editor_saving {
                    return Task::none();
                }
                let mut draft = SettingsDraft::from_settings(&self.settings);
                draft.connection_preferences = self
                    .servers
                    .iter()
                    .map(|server| {
                        ConnectionPreferenceDraft::from_server(server, self.settings.startup_all)
                    })
                    .collect();
                self.settings_draft = Some(draft);
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
                        log_view.existed = log.existed;
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
                    self.mountpoint_preflight = MountpointPreflight::NotRequired;
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
            Message::ConnectionTagsChanged(value) => {
                self.connection_tags_input = value.clone();
                if let Some(draft) = &mut self.connection_draft {
                    draft.tags = parse_tag_input(&value);
                    draft.folder.clear();
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
                        self.connection_custom_mountpoint =
                            custom_mountpoint_value(&draft.mountpoint);
                    }
                    draft.mountpoint =
                        mountpoint_value_for_choice(&choice, &self.connection_custom_mountpoint);
                }
                if choice == "custom" {
                    return self.start_mountpoint_preflight();
                }
                self.mountpoint_preflight = MountpointPreflight::NotRequired;
            }
            Message::CustomMountpointChanged(value) => {
                self.connection_custom_mountpoint = value.clone();
                if let Some(draft) = &mut self.connection_draft {
                    draft.mountpoint = custom_mountpoint_draft_value(value);
                }
                return self.start_mountpoint_preflight();
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
                return self.start_mountpoint_preflight();
            }
            Message::MountpointPicked(None) => {}
            Message::MountpointPreflightFinished {
                generation,
                value,
                result,
            } => {
                let current = self.connection_draft.as_ref().map(|draft| {
                    if draft.mountpoint == CUSTOM_MOUNTPOINT_PENDING {
                        self.connection_custom_mountpoint.trim()
                    } else {
                        draft.mountpoint.trim()
                    }
                });
                if !mountpoint_preflight_result_is_current(
                    generation,
                    self.mountpoint_preflight_generation,
                    &value,
                    current,
                ) {
                    return Task::none();
                }
                self.mountpoint_preflight = match result {
                    Ok(()) => MountpointPreflight::Valid(value),
                    Err(error) => MountpointPreflight::Invalid { value, error },
                };
            }
            Message::ConnectionAuthChanged(auth) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.auth = auth;
                }
            }
            Message::ConnectionMethodChanged(method) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.connection_method = method;
                    if method != ConnectionMethod::Native {
                        draft.auth = AuthMethod::Key;
                    }
                    if method == ConnectionMethod::Interactive {
                        draft.auto_mount_at_login = false;
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
            Message::ClearSecret(kind) => {
                if let Some(draft) = &mut self.connection_draft {
                    draft.clear_preserved_secret(kind);
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
            Message::ConnectionStartupChanged(value) => {
                if let Some(draft) = &mut self.connection_draft
                    && draft.connection_method != ConnectionMethod::Interactive
                {
                    draft.auto_mount_at_login = value;
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
                    Ok(outcome) => {
                        let terminal_task = self.reconcile_interactive_sessions(&outcome.servers);
                        self.servers = outcome.servers;
                        self.connection_draft = None;
                        self.mountpoint_preflight = MountpointPreflight::NotRequired;
                        self.screen = Screen::Connections;
                        self.status = outcome
                            .warning
                            .unwrap_or_else(|| locale.text(TextKey::ConnectionSaved).into());
                        return Task::batch([
                            terminal_task,
                            self.status_task(StatusPublishPolicy::Silent),
                            self.reconcile_startup_task(),
                        ]);
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::SaveConnectionPreferences => return self.save_connection_preferences(),
            Message::ConnectionPreferencesSaved(result) => {
                self.editor_saving = false;
                match result {
                    Ok(outcome) => {
                        self.settings = outcome.settings;
                        self.servers = outcome.servers;
                        self.connection_draft = None;
                        self.screen = Screen::Connections;
                        self.status = outcome.warning.unwrap_or_else(|| match locale {
                            Locale::English => "Connection organization saved".into(),
                            Locale::Chinese => "连接整理设置已保存".into(),
                        });
                        return self.status_task(StatusPublishPolicy::Silent);
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
            Message::MountBackendChanged(backend) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.mount_backend = backend;
                }
            }
            Message::CredentialStorageChanged(target) => {
                let current = self
                    .settings_draft
                    .as_ref()
                    .map_or(self.settings.credential_storage, |draft| {
                        draft.credential_storage
                    });
                if target == current {
                    return Task::none();
                }
                let description = credential_storage_confirmation(locale, target);
                return Task::perform(
                    async move {
                        rfd::AsyncMessageDialog::new()
                            .set_title(APP_NAME)
                            .set_description(description)
                            .set_buttons(rfd::MessageButtons::YesNo)
                            .show()
                            .await
                    },
                    move |result| Message::CredentialStorageDecision { target, result },
                );
            }
            Message::CredentialStorageDecision { target, result } => {
                if result == rfd::MessageDialogResult::Yes
                    && let Some(draft) = &mut self.settings_draft
                {
                    draft.credential_storage = target;
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
                        digits: if custom_setting_is_supported(option.kind, current) {
                            digits
                        } else {
                            current.to_owned()
                        },
                        unit,
                        raw_value: (!custom_setting_is_supported(option.kind, current))
                            .then(|| current.to_owned()),
                    });
                } else if let Some(draft) = &mut self.settings_draft {
                    set_setting_value(draft, option.kind, option.value);
                }
            }
            Message::CustomSettingDigitsChanged(value) => {
                if let Some(custom) = &mut self.custom_setting {
                    if custom.raw_value.is_some() {
                        custom.digits = value.clone();
                        custom.raw_value = Some(value);
                    } else {
                        custom.digits = value
                            .chars()
                            .filter(|character| character.is_ascii_digit())
                            .collect();
                    }
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
                    match custom_setting_value(&custom, locale) {
                        Ok(value) => {
                            if let Some(draft) = &mut self.settings_draft {
                                set_setting_value(draft, custom.kind, value);
                            }
                        }
                        Err(error) => {
                            self.status = error;
                            self.custom_setting = Some(custom);
                        }
                    }
                }
            }
            Message::CancelCustomSetting => self.custom_setting = None,
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
            Message::AppearanceModeChanged(mode) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.appearance_mode = mode;
                }
            }
            Message::AccentColorChanged(accent) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.accent_color = accent;
                }
            }
            Message::FontScaleChanged(font_scale) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.font_scale = font_scale;
                }
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
                    Ok(outcome) => {
                        self.settings = outcome.settings;
                        self.servers = outcome.servers;
                        self.settings_draft = None;
                        self.screen = Screen::Connections;
                        self.status = outcome
                            .warning
                            .unwrap_or_else(|| locale.text(TextKey::SettingsSaved).into());
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::StartupReconciled(result) => {
                if let Err(error) = result {
                    diagnostic_trace(&format!("login startup reconciliation failed: {error}"));
                    self.status = match locale {
                        Locale::English => format!(
                            "Login startup integration failed and will be retried next launch: {error}"
                        ),
                        Locale::Chinese => {
                            format!("登录自启集成失败，将在下次启动时重试：{error}")
                        }
                    };
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
                self.invalidate_status_publications();
                self.busy.remove(&id);
                let mut tasks = Vec::new();
                match result {
                    Ok(message) => {
                        self.operation_errors.remove(&id);
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
                        self.operation_errors.insert(
                            id.clone(),
                            ConnectionOperationError {
                                operation,
                                cause: error.clone(),
                            },
                        );
                        self.status = error;
                        tasks.push(self.status_task(StatusPublishPolicy::Silent));
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
            Message::RetryOperation(id, operation) => {
                return self.start_mount_operation(id, Some(operation));
            }
            Message::DismissOperationError(id) => {
                self.operation_errors.remove(&id);
            }
            Message::OpenOperationLog(id) => return self.open_log(id),
            Message::Open(id) => return self.open_mountpoint(id),
            Message::OpenFinished(result) => match result {
                Ok(()) => self.status = locale.text(TextKey::OpenedMountpoint).into(),
                Err(error) => self.status = error,
            },
            Message::Edit(id) => {
                if self.connection_list_saving || self.editor_saving {
                    return Task::none();
                }
                let can_modify = self.can_modify(&id);
                if let Some(server) = self.servers.iter().find(|server| server.id == id) {
                    self.connection_draft = Some(ConnectionDraft::from_server(server));
                    self.connection_tags_input = server.tags.join(", ");
                    if let Some(draft) = &mut self.connection_draft {
                        draft.auto_mount_at_login |= self.settings.startup_all;
                    }
                    self.connection_custom_mountpoint = custom_mountpoint_value(&server.mountpoint);
                    if let Some(draft) = &mut self.connection_draft
                        && draft.ssh_config_path.trim().is_empty()
                    {
                        draft.ssh_config_path = default_ssh_config_path().display().to_string();
                    }
                    self.ssh_import_plan = None;
                    self.ssh_import_actions.clear();
                    self.screen = Screen::ConnectionEditor;
                    self.status = if can_modify {
                        locale.editing(server.display_name())
                    } else {
                        connection_settings_locked_help(locale).into()
                    };
                    if can_modify && mountpoint_choice(&server.mountpoint) == "custom" {
                        return self.start_mountpoint_preflight();
                    }
                    self.mountpoint_preflight = MountpointPreflight::NotRequired;
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
                if !self.busy.insert(id.clone()) {
                    return Task::none();
                }
                self.editor_saving = true;
                let paths = self.paths.clone();
                let server = self.servers.iter().find(|server| server.id == id).cloned();
                let result_id = id.clone();
                self.status = locale.removing(&operation_display_name(server.as_ref(), &id));
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let servers = storage::remove_server(&paths, &id)
                                .map_err(|error| error.to_string())?;
                            let mut cleanup_errors = Vec::new();
                            if let Some(server) = &server {
                                if server.ssh_config_managed
                                    && let Err(error) = remove_managed_ssh_server(server)
                                {
                                    cleanup_errors.push(match locale {
                                        Locale::English => {
                                            format!("managed SSH cleanup failed: {error}")
                                        }
                                        Locale::Chinese => {
                                            format!("托管 SSH 配置清理失败：{error}")
                                        }
                                    });
                                }
                                if let Err(error) =
                                    delete_server_credentials(server, &SystemCredentialStore)
                                {
                                    cleanup_errors.push(match locale {
                                        Locale::English => {
                                            format!("credential cleanup failed: {error}")
                                        }
                                        Locale::Chinese => {
                                            format!("系统凭据清理失败：{error}")
                                        }
                                    });
                                }
                            }
                            let warning = (!cleanup_errors.is_empty()).then(|| match locale {
                                Locale::English => {
                                    format!("Connection removed; {}", cleanup_errors.join("; "))
                                }
                                Locale::Chinese => {
                                    format!("连接已删除，但{}", cleanup_errors.join("；"))
                                }
                            });
                            if let Some(warning) = &warning {
                                diagnostic_trace(warning);
                            }
                            Ok(ServerMutation { servers, warning })
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                    },
                    move |result| Message::RemoveFinished {
                        id: result_id.clone(),
                        result,
                    },
                );
            }
            Message::RemoveFinished { id, result } => {
                self.editor_saving = false;
                self.busy.remove(&id);
                match result {
                    Ok(outcome) => {
                        let terminal_task = self.reconcile_interactive_sessions(&outcome.servers);
                        self.servers = outcome.servers;
                        self.selected_connections.retain(|selected| {
                            self.servers.iter().any(|server| server.id == *selected)
                        });
                        self.status = outcome
                            .warning
                            .unwrap_or_else(|| locale.text(TextKey::ConnectionRemoved).into());
                        return Task::batch([terminal_task, self.reconcile_startup_task()]);
                    }
                    Err(error) => self.status = error,
                }
            }
        }
        Task::none()
    }

    fn initialize_tray(&mut self) {
        if self.tray.is_some()
            || self
                .tray_retry_at
                .is_some_and(|retry_at| Instant::now() < retry_at)
            || (self.tray_error.is_some() && self.tray_retry_at.is_none())
        {
            return;
        }
        match TrayController::new(self.locale(), self.tray_action_sender.clone()) {
            Ok(tray) => {
                self.tray = Some(tray);
                self.tray_error = None;
                self.tray_retry_at = None;
                self.tray_retry_delay = Duration::from_secs(1);
                self.sync_tray();
                diagnostic_trace("tray initialized");
            }
            Err(error) => {
                diagnostic_trace(&format!("tray unavailable: {error}"));
                let message = error.to_string();
                self.tray_error = Some(message.clone());
                self.status = self.locale().tray_unavailable(&message);
                match error {
                    TrayError::Transient(_) => {
                        self.tray_retry_at = Some(Instant::now() + self.tray_retry_delay);
                        self.tray_retry_delay =
                            (self.tray_retry_delay * 2).min(Duration::from_secs(30));
                    }
                    TrayError::Permanent(_) => self.tray_retry_at = None,
                }
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
                let generation = self.claim_status_generation();
                let expected_status = self.status.clone();
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
                    move |result| Message::RefreshFinished {
                        generation,
                        expected_status,
                        result,
                    },
                )
            }
            AppCommand::Refresh { id, relative_dir } => {
                if !self.servers.iter().any(|server| server.id == id) {
                    self.status = self.locale().text(TextKey::ConnectionGone).into();
                    return Task::none();
                }
                self.status = self.locale().text(TextKey::Refreshing).into();
                let generation = self.claim_status_generation();
                let expected_status = self.status.clone();
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
                    move |result| Message::RefreshFinished {
                        generation,
                        expected_status,
                        result,
                    },
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
            AppCommand::MountStartup => {
                let ids = startup_servers(&self.settings, &self.servers)
                    .into_iter()
                    .map(|server| server.id)
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
                        self.synced_polls.get(id).copied().unwrap_or(0),
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
                    self.request_exit()
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
        let interactive_sessions = self
            .interactive_terminals
            .values()
            .filter(|session| interactive_terminal_is_live(session.lifecycle))
            .count();
        if active == 0 && unknown == 0 && interactive_sessions == 0 {
            return iced::exit();
        }
        self.exit_confirmation_open = true;
        let description = self
            .locale()
            .exit_warning(active, unknown, interactive_sessions);
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
        if self.editor_saving || self.connection_list_saving {
            return Task::none();
        }
        if self.connection_draft.as_ref().is_some_and(|draft| {
            draft
                .editing_id
                .as_deref()
                .is_some_and(|id| !self.can_modify(id))
        }) {
            self.status = connection_settings_locked_help(self.locale()).into();
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
        let credential_storage = self.settings.credential_storage;
        let custom_mountpoint = (mountpoint_choice(&validated.server.mountpoint) == "custom")
            .then(|| validated.server.mountpoint.clone());
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    if let Some(mountpoint) = custom_mountpoint {
                        let home = directories::BaseDirs::new()
                            .map(|directories| directories.home_dir().to_owned())
                            .unwrap_or_else(|| PathBuf::from("."));
                        preflight_custom_mountpoint(&mountpoint, &home)
                            .map_err(|error| error.to_string())?;
                    }
                    let server_id = validated.server.id.clone();
                    let password = prepare_secret_action(
                        &service,
                        credential_storage,
                        &server_id,
                        CredentialKind::Password,
                        &validated.password,
                    )?;
                    let key_passphrase = match prepare_secret_action(
                        &service,
                        credential_storage,
                        &server_id,
                        CredentialKind::KeyPassphrase,
                        &validated.key_passphrase,
                    ) {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            let _ = rollback_prepared_secret(&password);
                            return Err(error);
                        }
                    };
                    let mut server = match validated
                        .apply_secrets(password.obscured.clone(), key_passphrase.obscured.clone())
                    {
                        Ok(server) => server,
                        Err(error) => {
                            let _ = rollback_prepared_secrets([&password, &key_passphrase]);
                            return Err(localize_draft_error(locale, &error));
                        }
                    };
                    password.apply(&mut server);
                    key_passphrase.apply(&mut server);
                    let managed_snapshot = match capture_managed_profile(previous.as_ref()) {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            let rollback = rollback_prepared_secrets([&password, &key_passphrase]);
                            return Err(match rollback {
                                Ok(()) => error,
                                Err(rollback) => {
                                    format!("{error}; credential rollback failed: {rollback}")
                                }
                            });
                        }
                    };
                    if let Err(error) = prepare_managed_ssh_server(&mut server, &Platform) {
                        let rollback = rollback_prepared_secrets([&password, &key_passphrase]);
                        return Err(match rollback {
                            Ok(()) => error.to_string(),
                            Err(rollback) => {
                                format!("{error}; credential rollback failed: {rollback}")
                            }
                        });
                    }
                    let servers = match storage::upsert_server(&paths, server.clone()) {
                        Ok(servers) => servers,
                        Err(error) => {
                            let managed_rollback = rollback_prepared_managed_profile(
                                &server,
                                managed_snapshot.as_ref(),
                            );
                            let credential_rollback =
                                rollback_prepared_secrets([&password, &key_passphrase]);
                            let mut message = error.to_string();
                            if let Err(rollback) = managed_rollback {
                                message.push_str(&format!(
                                    "; managed SSH rollback failed: {rollback}"
                                ));
                            }
                            if let Err(rollback) = credential_rollback {
                                message
                                    .push_str(&format!("; credential rollback failed: {rollback}"));
                            }
                            return Err(message);
                        }
                    };
                    let mut cleanup_errors = Vec::new();
                    if let Some(previous) = &previous {
                        if previous.ssh_config_managed
                            && (!server.ssh_config_managed
                                || previous.managed_ssh_config_path
                                    != server.managed_ssh_config_path)
                            && let Err(error) = remove_managed_ssh_server(previous)
                        {
                            cleanup_errors.push(match locale {
                                Locale::English => {
                                    format!("old managed SSH cleanup failed: {error}")
                                }
                                Locale::Chinese => {
                                    format!("旧托管 SSH 配置清理失败：{error}")
                                }
                            });
                        }
                        if let Err(error) = delete_retired_connection_credentials(previous, &server)
                        {
                            cleanup_errors.push(match locale {
                                Locale::English => {
                                    format!("retired credential cleanup failed: {error}")
                                }
                                Locale::Chinese => {
                                    format!("旧系统凭据清理失败：{error}")
                                }
                            });
                        }
                    }
                    let warning = (!cleanup_errors.is_empty()).then(|| match locale {
                        Locale::English => {
                            format!("Connection saved; {}", cleanup_errors.join("; "))
                        }
                        Locale::Chinese => {
                            format!("连接已保存，但{}", cleanup_errors.join("；"))
                        }
                    });
                    if let Some(warning) = &warning {
                        diagnostic_trace(warning);
                    }
                    Ok(ServerMutation { servers, warning })
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::ConnectionSaved,
        )
    }

    fn start_mountpoint_preflight(&mut self) -> Task<Message> {
        let Some(draft) = &self.connection_draft else {
            self.mountpoint_preflight = MountpointPreflight::NotRequired;
            return Task::none();
        };
        if mountpoint_choice(&draft.mountpoint) != "custom" {
            self.mountpoint_preflight = MountpointPreflight::NotRequired;
            return Task::none();
        }
        let value = if draft.mountpoint == CUSTOM_MOUNTPOINT_PENDING {
            self.connection_custom_mountpoint.trim().to_owned()
        } else {
            draft.mountpoint.trim().to_owned()
        };
        if value.is_empty() {
            self.mountpoint_preflight = MountpointPreflight::Invalid {
                value,
                error: match self.locale() {
                    Locale::English => "Select a custom mountpoint".into(),
                    Locale::Chinese => "请选择自定义挂载点".into(),
                },
            };
            return Task::none();
        }
        self.mountpoint_preflight = MountpointPreflight::Checking(value.clone());
        self.mountpoint_preflight_generation =
            self.mountpoint_preflight_generation.saturating_add(1);
        let generation = self.mountpoint_preflight_generation;
        let checked_value = value.clone();
        let failed_value = value.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let home = directories::BaseDirs::new()
                        .map(|directories| directories.home_dir().to_owned())
                        .unwrap_or_else(|| PathBuf::from("."));
                    let result = preflight_custom_mountpoint(&checked_value, &home)
                        .map(|_| ())
                        .map_err(|error| error.to_string());
                    (checked_value, result)
                })
                .await
                .unwrap_or_else(|error| (failed_value, Err(error.to_string())))
            },
            move |(value, result)| Message::MountpointPreflightFinished {
                generation,
                value,
                result,
            },
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
        let previous_servers = self.servers.clone();
        let locale = self.locale();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    storage::upsert_servers(&paths, updates)
                        .map(|servers| {
                            let errors: Vec<_> = previous_servers
                                .iter()
                                .filter_map(|previous| {
                                    let current =
                                        servers.iter().find(|server| server.id == previous.id)?;
                                    delete_retired_connection_credentials(previous, current)
                                        .err()
                                        .map(|error| {
                                            format!("{}: {error}", previous.display_name())
                                        })
                                })
                                .collect();
                            let warning = (!errors.is_empty()).then(|| match locale {
                                Locale::English => format!(
                                    "Connections saved; retired credential cleanup failed: {}",
                                    errors.join("; ")
                                ),
                                Locale::Chinese => format!(
                                    "连接已保存，但旧系统凭据清理失败：{}",
                                    errors.join("；")
                                ),
                            });
                            ServerMutation { servers, warning }
                        })
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::ConnectionSaved,
        )
    }

    fn save_connection_preferences(&mut self) -> Task<Message> {
        if self.editor_saving {
            return Task::none();
        }
        let Some(draft) = &self.connection_draft else {
            return Task::none();
        };
        let Some(id) = draft.editing_id.clone() else {
            return Task::none();
        };
        let tags = match validated_connection_tags(&draft.tags, self.locale()) {
            Ok(tags) => tags,
            Err(error) => {
                self.status = error;
                return Task::none();
            }
        };
        let auto_mount_at_login =
            draft.auto_mount_at_login && draft.connection_method != ConnectionMethod::Interactive;
        self.editor_saving = true;
        self.status = self.locale().text(TextKey::SavingSettings).into();
        let paths = self.paths.clone();
        let previous_servers = self.servers.clone();
        let previous_settings = self.settings.clone();
        let mut result_settings = previous_settings.clone();
        let startup_lock = self.startup_integration_lock.clone();
        let locale = self.locale();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let updates = if previous_settings.startup_all {
                        previous_servers
                            .iter()
                            .map(|server| {
                                let selected = server.connection_method
                                    != ConnectionMethod::Interactive;
                                if server.id == id {
                                    storage::ServerPreferenceUpdate {
                                        id: id.clone(),
                                        tags: Some(tags.clone()),
                                        auto_mount_at_login: Some(auto_mount_at_login),
                                    }
                                } else {
                                    storage::ServerPreferenceUpdate {
                                        id: server.id.clone(),
                                        tags: None,
                                        auto_mount_at_login: Some(selected),
                                    }
                                }
                            })
                            .collect::<Vec<_>>()
                    } else {
                        vec![storage::ServerPreferenceUpdate {
                            id,
                            tags: Some(tags),
                            auto_mount_at_login: Some(auto_mount_at_login),
                        }]
                    };
                    let servers = storage::update_server_preferences_batch(&paths, &updates)
                        .map_err(|error| error.to_string())?;
                    if previous_settings.startup_all {
                        result_settings.startup_all = false;
                        if let Err(error) = storage::save_settings(&paths, &result_settings) {
                            let mut message = error.to_string();
                            if let Err(rollback) = storage::save_servers(&paths, &previous_servers) {
                                message.push_str(&format!(
                                    "; server rollback failed: {rollback}"
                                ));
                            }
                            return Err(message);
                        }
                    }
                    let warning = reconcile_login_startup(&paths, &startup_lock)
                        .err()
                        .map(|error| match locale {
                            Locale::English => format!(
                                "Preferences were saved, but login startup integration will be retried next launch: {error}"
                            ),
                            Locale::Chinese => format!(
                                "整理设置已保存，但登录自启集成失败，将在下次启动时重试：{error}"
                            ),
                        });
                    Ok(SettingsMutation {
                        settings: result_settings,
                        servers,
                        warning,
                    })
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::ConnectionPreferencesSaved,
        )
    }

    fn save_settings(&mut self) -> Task<Message> {
        if self.editor_saving {
            return Task::none();
        }
        let Some(draft) = &self.settings_draft else {
            return Task::none();
        };
        let mut settings = match draft.build(&self.settings, self.locale()) {
            Ok(settings) => settings,
            Err(error) => {
                self.status = error;
                return Task::none();
            }
        };
        settings.startup_all = false;
        let preference_updates = match connection_preference_updates(draft, self.locale()) {
            Ok(updates) => updates,
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
        let previous_servers = self.servers.clone();
        let service = self.service.clone();
        let locale = self.locale();
        let startup_lock = self.startup_integration_lock.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let storage_changed =
                        settings.credential_storage != previous_settings.credential_storage;
                    if storage_changed {
                        diagnostic_trace(&credential_presence_summary(
                            "before",
                            &previous_servers,
                        ));
                    }
                    let credential_migration = if storage_changed {
                        Some(migrate_servers_for_storage(
                            &paths,
                            &service,
                            &previous_servers,
                            settings.credential_storage,
                        )?)
                    } else {
                        None
                    };
                    let migrated_servers = match storage::update_server_preferences_batch(
                        &paths,
                        &preference_updates,
                    ) {
                        Ok(servers) => servers,
                        Err(error) => {
                            return Err(rollback_credential_storage_change(
                                &paths,
                                &previous_servers,
                                credential_migration.as_ref(),
                                error.to_string(),
                            ));
                        }
                    };
                    if storage_changed {
                        diagnostic_trace(&credential_presence_summary("after", &migrated_servers));
                    }
                    if storage_changed && settings.credential_storage == CredentialStorage::System {
                        for server in &migrated_servers {
                            if let Err(error) =
                                clear_rclone_remote_secrets(&paths, server.remote_name())
                            {
                                return Err(rollback_credential_storage_change(
                                    &paths,
                                    &previous_servers,
                                    credential_migration.as_ref(),
                                    error.to_string(),
                                ));
                            }
                        }
                    }
                    if let Err(error) = storage::save_settings(&paths, &settings) {
                        let mut message = rollback_credential_storage_change(
                            &paths,
                            &previous_servers,
                            credential_migration.as_ref(),
                            error.to_string(),
                        );
                        if credential_migration.is_none() {
                            let rollback_updates = previous_servers
                                .iter()
                                .map(|server| storage::ServerPreferenceUpdate {
                                    id: server.id.clone(),
                                    tags: None,
                                    auto_mount_at_login: Some(server.auto_mount_at_login),
                                })
                                .collect::<Vec<_>>();
                            if let Err(rollback) = storage::update_server_preferences_batch(
                                &paths,
                                &rollback_updates,
                            ) {
                                message.push_str(&format!(
                                    "; login-startup rollback failed: {rollback}"
                                ));
                            }
                        }
                        return Err(message);
                    }
                    let mut warnings = Vec::new();
                    if let Err(error) = reconcile_login_startup(&paths, &startup_lock) {
                        warnings.push(match locale {
                            Locale::English => format!(
                                "settings were saved, but login startup integration will be retried next launch: {error}"
                            ),
                            Locale::Chinese => format!(
                                "设置已保存，但登录自启集成失败，将在下次启动时重试：{error}"
                            ),
                        });
                    }
                    if storage_changed
                        && settings.credential_storage == CredentialStorage::Obscure
                        && let Some(error) = credential_migration
                            .as_ref()
                            .and_then(|migration| migration.retire_system_references().err())
                    {
                        warnings.push(match locale {
                            Locale::English => format!(
                                "some retired vault entries could not be removed: {error}"
                            ),
                            Locale::Chinese => {
                                format!("部分旧系统凭据无法删除：{error}")
                            }
                        });
                    }
                    let warning = (!warnings.is_empty()).then(|| warnings.join("; "));
                    Ok(SettingsMutation {
                        settings: result_settings,
                        servers: migrated_servers,
                        warning,
                    })
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
        let paths = self.paths.clone();
        let startup_lock = self.startup_integration_lock.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || reconcile_login_startup(&paths, &startup_lock))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::StartupReconciled,
        )
    }

    fn dependency_check_task(&self) -> Task<Message> {
        let paths = self.paths.clone();
        let app_root = application_root();
        let mount_backend = self
            .settings_draft
            .as_ref()
            .map_or(self.settings.macos_mount_backend, |draft| {
                draft.mount_backend
            });
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    check_dependencies(&paths, &app_root, mount_backend)
                        .map_err(|error| error.to_string())
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
            self.status = self
                .update_info
                .as_ref()
                .and_then(|info| info.trust_error.as_ref())
                .map_or_else(
                    || match self.locale() {
                        Locale::English => {
                            "This release has no verified asset for this platform".into()
                        }
                        Locale::Chinese => "此版本没有适用于当前平台的已验证安装包".into(),
                    },
                    |error| automatic_install_blocked_message(self.locale(), error),
                );
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

    fn status_task(&mut self, policy: StatusPublishPolicy) -> Task<Message> {
        let (generation, expected_status) = match policy {
            StatusPublishPolicy::Silent => (None, None),
            StatusPublishPolicy::Initial | StatusPublishPolicy::UserRefresh => {
                let generation = self.claim_status_generation();
                (Some(generation), Some(self.status.clone()))
            }
        };
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
            move |results| Message::StatusesLoaded {
                policy,
                generation,
                expected_status,
                results,
            },
        )
    }

    fn save_batch_tag_updates(
        &mut self,
        updates: Vec<storage::ServerPreferenceUpdate>,
    ) -> Task<Message> {
        if updates.is_empty() || self.connection_list_saving || self.editor_saving {
            return Task::none();
        }
        self.connection_list_saving = true;
        self.status = match self.locale() {
            Locale::English => "Saving connection tags...".into(),
            Locale::Chinese => "正在保存连接标签…".into(),
        };
        let paths = self.paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    storage::update_server_preferences_batch(&paths, &updates)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::BatchTagsSaved,
        )
    }

    fn claim_status_generation(&mut self) -> u64 {
        self.status_generation = self.status_generation.wrapping_add(1);
        self.status_generation
    }

    fn invalidate_status_publications(&mut self) {
        self.claim_status_generation();
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
        let failed_ids = servers
            .iter()
            .map(|server| server.id.clone())
            .collect::<Vec<_>>();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    std::thread::scope(|scope| {
                        let tasks: Vec<_> = servers
                            .into_iter()
                            .map(|server| {
                                let service = service.clone();
                                let id = server.id.clone();
                                let task_id = id.clone();
                                let task = scope.spawn(move || {
                                    service.capacity(&server).map_err(|error| error.to_string())
                                });
                                (task_id, task)
                            })
                            .collect();
                        tasks
                            .into_iter()
                            .map(|(id, task)| match task.join() {
                                Ok(result) => (id, result),
                                Err(_) => (id, Err("capacity worker panicked".into())),
                            })
                            .collect()
                    })
                })
                .await
                .unwrap_or_else(|error| {
                    failed_ids
                        .into_iter()
                        .map(|id| (id, Err(error.to_string())))
                        .collect()
                })
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
        let failed_ids = ids.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    std::thread::scope(|scope| {
                        let tasks: Vec<_> = ids
                            .into_iter()
                            .map(|id| {
                                let service = service.clone();
                                let task_id = id.clone();
                                let task = scope.spawn(move || {
                                    service
                                        .transfer_snapshot(&id)
                                        .map_err(|error| error.to_string())
                                });
                                (task_id, task)
                            })
                            .collect();
                        tasks
                            .into_iter()
                            .map(|(id, task)| match task.join() {
                                Ok(result) => (id, result),
                                Err(_) => (id, Err("transfer worker panicked".into())),
                            })
                            .collect()
                    })
                })
                .await
                .unwrap_or_else(|error| {
                    failed_ids
                        .into_iter()
                        .map(|id| (id, Err(error.to_string())))
                        .collect()
                })
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

    fn interactive_session_ready(&self, id: &str) -> bool {
        self.interactive_terminals
            .get(id)
            .is_some_and(|session| session.lifecycle == InteractiveTerminalLifecycle::Ready)
    }

    fn effective_font_scale(&self) -> FontScale {
        self.settings_draft
            .as_ref()
            .map_or(self.settings.font_scale, |draft| draft.font_scale)
    }

    fn queue_interactive_mount(&mut self, id: String, server: ServerConfig) -> Task<Message> {
        if self.interactive_terminals.get(&id).is_some_and(|session| {
            matches!(
                session.lifecycle,
                InteractiveTerminalLifecycle::Ready
                    | InteractiveTerminalLifecycle::Exited
                    | InteractiveTerminalLifecycle::Failed
            )
        }) {
            self.interactive_terminals.remove(&id);
            return self.open_interactive_terminal(id, server);
        }
        if !self.interactive_terminals.contains_key(&id) {
            self.open_interactive_terminal(id.clone(), server)
        } else {
            if let Some(session) = self.interactive_terminals.get_mut(&id) {
                session.queued_mount = true;
                session.resume_sent = false;
            }
            self.terminal_error = None;
            self.status = self
                .locale()
                .text(TextKey::InteractiveTerminalStarting)
                .into();
            self.open_terminal_window(id)
        }
    }

    fn open_interactive_terminal(&mut self, id: String, server: ServerConfig) -> Task<Message> {
        self.terminal_error = None;
        let generation = self.next_terminal_generation;
        self.next_terminal_generation = self.next_terminal_generation.saturating_add(1);
        let result: Result<(InteractiveSshSession, iced_term::Terminal), String> =
            InteractiveSshSession::for_server(&self.paths, &application_root(), &server)
                .map_err(|error| error.to_string())
                .and_then(|ssh| {
                    let (program, args) = strict_terminal_command(ssh.login_command())?;
                    let terminal = iced_term::Terminal::new(
                        generation,
                        iced_term::settings::Settings {
                            backend: iced_term::settings::BackendSettings {
                                program,
                                args,
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    )
                    .map_err(|error| {
                        format!("could not start interactive SSH terminal: {error}")
                    })?;
                    Ok((ssh, terminal))
                });
        match result {
            Ok((ssh, terminal)) => {
                diagnostic_trace(&format!(
                    "interactive terminal session created for {id} generation {generation}"
                ));
                self.interactive_terminals.insert(
                    id.clone(),
                    InteractiveTerminalSession {
                        generation,
                        ssh,
                        terminal,
                        lifecycle: InteractiveTerminalLifecycle::Starting,
                        queued_mount: true,
                        resume_sent: false,
                        readiness_check_in_flight: false,
                    },
                );
                self.status = self
                    .locale()
                    .text(TextKey::InteractiveTerminalStarting)
                    .into();
            }
            Err(error) => {
                diagnostic_trace(&format!(
                    "interactive terminal session creation failed for {id} generation {generation}"
                ));
                self.terminal_error = Some((id.clone(), error));
                self.status = self
                    .locale()
                    .text(TextKey::InteractiveTerminalFailed)
                    .into();
            }
        }
        self.open_terminal_window(id)
    }

    fn open_terminal_window(&mut self, id: String) -> Task<Message> {
        self.terminal_server_id = Some(id.clone());
        if let Some(window) = self.terminal_window {
            diagnostic_trace(&format!(
                "focusing interactive terminal window {window:?} for {id}"
            ));
            return window::gain_focus(window);
        }
        if !self.main_window_ready {
            diagnostic_trace(&format!(
                "deferring interactive terminal window for {id} until main window is ready"
            ));
            return Task::none();
        }
        let (window, open) = window::open(terminal_window_settings());
        diagnostic_trace(&format!(
            "opening interactive terminal window {window:?} for {id}"
        ));
        self.terminal_window = Some(window);
        open.map(Message::TerminalWindowOpened)
    }

    fn poll_interactive_terminals(&mut self) -> Task<Message> {
        let saving_id = self
            .editor_saving
            .then(|| {
                self.connection_draft
                    .as_ref()
                    .and_then(|draft| draft.editing_id.clone())
            })
            .flatten();
        let mut checks = Vec::new();
        for (id, session) in &mut self.interactive_terminals {
            if !interactive_mount_poll_eligible(id, session.queued_mount, saving_id.as_deref())
                || session.readiness_check_in_flight
                || session.lifecycle != InteractiveTerminalLifecycle::Starting
            {
                continue;
            }
            session.readiness_check_in_flight = true;
            let id = id.clone();
            let generation = session.generation;
            let ssh = session.ssh.clone();
            checks.push(Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        ssh.check_ready(Duration::from_secs(2))
                            .map_err(|error| error.to_string())
                    })
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
                },
                move |result| Message::InteractiveReadinessChecked {
                    id: id.clone(),
                    generation,
                    result,
                },
            ));
        }
        Task::batch(checks)
    }

    fn handle_terminal_event(&mut self, event: RedactedTerminalEvent) -> Task<Message> {
        let iced_term::Event::BackendCall(generation, command) = event.0;
        match &command {
            iced_term::BackendCommand::ProcessAlacrittyEvent(
                iced_term::AlacrittyEvent::ChildExit(code),
            ) => diagnostic_trace(&format!(
                "interactive terminal child exited for generation {generation} with code {code}"
            )),
            iced_term::BackendCommand::ProcessAlacrittyEvent(iced_term::AlacrittyEvent::Exit) => {
                diagnostic_trace(&format!(
                    "interactive terminal backend exited for generation {generation}"
                ));
            }
            _ => {}
        }
        let Some(session) = self
            .interactive_terminals
            .values_mut()
            .find(|session| session.generation == generation)
        else {
            return Task::none();
        };
        let action = session
            .terminal
            .handle(iced_term::Command::ProxyToBackend(command));
        if action == iced_term::actions::Action::Shutdown {
            session.lifecycle = InteractiveTerminalLifecycle::Exited;
            session.queued_mount = false;
            session.resume_sent = true;
            self.status = self
                .locale()
                .text(TextKey::InteractiveTerminalExited)
                .into();
        }
        Task::none()
    }

    fn end_interactive_session(&mut self) -> Task<Message> {
        let Some(id) = self.terminal_server_id.clone() else {
            return Task::none();
        };
        if !interactive_session_can_restart_or_end(self.mount_statuses.get(&id).copied()) {
            self.status = match self.locale() {
                Locale::English => {
                    "Unmount this connection before ending its shared SSH session".into()
                }
                Locale::Chinese => "请先卸载此连接，再结束共享 SSH 会话".into(),
            };
            return Task::none();
        }
        let window = self.terminal_window.take();
        self.terminal_server_id = None;
        self.interactive_terminals.remove(&id);
        self.terminal_error = None;
        if let Some(window) = window {
            window::close(window)
        } else {
            Task::none()
        }
    }

    fn retry_interactive_terminal(&mut self) -> Task<Message> {
        let Some(id) = self.terminal_server_id.clone() else {
            return Task::none();
        };
        let Some(server) = self.servers.iter().find(|server| server.id == id).cloned() else {
            return self.end_interactive_session();
        };
        if !interactive_session_can_restart_or_end(self.mount_statuses.get(&id).copied()) {
            self.status = match self.locale() {
                Locale::English => {
                    "Unmount this connection before restarting its shared SSH session".into()
                }
                Locale::Chinese => "请先卸载此连接，再重试共享 SSH 会话".into(),
            };
            return Task::none();
        }
        self.interactive_terminals.remove(&id);
        self.open_interactive_terminal(id, server)
    }

    fn reconcile_interactive_sessions(&mut self, next_servers: &[ServerConfig]) -> Task<Message> {
        let compatible = self
            .servers
            .iter()
            .filter_map(|previous| {
                next_servers
                    .iter()
                    .find(|next| next.id == previous.id)
                    .filter(|next| interactive_session_config_compatible(previous, next))
                    .map(|next| next.id.clone())
            })
            .collect::<HashSet<_>>();
        self.interactive_terminals
            .retain(|id, _| compatible.contains(id));

        let visible_is_compatible = self
            .terminal_server_id
            .as_deref()
            .is_none_or(|id| compatible.contains(id));
        if visible_is_compatible {
            return Task::none();
        }

        self.terminal_server_id = None;
        self.terminal_error = None;
        self.terminal_window
            .take()
            .map_or_else(Task::none, window::close)
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
                self.synced_polls.get(&id).copied().unwrap_or(0),
            )
        {
            return self.confirm_waiting_unmount(vec![id]);
        }
        let server = self.servers.iter().find(|server| server.id == id).cloned();
        if operation == MountOperation::Mount
            && server
                .as_ref()
                .is_some_and(|server| server.connection_method == ConnectionMethod::Interactive)
            && !self.interactive_session_ready(&id)
        {
            let Some(server) = server else {
                self.status = self.locale().text(TextKey::ConnectionGone).into();
                return Task::none();
            };
            return self.queue_interactive_mount(id, server);
        }
        if operation == MountOperation::Mount
            && server
                .as_ref()
                .is_some_and(|server| server.connection_method == ConnectionMethod::Interactive)
            && let Some(session) = self.interactive_terminals.get_mut(&id)
        {
            session.lifecycle = InteractiveTerminalLifecycle::Ready;
            session.queued_mount = false;
            session.resume_sent = true;
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
        self.operation_errors.remove(&id);
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
                            .map_err(|error| localize_service_error(locale, &error))
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
            existed: false,
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
        } else if self.terminal_window == Some(window) {
            self.interactive_terminal_view()
        } else {
            self.transfer_popup_view(window)
        }
    }

    fn main_view(&self) -> Element<'_, Message> {
        let locale = self.locale();
        let reordering = self.connection_list_mode == ConnectionListMode::Reorder;
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
        let toolbar = responsive(move |size| {
            let actions = row![
                button(locale.text(TextKey::Refresh))
                    .on_press_maybe((!reordering).then_some(Message::Refresh)),
                button(text(transfers_label.clone()))
                    .on_press_maybe((!reordering).then_some(Message::OpenTransfers)),
                button(locale.text(TextKey::AddConnection)).on_press_maybe(
                    (!reordering && !self.connection_list_saving).then_some(Message::AddConnection),
                ),
                button(locale.text(TextKey::Settings)).on_press_maybe(
                    (!reordering && !self.connection_list_saving).then_some(Message::OpenSettings),
                ),
            ]
            .spacing(10);
            if size.width < 760.0 {
                column![text(APP_NAME).size(28), actions].spacing(10).into()
            } else {
                row![text(APP_NAME).size(28), Space::new().width(Fill), actions]
                    .align_y(Center)
                    .into()
            }
        })
        .height(Length::Shrink);

        let selected_count = self.selected_connections.len();
        let selected_any = selected_count > 0;
        let visible_ids = visible_connections(
            &self.servers,
            &self.connection_search,
            self.connection_tag_filter.as_deref(),
            self.connection_sort,
        )
        .into_iter()
        .map(|server| server.id.as_str())
        .collect::<Vec<_>>();
        let all_selected = !visible_ids.is_empty()
            && visible_ids
                .iter()
                .all(|id| self.selected_connections.contains(*id));
        let selected_can_delete = selected_any
            && self
                .selected_connections
                .iter()
                .all(|id| self.can_modify(id));
        let mode_actions: Element<'_, Message> = match self.connection_list_mode {
            ConnectionListMode::Browse => row![
                button(match locale {
                    Locale::English => "Batch actions",
                    Locale::Chinese => "批量操作",
                })
                .on_press(Message::ConnectionListModeChanged(
                    ConnectionListMode::Batch
                )),
                button(match locale {
                    Locale::English => "Manage tags",
                    Locale::Chinese => "标签管理",
                })
                .on_press(Message::ConnectionListModeChanged(ConnectionListMode::Tags)),
            ]
            .spacing(10)
            .into(),
            ConnectionListMode::Batch => responsive(move |size| {
                let selection = row![
                    checkbox(all_selected)
                        .label(match locale {
                            Locale::English => "Select all",
                            Locale::Chinese => "全选",
                        })
                        .on_toggle(Message::BatchSelectAllChanged),
                    text(match locale {
                        Locale::English => format!("{selected_count} selected"),
                        Locale::Chinese => format!("已选择 {selected_count} 个"),
                    }),
                    button(locale.text(TextKey::Mount)).on_press_maybe(
                        (selected_any && !self.connection_list_saving)
                            .then_some(Message::BatchMountSelected)
                    ),
                    button(locale.text(TextKey::Unmount)).on_press_maybe(
                        (selected_any && !self.connection_list_saving)
                            .then_some(Message::BatchUnmountSelected)
                    ),
                ]
                .spacing(10)
                .align_y(Center);
                let existing_tags = connection_tags(&self.servers);
                let tagging = row![
                    pick_list(
                        existing_tags,
                        self.batch_existing_tag.clone(),
                        Message::BatchExistingTagChanged,
                    )
                    .placeholder(match locale {
                        Locale::English => "Existing tag",
                        Locale::Chinese => "选择已有标签",
                    })
                    .width(Length::Fixed(160.0)),
                    text_input(
                        match locale {
                            Locale::English => "Or create a tag",
                            Locale::Chinese => "或新建标签",
                        },
                        &self.batch_tag_input,
                    )
                    .on_input(Message::BatchTagInputChanged)
                    .width(Length::Fixed(190.0)),
                    button(match locale {
                        Locale::English => "Add tag",
                        Locale::Chinese => "添加标签",
                    })
                    .on_press_maybe(
                        (selected_any
                            && !self.connection_list_saving
                            && (self.batch_existing_tag.is_some()
                                || !self.batch_tag_input.trim().is_empty()))
                        .then_some(Message::BatchAddTag),
                    ),
                ]
                .spacing(8)
                .align_y(Center);
                let completion = row![
                    button(match locale {
                        Locale::English => "Delete connections",
                        Locale::Chinese => "删除连接",
                    })
                    .on_press_maybe(
                        (selected_can_delete && !self.connection_list_saving)
                            .then_some(Message::BatchDeleteSelected),
                    ),
                    button(match locale {
                        Locale::English => "Done",
                        Locale::Chinese => "完成",
                    })
                    .on_press_maybe((!self.connection_list_saving).then_some(
                        Message::ConnectionListModeChanged(ConnectionListMode::Browse),
                    )),
                ]
                .spacing(10)
                .align_y(Center);
                if size.width < 860.0 {
                    column![selection, tagging, completion].spacing(10).into()
                } else {
                    row![
                        column![selection, tagging].spacing(8),
                        Space::new().width(Fill),
                        completion
                    ]
                    .spacing(10)
                    .align_y(Center)
                    .into()
                }
            })
            .height(Length::Shrink)
            .into(),
            ConnectionListMode::Tags => responsive(move |size| {
                let completion = button(match locale {
                    Locale::English => "Done",
                    Locale::Chinese => "完成",
                })
                .on_press_maybe((!self.connection_list_saving).then_some(
                    Message::ConnectionListModeChanged(ConnectionListMode::Browse),
                ));
                let hint = text(match locale {
                    Locale::English => "Remove a tag everywhere with its x button below.",
                    Locale::Chinese => "点击下方标签右侧的 x 可从所有连接中移除该标签。",
                })
                .size(13);
                if size.width < 760.0 {
                    column![hint, completion].spacing(10).into()
                } else {
                    row![hint, Space::new().width(Fill), completion]
                        .spacing(10)
                        .align_y(Center)
                        .into()
                }
            })
            .height(Length::Shrink)
            .into(),
            ConnectionListMode::Reorder => row![
                text(match locale {
                    Locale::English => "Adjust saved order",
                    Locale::Chinese => "调整保存顺序",
                }),
                Space::new().width(Fill),
                button(match locale {
                    Locale::English => "Cancel",
                    Locale::Chinese => "取消",
                })
                .on_press_maybe(
                    (!self.connection_list_saving).then_some(Message::CancelConnectionOrder)
                ),
                button(match locale {
                    Locale::English => "Save order",
                    Locale::Chinese => "保存排序",
                })
                .on_press_maybe(
                    (!self.connection_list_saving).then_some(Message::SaveConnectionOrder)
                ),
            ]
            .spacing(10)
            .align_y(Center)
            .into(),
        };

        let mut sort_options = ConnectionSort::ALL
            .into_iter()
            .map(|value| {
                locale.choice(
                    ConnectionSortMenuAction::Sort(value),
                    locale.connection_sort(value),
                )
            })
            .collect::<Vec<_>>();
        sort_options.push(locale.choice(
            ConnectionSortMenuAction::AdjustSavedOrder,
            match locale {
                Locale::English => "⚙ Adjust saved order",
                Locale::Chinese => "⚙ 调整保存顺序",
            },
        ));
        let organization = responsive(move |size| {
            let search: Element<'_, Message> =
                if self.connection_list_mode == ConnectionListMode::Reorder {
                    text(match locale {
                        Locale::English => "All connections",
                        Locale::Chinese => "全部连接",
                    })
                    .into()
                } else {
                    text_input(
                        locale.text(TextKey::SearchConnections),
                        &self.connection_search,
                    )
                    .on_input(Message::ConnectionSearchChanged)
                    .width(Fill)
                    .into()
                };
            let sort: Element<'_, Message> =
                if self.connection_list_mode == ConnectionListMode::Reorder {
                    text(locale.connection_sort(ConnectionSort::SavedOrder)).into()
                } else {
                    pick_list(
                        sort_options.clone(),
                        Some(locale.choice(
                            ConnectionSortMenuAction::Sort(self.connection_sort),
                            locale.connection_sort(self.connection_sort),
                        )),
                        |choice| Message::ConnectionSortMenuChanged(choice.value),
                    )
                    .placeholder(locale.text(TextKey::SortConnections))
                    .into()
                };
            if size.width < 620.0 {
                column![search, container(sort).width(Fill)]
                    .spacing(10)
                    .into()
            } else {
                row![search, container(sort).width(Length::Fixed(230.0))]
                    .spacing(10)
                    .align_y(Center)
                    .into()
            }
        })
        .height(Length::Shrink);

        let mut tag_controls = row![
            button(match locale {
                Locale::English => "All",
                Locale::Chinese => "全部",
            })
            .style(if self.connection_tag_filter.is_none() {
                button::secondary
            } else {
                button::text
            })
            .on_press(Message::ConnectionTagFilterChanged(None)),
        ]
        .spacing(8)
        .align_y(Center);
        for tag in connection_tags(&self.servers) {
            if self.connection_list_mode == ConnectionListMode::Batch {
                let tag_selected = self
                    .servers
                    .iter()
                    .filter(|server| server.tags.iter().any(|candidate| candidate == &tag))
                    .all(|server| self.selected_connections.contains(&server.id));
                let select_tag = tag.clone();
                tag_controls =
                    tag_controls.push(checkbox(tag_selected).on_toggle(move |selected| {
                        Message::BatchTagSelectionChanged(select_tag.clone(), selected)
                    }));
            }
            let filter_tag = tag.clone();
            let filter_selected = self.connection_tag_filter.as_deref() == Some(tag.as_str());
            tag_controls = tag_controls.push(
                button(text(tag.clone()).size(12))
                    .style(if filter_selected {
                        button::secondary
                    } else {
                        button::text
                    })
                    .on_press(Message::ConnectionTagFilterChanged(Some(filter_tag))),
            );
            if self.connection_list_mode == ConnectionListMode::Tags {
                tag_controls = tag_controls.push(
                    button(text("×").size(12))
                        .padding([2, 6])
                        .style(button::secondary)
                        .on_press_maybe(
                            (!self.connection_list_saving)
                                .then_some(Message::RemoveTagEverywhere(tag.clone())),
                        ),
                );
            }
        }
        let tags: Element<'_, Message> = if reordering {
            Space::new().height(Length::Shrink).into()
        } else {
            scrollable(tag_controls)
                .direction(iced::widget::scrollable::Direction::Horizontal(
                    iced::widget::scrollable::Scrollbar::new(),
                ))
                .height(Length::Shrink)
                .into()
        };

        let mut connections = column![].spacing(8);
        if self.servers.is_empty() {
            connections = connections.push(
                container(
                    column![
                        text(locale.text(TextKey::NoSavedConnections)).size(20),
                        text(match locale {
                            Locale::English =>
                                "Create a connection to mount your first remote folder.",
                            Locale::Chinese => "新建连接以挂载第一个远程目录。",
                        })
                        .size(14),
                        button(locale.text(TextKey::AddConnection)).on_press_maybe(
                            (!reordering && !self.connection_list_saving)
                                .then_some(Message::AddConnection),
                        ),
                    ]
                    .spacing(12)
                    .align_x(Center),
                )
                .padding(28)
                .width(Fill)
                .center_x(Fill),
            );
        } else {
            let visible = visible_connections(
                &self.servers,
                &self.connection_search,
                self.connection_tag_filter.as_deref(),
                self.connection_sort,
            );
            if visible.is_empty() {
                connections = connections.push(
                    container(text(locale.text(TextKey::NoMatchingConnections)).size(16))
                        .padding(24)
                        .width(Fill)
                        .center_x(Fill),
                );
            }
            for server in visible {
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
                        operation_error: self.operation_errors.get(&server.id),
                        has_interactive_terminal: self
                            .interactive_terminals
                            .contains_key(&server.id),
                        auto_mount_at_login: (self.settings.startup_all
                            || server.auto_mount_at_login)
                            && server.connection_method != ConnectionMethod::Interactive,
                        login_startup_available: server.connection_method
                            != ConnectionMethod::Interactive,
                        list_mode: self.connection_list_mode,
                        selected: self.selected_connections.contains(&server.id),
                        can_move_up: can_move_connection(&self.servers, &server.id, -1),
                        can_move_down: can_move_connection(&self.servers, &server.id, 1),
                        connection_list_saving: self.connection_list_saving,
                    },
                    locale,
                ));
            }
        }

        container(
            column![
                toolbar,
                mode_actions,
                organization,
                tags,
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
        if draft
            .editing_id
            .as_deref()
            .is_some_and(|id| !self.can_modify(id))
        {
            return self.read_only_connection_settings_view(draft, title);
        }
        let requirements = draft.requirements();
        let mountpoint_allows_save = mountpoint_choice(&draft.mountpoint) != "custom"
            || self.mountpoint_preflight.allows_save();
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
            .on_press_maybe(
                (!self.editor_saving && mountpoint_allows_save).then_some(Message::SaveConnection),
            ),
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
                        requirements.ssh_config_path,
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
                ConnectionField::Name,
                requirements.name,
            ),
            connection_input(
                locale.text(TextKey::SshHostAlias),
                &draft.host_alias,
                ConnectionField::HostAlias,
                requirements.host_alias,
            ),
        ]
        .spacing(12);
        let ssh_config_authoritative = draft.source == ConnectionSource::SshConfig
            && draft.connection_method != ConnectionMethod::Native;
        let target = if ssh_config_authoritative {
            row![
                connection_read_only_field(locale.text(TextKey::IpHost), &draft.host),
                connection_read_only_field(locale.text(TextKey::User), &draft.user),
                connection_read_only_field(locale.text(TextKey::Port), &draft.port)
                    .width(Length::Fixed(150.0)),
            ]
            .spacing(12)
        } else {
            row![
                connection_input(
                    locale.text(TextKey::IpHost),
                    &draft.host,
                    ConnectionField::Host,
                    requirements.host,
                ),
                connection_input(
                    locale.text(TextKey::User),
                    &draft.user,
                    ConnectionField::User,
                    requirements.user,
                ),
                connection_input(
                    locale.text(TextKey::Port),
                    &draft.port,
                    ConnectionField::Port,
                    requirements.port,
                )
                .width(Length::Fixed(150.0)),
            ]
            .spacing(12)
        };
        let authentication: Element<'_, Message> =
            if draft.connection_method != ConnectionMethod::Native {
                let label = if draft.connection_method == ConnectionMethod::Interactive {
                    interactive_auth_help(locale)
                } else {
                    locale.text(TextKey::ManagedByOpenSsh)
                };
                container(text(label)).padding(10).width(Fill).into()
            } else {
                pick_list(
                    localized_choices(AuthMethod::ALL, locale, Locale::auth_method),
                    Some(locale.choice(draft.auth, locale.auth_method(draft.auth))),
                    |auth| Message::ConnectionAuthChanged(auth.value),
                )
                .width(Fill)
                .into()
            };
        let transport_choice = column![
            pick_list(
                localized_choices(ConnectionMethod::ALL, locale, Locale::connection_method),
                Some(locale.choice(
                    draft.connection_method,
                    locale.connection_method(draft.connection_method),
                )),
                |method| Message::ConnectionMethodChanged(method.value),
            )
            .width(Fill)
        ]
        .spacing(5);
        let transport = row![
            labeled_control(locale.text(TextKey::Transport), transport_choice,),
            labeled_control(locale.text(TextKey::Authentication), authentication),
        ]
        .spacing(12);

        if ssh_config_authoritative {
            ssh_config_controls = ssh_config_controls.push(
                container(
                    column![
                        text(match locale {
                            Locale::English => "OpenSSH source of truth",
                            Locale::Chinese => "OpenSSH 权威来源",
                        })
                        .size(16),
                        text(format!(
                            "{}: {}",
                            locale.text(TextKey::SshConfigFile),
                            draft.ssh_config_path
                        ))
                        .size(13),
                        text(format!(
                            "{}: {}",
                            locale.text(TextKey::SshHostAlias),
                            draft.host_alias
                        ))
                        .size(13),
                        text(openssh_command_preview(
                            &draft.ssh_config_path,
                            &draft.host_alias
                        ))
                        .size(13),
                        text(match locale {
                            Locale::English => "The visible resolved fields are only an import snapshot. The actual OpenSSH command remains authoritative and may apply Include, Match, ProxyJump, ProxyCommand, agent, certificate, and token expansion rules not shown here.",
                            Locale::Chinese => "可见的解析字段只是导入快照。实际 OpenSSH 命令仍是权威来源，并可能应用此处未显示的 Include、Match、ProxyJump、ProxyCommand、代理、证书和令牌展开规则。",
                        })
                        .size(12),
                    ]
                    .spacing(5),
                )
                .padding(10)
                .width(Fill)
                .style(container::rounded_box),
            );
        }

        let mut auth_fields = column![].spacing(12);
        if draft.connection_method == ConnectionMethod::Native {
            match draft.auth {
                AuthMethod::Password => {
                    auth_fields = auth_fields.push(secret_input_control(
                        locale.text(TextKey::Password),
                        locale.text(TextKey::PasswordRequired),
                        &draft.password,
                        draft.preserved_secret_state(CredentialKind::Password),
                        CredentialKind::Password,
                        locale,
                        requirements.password,
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
                                requirements.key_file,
                            ),
                            secret_input_control(
                                locale.text(TextKey::KeyPassphrase),
                                locale.text(TextKey::Optional),
                                &draft.key_passphrase,
                                draft.preserved_secret_state(CredentialKind::KeyPassphrase),
                                CredentialKind::KeyPassphrase,
                                locale,
                                false,
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
                connection_field_label(locale.text(TextKey::Mountpoint), custom_mountpoint),
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
            let preflight_text = match &self.mountpoint_preflight {
                MountpointPreflight::Checking(_) => Some(
                    match locale {
                        Locale::English => "Checking mountpoint availability...",
                        Locale::Chinese => "正在检查挂载点可用性……",
                    }
                    .to_owned(),
                ),
                MountpointPreflight::Valid(_) => Some(
                    match locale {
                        Locale::English => "Mountpoint is available",
                        Locale::Chinese => "挂载点可用",
                    }
                    .to_owned(),
                ),
                MountpointPreflight::Invalid { error, .. } => Some(match locale {
                    Locale::English => format!("Mountpoint unavailable: {error}"),
                    Locale::Chinese => format!("挂载点不可用：{error}"),
                }),
                MountpointPreflight::NotRequired => None,
            };
            if let Some(message) = preflight_text {
                mountpoint = mountpoint.push(text(message).size(13));
            }
        }
        let paths = row![remote_path, mountpoint].spacing(12);
        let startup_control: Element<'_, Message> =
            if draft.connection_method == ConnectionMethod::Interactive {
                container(text(match locale {
                    Locale::English => "Unavailable for interactive SSH",
                    Locale::Chinese => "交互式 SSH 不支持开机自动挂载",
                }))
                .padding(10)
                .width(Fill)
                .into()
            } else {
                toggler(draft.auto_mount_at_login)
                    .label(match locale {
                        Locale::English => "Mount at login",
                        Locale::Chinese => "登录时自动挂载",
                    })
                    .on_toggle(Message::ConnectionStartupChanged)
                    .into()
            };
        let organization = row![
            labeled_control(
                match locale {
                    Locale::English => "Tags",
                    Locale::Chinese => "标签",
                },
                text_input(
                    match locale {
                        Locale::English => "Comma-separated tags",
                        Locale::Chinese => "用逗号分隔多个标签",
                    },
                    &self.connection_tags_input,
                )
                .on_input(Message::ConnectionTagsChanged)
                .width(Fill),
            ),
            labeled_control(
                match locale {
                    Locale::English => "Login startup",
                    Locale::Chinese => "登录自启",
                },
                startup_control,
            ),
        ]
        .spacing(12);
        let content = column![
            source,
            ssh_config_controls,
            identity,
            target,
            transport,
            auth_fields,
            managed_fields,
            organization,
            paths
        ]
        .spacing(16)
        .max_width(900);
        editor_shell(header, scrollable(content), &self.status)
    }

    fn read_only_connection_settings_view<'a>(
        &'a self,
        draft: &'a ConnectionDraft,
        title: &'a str,
    ) -> Element<'a, Message> {
        let locale = self.locale();
        let header = row![
            text(title).size(28),
            Space::new().width(Fill),
            button(locale.text(TextKey::Cancel)).on_press(Message::CancelEditor),
            button(if self.editor_saving {
                locale.text(TextKey::Saving)
            } else {
                locale.text(TextKey::Save)
            })
            .on_press_maybe((!self.editor_saving).then_some(Message::SaveConnectionPreferences)),
        ]
        .spacing(10)
        .align_y(Center);
        let mut content = column![
            container(text(match locale {
                Locale::English => "The connection is mounted or busy. Connection, authentication, and mount fields remain read-only; tags and login startup can still be changed.",
                Locale::Chinese => "此连接已挂载或正在执行操作。连接、认证和挂载字段保持只读；仍可修改标签与登录自启。",
            }).size(14))
                .padding(12)
                .width(Fill)
                .style(container::rounded_box),
            row![
                labeled_control(
                    match locale {
                        Locale::English => "Tags",
                        Locale::Chinese => "标签",
                    },
                    text_input(
                        match locale {
                            Locale::English => "Comma-separated tags",
                            Locale::Chinese => "用逗号分隔多个标签",
                        },
                        &self.connection_tags_input,
                    )
                    .on_input(Message::ConnectionTagsChanged)
                    .width(Fill),
                ),
                if draft.connection_method == ConnectionMethod::Interactive {
                    labeled_control(
                        match locale {
                            Locale::English => "Login startup",
                            Locale::Chinese => "登录自启",
                        },
                        container(text(match locale {
                            Locale::English => "Unavailable for interactive SSH",
                            Locale::Chinese => "交互式 SSH 不支持",
                        }))
                        .padding(10)
                        .width(Fill),
                    )
                } else {
                    labeled_control(
                        match locale {
                            Locale::English => "Login startup",
                            Locale::Chinese => "登录自启",
                        },
                        toggler(draft.auto_mount_at_login)
                            .label(match locale {
                                Locale::English => "Mount at login",
                                Locale::Chinese => "登录时自动挂载",
                            })
                            .on_toggle(Message::ConnectionStartupChanged),
                    )
                },
            ]
            .spacing(12),
            row![
                connection_read_only_field(
                    locale.text(TextKey::Source),
                    locale.connection_source(draft.source),
                ),
                connection_read_only_field(locale.text(TextKey::Name), &draft.name),
            ]
            .spacing(12),
            row![
                connection_read_only_field(locale.text(TextKey::SshHostAlias), &draft.host_alias,),
                connection_read_only_field(locale.text(TextKey::IpHost), &draft.host),
            ]
            .spacing(12),
            row![
                connection_read_only_field(locale.text(TextKey::User), &draft.user),
                connection_read_only_field(locale.text(TextKey::Port), &draft.port),
            ]
            .spacing(12),
            row![
                connection_read_only_field(
                    locale.text(TextKey::Transport),
                    locale.connection_method(draft.connection_method),
                ),
                connection_read_only_field(
                    locale.text(TextKey::Authentication),
                    locale.auth_method(draft.auth),
                ),
            ]
            .spacing(12),
        ]
        .spacing(16)
        .max_width(900);
        if matches!(
            draft.source,
            ConnectionSource::SshConfig | ConnectionSource::SshConfigBatch
        ) {
            content = content.push(connection_read_only_field(
                locale.text(TextKey::SshConfigFile),
                &draft.ssh_config_path,
            ));
        }
        if draft.auth == AuthMethod::Key {
            content = content.push(
                row![
                    connection_read_only_field(
                        locale.text(TextKey::PrivateKeyFile),
                        &draft.key_file,
                    ),
                    connection_read_only_field(
                        locale.text(TextKey::KeyPassphrase),
                        connection_secret_state_label(
                            locale,
                            draft.preserved_secret_state(CredentialKind::KeyPassphrase),
                        ),
                    ),
                ]
                .spacing(12),
            );
        } else {
            content = content.push(connection_read_only_field(
                locale.text(TextKey::Password),
                connection_secret_state_label(
                    locale,
                    draft.preserved_secret_state(CredentialKind::Password),
                ),
            ));
        }
        content = content
            .push(
                row![
                    connection_read_only_field(
                        locale.text(TextKey::WriteManagedProfile),
                        localized_yes_no(locale, draft.ssh_config_managed),
                    ),
                    connection_read_only_field(
                        locale.text(TextKey::CopyPrivateKey),
                        localized_yes_no(locale, draft.copy_key_to_ssh_dir),
                    ),
                ]
                .spacing(12),
            )
            .push(
                row![
                    connection_read_only_field(
                        locale.text(TextKey::RemotePath),
                        &draft.remote_path,
                    ),
                    connection_read_only_field(
                        locale.text(TextKey::Mountpoint),
                        display_draft_mountpoint(draft, locale),
                    ),
                ]
                .spacing(12),
            );
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
        let mount_backend = if mount_backend_settings_visible(std::env::consts::OS) {
            column![
                text(match locale {
                    Locale::English => "Mount method",
                    Locale::Chinese => "挂载方式",
                })
                .size(20),
                pick_list(
                    localized_choices(MountBackend::ALL, locale, Locale::mount_backend),
                    Some(locale.choice(
                        draft.mount_backend,
                        locale.mount_backend(draft.mount_backend),
                    )),
                    |backend| Message::MountBackendChanged(backend.value)
                )
                .width(Fill),
                text(mount_backend_help(locale)).size(14),
            ]
            .spacing(8)
            .max_width(640)
        } else {
            column![]
        };
        let credential_storage = column![
            text(match locale {
                Locale::English => "Credential storage",
                Locale::Chinese => "凭据存储",
            })
            .size(20),
            pick_list(
                localized_choices(CredentialStorage::ALL, locale, Locale::credential_storage,),
                Some(locale.choice(
                    draft.credential_storage,
                    locale.credential_storage(draft.credential_storage),
                )),
                |storage| Message::CredentialStorageChanged(storage.value)
            )
            .width(Fill),
            text(credential_storage_help(locale)).size(14),
        ]
        .spacing(8)
        .max_width(640);
        let appearance = column![
            text(match locale {
                Locale::English => "Appearance",
                Locale::Chinese => "外观",
            })
            .size(20),
            row![
                labeled_control(
                    match locale {
                        Locale::English => "Theme",
                        Locale::Chinese => "主题",
                    },
                    pick_list(
                        localized_choices(
                            AppearanceMode::ALL,
                            locale,
                            Locale::appearance_mode,
                        ),
                        Some(locale.choice(
                            draft.appearance_mode,
                            locale.appearance_mode(draft.appearance_mode),
                        )),
                        |mode| Message::AppearanceModeChanged(mode.value),
                    )
                    .width(Fill),
                ),
                labeled_control(
                    match locale {
                        Locale::English => "Accent",
                        Locale::Chinese => "强调色",
                    },
                    pick_list(
                        localized_choices(AccentColor::ALL, locale, Locale::accent_color),
                        Some(locale.choice(
                            draft.accent_color,
                            locale.accent_color(draft.accent_color),
                        )),
                        |accent| Message::AccentColorChanged(accent.value),
                    )
                    .width(Fill),
                ),
                labeled_control(
                    match locale {
                        Locale::English => "Text size",
                        Locale::Chinese => "字号",
                    },
                    pick_list(
                        localized_choices(FontScale::ALL, locale, Locale::font_scale),
                        Some(locale.choice(draft.font_scale, locale.font_scale(draft.font_scale))),
                        |font_scale| Message::FontScaleChanged(font_scale.value),
                    )
                    .width(Fill),
                ),
            ]
            .spacing(12),
            text(match locale {
                Locale::English => {
                    "Theme, accent, and text size preview immediately; Save makes them persistent. Follow system is detected when the app starts."
                }
                Locale::Chinese => {
                    "主题、强调色和字号会立即预览，保存后持久生效；跟随系统会在应用启动时检测。"
                }
            })
            .size(14),
        ]
        .spacing(8)
        .max_width(640);
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
            setting_picker(
                SettingKind::Transfers,
                locale.text(TextKey::UploadConcurrency),
                &draft.upload_transfers,
                locale,
            ),
        ]
        .spacing(12);
        let mut connection_preferences = column![
            text(match locale {
                Locale::English => "Connection management",
                Locale::Chinese => "连接管理",
            })
            .size(20),
            text(match locale {
                Locale::English => {
                    "Tags and batch actions are managed from the connection list. Login startup preferences stay available here when expanded."
                }
                Locale::Chinese => {
                    "标签与批量操作在连接列表中管理；登录自启偏好可按需展开。"
                }
            })
            .size(14),
            row![
                button(match (locale, draft.connection_preferences_expanded) {
                    (Locale::English, false) => "Show login preferences",
                    (Locale::English, true) => "Hide login preferences",
                    (Locale::Chinese, false) => "展开登录自启设置",
                    (Locale::Chinese, true) => "收起登录自启设置",
                })
                .on_press(Message::ToggleSettingsConnectionPreferences),
                button(match locale {
                    Locale::English => "Open batch management",
                    Locale::Chinese => "打开批量管理",
                })
                .on_press_maybe((!self.editor_saving).then_some(Message::OpenBatchManagement)),
            ]
            .spacing(10),
        ]
        .spacing(8);
        if draft.connection_preferences_expanded {
            for preference in &draft.connection_preferences {
                let startup_id = preference.id.clone();
                let startup: Element<'_, Message> = if preference.startup_available {
                    toggler(preference.auto_mount_at_login)
                        .label(match locale {
                            Locale::English => "Mount at login",
                            Locale::Chinese => "登录时挂载",
                        })
                        .on_toggle(move |value| {
                            Message::SettingsConnectionStartupChanged(startup_id.clone(), value)
                        })
                        .into()
                } else {
                    text(match locale {
                        Locale::English => "Interactive SSH: unavailable",
                        Locale::Chinese => "交互式 SSH：不可用",
                    })
                    .size(13)
                    .into()
                };
                let tags = if preference.tags.is_empty() {
                    match locale {
                        Locale::English => "No tags".to_owned(),
                        Locale::Chinese => "无标签".to_owned(),
                    }
                } else {
                    preference.tags.join(", ")
                };
                connection_preferences = connection_preferences.push(
                    container(
                        row![
                            column![text(&preference.name).size(14), text(tags).size(12)]
                                .spacing(3)
                                .width(Fill),
                            container(startup).width(Length::Fixed(210.0)),
                        ]
                        .spacing(12)
                        .align_y(Center),
                    )
                    .padding(10)
                    .width(Fill)
                    .style(container::rounded_box),
                );
            }
        }
        let behavior = column![
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
            if cfg!(windows) {
                dependency_section = dependency_section.push(text(format!(
                    "Plink (optional interactive sharing): {}",
                    available(dependencies.plink.is_some())
                )));
            }
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
                update_section = update_section.push(text(info.trust_error.as_ref().map_or_else(
                    || match locale {
                        Locale::English => {
                            "A verified package is not available for this platform".into()
                        }
                        Locale::Chinese => "当前平台暂无已验证的安装包".into(),
                    },
                    |error| automatic_install_blocked_message(locale, error),
                )));
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
            mount_backend,
            credential_storage,
            appearance,
            cache_profile,
            cache_limits,
            cache_timing,
            connection_preferences,
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
            let mut custom_value = row![
                text_input(
                    if custom.raw_value.is_some() {
                        "value"
                    } else {
                        "0"
                    },
                    &custom.digits,
                )
                .on_input(Message::CustomSettingDigitsChanged)
                .width(Length::Fixed(180.0))
            ]
            .spacing(10);
            if !units.is_empty() && custom.raw_value.is_none() {
                custom_value = custom_value.push(
                    pick_list(
                        units,
                        Some(custom.unit.clone()),
                        Message::CustomSettingUnitChanged,
                    )
                    .width(Length::Fixed(120.0)),
                );
            }
            let dialog = container(
                column![
                    text(title).size(22),
                    custom_value,
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
            let guidance = match locale {
                Locale::English => {
                    "Choose a connection from the selector above to view its mount log."
                }
                Locale::Chinese => "请从上方选择器选择一个连接以查看其挂载日志。",
            };
            return editor_shell(
                row![
                    text(locale.text(TextKey::Logs)).size(28),
                    Space::new().width(Fill),
                    selector,
                    button("x").on_press(Message::CloseLog),
                ]
                .spacing(10)
                .align_y(Center),
                container(text(guidance).size(18))
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
        if !log_view.loading && !log_view.existed {
            details = details.push(
                text(match locale {
                    Locale::English => format!(
                        "No log file exists at {}. This connection may never have been mounted, or logging has not started.",
                        log_view.path.display()
                    ),
                    Locale::Chinese => format!(
                        "{} 尚不存在。该连接可能从未挂载，或日志记录尚未开始。",
                        log_view.path.display()
                    ),
                })
                .size(13),
            );
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

    fn interactive_terminal_view(&self) -> Element<'_, Message> {
        let locale = self.locale();
        let Some(server_id) = self.terminal_server_id.as_deref() else {
            return container(text(locale.text(TextKey::InteractiveTerminalFailed)))
                .width(Fill)
                .height(Fill)
                .into();
        };
        if let Some(error) = interactive_terminal_error(&self.terminal_error, server_id) {
            let can_restart_or_end =
                interactive_session_can_restart_or_end(self.mount_statuses.get(server_id).copied());
            return column![
                text(locale.text(TextKey::InteractiveTerminal)).size(24),
                text(locale.text(TextKey::InteractiveTerminalHelp)).size(13),
                text(error),
                row![
                    button(locale.text(TextKey::RetryTerminal))
                        .on_press_maybe(can_restart_or_end.then_some(Message::RetryTerminal)),
                    button(locale.text(TextKey::HideTerminal)).on_press(Message::HideTerminal),
                    button(locale.text(TextKey::EndInteractiveSession)).on_press_maybe(
                        can_restart_or_end.then_some(Message::EndInteractiveSession)
                    ),
                ]
                .spacing(10),
            ]
            .spacing(10)
            .padding(14)
            .into();
        }
        let Some(session) = self.interactive_terminals.get(server_id) else {
            return container(text(locale.text(TextKey::InteractiveTerminalFailed)))
                .width(Fill)
                .height(Fill)
                .into();
        };
        let lifecycle = match session.lifecycle {
            InteractiveTerminalLifecycle::Starting => {
                locale.text(TextKey::InteractiveTerminalStarting)
            }
            InteractiveTerminalLifecycle::Ready => locale.text(TextKey::InteractiveTerminalReady),
            InteractiveTerminalLifecycle::Exited => locale.text(TextKey::InteractiveTerminalExited),
            InteractiveTerminalLifecycle::Failed => locale.text(TextKey::InteractiveTerminalFailed),
        };
        let terminal: Element<'_, Message> = iced_term::TerminalView::show(&session.terminal)
            .map(|event| Message::TerminalEvent(RedactedTerminalEvent(event)));
        let terminal = keyed_column([(session.generation, terminal)]).height(Fill);
        let can_restart_or_end =
            interactive_session_can_restart_or_end(self.mount_statuses.get(server_id).copied());
        let controls = row![
            text(lifecycle),
            Space::new().width(Fill),
            button(locale.text(TextKey::RetryTerminal)).on_press_maybe(
                (session.lifecycle != InteractiveTerminalLifecycle::Starting && can_restart_or_end)
                    .then_some(Message::RetryTerminal),
            ),
            button(locale.text(TextKey::HideTerminal)).on_press(Message::HideTerminal),
            button(locale.text(TextKey::EndInteractiveSession))
                .on_press_maybe(can_restart_or_end.then_some(Message::EndInteractiveSession)),
        ]
        .spacing(10)
        .align_y(Center);
        column![
            text(locale.text(TextKey::InteractiveTerminal)).size(24),
            text(locale.text(TextKey::InteractiveTerminalHelp)).size(13),
            controls,
            terminal,
        ]
        .spacing(10)
        .padding(14)
        .into()
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
    operation_error: Option<&'a ConnectionOperationError>,
    has_interactive_terminal: bool,
    auto_mount_at_login: bool,
    login_startup_available: bool,
    list_mode: ConnectionListMode,
    selected: bool,
    can_move_up: bool,
    can_move_down: bool,
    connection_list_saving: bool,
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
        operation_error,
        has_interactive_terminal,
        auto_mount_at_login,
        login_startup_available,
        list_mode,
        selected,
        can_move_up,
        can_move_down,
        connection_list_saving,
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
    let mut title = row![text(server.display_name()).size(22)]
        .spacing(8)
        .align_y(Center);
    if auto_mount_at_login && list_mode != ConnectionListMode::Batch {
        title = title.push(
            text(match locale {
                Locale::English => "At login",
                Locale::Chinese => "登录自启",
            })
            .size(12),
        );
    }
    let mut tag_row = row![].spacing(6).align_y(Center);
    for tag in &server.tags {
        if list_mode == ConnectionListMode::Tags {
            tag_row =
                tag_row.push(
                    container(
                        row![
                            text(tag).size(11),
                            button(text("×").size(10))
                                .padding([1, 5])
                                .style(button::secondary)
                                .on_press_maybe((!connection_list_saving).then_some(
                                    Message::RemoveConnectionTag(id.clone(), tag.clone())
                                )),
                        ]
                        .spacing(3)
                        .align_y(Center),
                    )
                    .padding([1, 4])
                    .style(container::rounded_box),
                );
        } else {
            tag_row = tag_row.push(container(text(tag).size(11)).padding([2, 5]));
        }
    }
    let tag_row = scrollable(tag_row)
        .direction(iced::widget::scrollable::Direction::Horizontal(
            iced::widget::scrollable::Scrollbar::new(),
        ))
        .height(Length::Shrink);
    let mut details = column![
        title,
        tag_row,
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
    if let Some(error) = operation_error {
        let operation = error.operation;
        details = details.push(
            container(
                column![
                    text(mount_error_summary(locale, &error.cause)).size(13),
                    row![
                        button(match locale {
                            Locale::English => "Retry",
                            Locale::Chinese => "重试",
                        })
                        .on_press(mount_error_message(
                            id.clone(),
                            operation,
                            MountErrorAction::Retry,
                        )),
                        button(match locale {
                            Locale::English => "View full log",
                            Locale::Chinese => "查看完整日志",
                        })
                        .on_press(mount_error_message(
                            id.clone(),
                            operation,
                            MountErrorAction::ViewLog,
                        )),
                        button(match locale {
                            Locale::English => "Dismiss",
                            Locale::Chinese => "关闭",
                        })
                        .on_press(mount_error_message(
                            id.clone(),
                            operation,
                            MountErrorAction::Dismiss,
                        )),
                    ]
                    .spacing(8),
                ]
                .spacing(6),
            )
            .padding(10)
            .width(Fill)
            .style(container::rounded_box),
        );
    }
    if list_mode == ConnectionListMode::Batch {
        let selection_id = id.clone();
        let startup_id = id.clone();
        let startup_control: Element<'_, Message> = if login_startup_available
            && !connection_list_saving
        {
            checkbox(auto_mount_at_login)
                .label(match locale {
                    Locale::English => "At login",
                    Locale::Chinese => "登录自启",
                })
                .on_toggle(move |enabled| Message::BatchStartupChanged(startup_id.clone(), enabled))
                .into()
        } else if login_startup_available {
            checkbox(auto_mount_at_login)
                .label(match locale {
                    Locale::English => "At login",
                    Locale::Chinese => "登录自启",
                })
                .into()
        } else {
            text(match locale {
                Locale::English => "Interactive login",
                Locale::Chinese => "交互式登录",
            })
            .size(12)
            .into()
        };
        return container(
            row![
                checkbox(selected).on_toggle(move |selected| {
                    Message::BatchSelectionChanged(selection_id.clone(), selected)
                }),
                details,
                startup_control,
            ]
            .spacing(12)
            .align_y(Center),
        )
        .padding(16)
        .width(Fill)
        .style(container::rounded_box)
        .into();
    }
    if list_mode == ConnectionListMode::Reorder {
        return container(
            row![
                details,
                button("^").on_press_maybe(
                    (can_move_up && !connection_list_saving)
                        .then_some(Message::MoveConnection(id.clone(), -1)),
                ),
                button("v").on_press_maybe(
                    (can_move_down && !connection_list_saving)
                        .then_some(Message::MoveConnection(id.clone(), 1)),
                ),
            ]
            .spacing(8)
            .align_y(Center),
        )
        .padding(16)
        .width(Fill)
        .style(container::rounded_box)
        .into();
    }
    let edit = button(locale.text(TextKey::Edit))
        .on_press_maybe((!connection_list_saving).then_some(Message::Edit(id.clone())));
    let actions: Element<'_, Message> = if confirming_remove {
        row![
            button(locale.text(TextKey::Cancel)).on_press(Message::CancelRemove),
            button(locale.text(TextKey::ConfirmRemove)).on_press(Message::ConfirmRemove),
        ]
        .spacing(8)
        .into()
    } else {
        let mut actions = row![edit].spacing(8);
        if has_interactive_terminal {
            actions = actions.push(
                button(locale.text(TextKey::OpenInteractiveTerminal))
                    .on_press(Message::OpenInteractiveTerminal(id.clone())),
            );
        }
        actions
            .push(
                button(locale.text(TextKey::Remove))
                    .on_press_maybe(can_modify.then_some(Message::Remove(id))),
            )
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

fn visible_connections<'a>(
    servers: &'a [ServerConfig],
    search: &str,
    tag_filter: Option<&str>,
    sort: ConnectionSort,
) -> Vec<&'a ServerConfig> {
    let query = search.trim().to_lowercase();
    let mut matches = servers
        .iter()
        .enumerate()
        .filter(|(_, server)| {
            let matches_tag =
                tag_filter.is_none_or(|tag| server.tags.iter().any(|candidate| candidate == tag));
            let matches_query = query.is_empty()
                || server
                    .tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains(&query))
                || [
                    server.display_name(),
                    server.host_alias.as_str(),
                    server.host.as_str(),
                    server.user.as_str(),
                    server.remote_path.as_str(),
                    server.mountpoint.as_str(),
                ]
                .into_iter()
                .any(|value| value.to_lowercase().contains(&query));
            matches_tag && matches_query
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_index, left), (right_index, right)| {
        let ordering = match sort {
            ConnectionSort::SavedOrder => std::cmp::Ordering::Equal,
            ConnectionSort::Name => left
                .display_name()
                .to_lowercase()
                .cmp(&right.display_name().to_lowercase()),
            ConnectionSort::Host => left
                .host
                .to_lowercase()
                .cmp(&right.host.to_lowercase())
                .then_with(|| left.user.to_lowercase().cmp(&right.user.to_lowercase())),
        };
        ordering.then_with(|| left_index.cmp(right_index))
    });
    matches.into_iter().map(|(_, server)| server).collect()
}

fn connection_tags(servers: &[ServerConfig]) -> Vec<String> {
    let mut tags = Vec::new();
    for server in servers {
        for tag in &server.tags {
            if !tags.iter().any(|existing| existing == tag) {
                tags.push(tag.clone());
            }
        }
    }
    tags.sort_by_key(|tag| tag.to_lowercase());
    tags
}

fn parse_tag_input(value: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for tag in value.split([',', '，']) {
        let tag = tag.trim();
        if !tag.is_empty() && !tags.iter().any(|existing| existing == tag) {
            tags.push(tag.to_owned());
        }
    }
    tags
}

fn normalized_tag_name(value: &str, locale: Locale) -> Result<String, String> {
    let tag = value.trim();
    if tag.is_empty()
        || tag.chars().any(char::is_control)
        || tag.contains(',')
        || tag.contains('，')
    {
        return Err(match locale {
            Locale::English => {
                "A tag must be non-empty and cannot contain commas or control characters".into()
            }
            Locale::Chinese => "标签不能为空，也不能包含逗号或控制字符".into(),
        });
    }
    if tag.chars().count() > MAX_TAG_CHARS {
        return Err(match locale {
            Locale::English => format!("A tag must be at most {MAX_TAG_CHARS} characters"),
            Locale::Chinese => format!("标签最多只能有 {MAX_TAG_CHARS} 个字符"),
        });
    }
    Ok(tag.to_owned())
}

fn selected_server_ids(servers: &[ServerConfig], selected: &HashSet<String>) -> Vec<String> {
    servers
        .iter()
        .filter(|server| selected.contains(&server.id))
        .map(|server| server.id.clone())
        .collect()
}

fn can_move_connection(servers: &[ServerConfig], id: &str, direction: i8) -> bool {
    let Some(index) = servers.iter().position(|server| server.id == id) else {
        return false;
    };
    match direction {
        -1 => index > 0,
        1 => index + 1 < servers.len(),
        _ => false,
    }
}

fn moved_connection_order(
    servers: &[ServerConfig],
    id: &str,
    direction: i8,
) -> Option<Vec<String>> {
    let mut order = servers
        .iter()
        .map(|server| server.id.clone())
        .collect::<Vec<_>>();
    let index = order.iter().position(|candidate| candidate == id)?;
    let target = match direction {
        -1 => index.checked_sub(1)?,
        1 if index + 1 < order.len() => index + 1,
        _ => return None,
    };
    order.swap(index, target);
    Some(order)
}

fn connection_input<'a>(
    label: &'a str,
    value: &'a str,
    field: ConnectionField,
    required: bool,
) -> iced::widget::Column<'a, Message> {
    column![
        connection_field_label(label, required),
        text_input(label, value)
            .on_input(move |value| Message::ConnectionFieldChanged(field, value))
            .width(Fill),
    ]
    .spacing(5)
    .width(Fill)
}

fn connection_read_only_field<'a>(
    label: &'a str,
    value: &'a str,
) -> iced::widget::Column<'a, Message> {
    labeled_control(label, container(text(value)).padding(10).width(Fill))
}

fn secret_input_control<'a>(
    label: &'a str,
    placeholder: &'a str,
    value: &'a str,
    state: PreservedSecretState,
    kind: CredentialKind,
    locale: Locale,
    required: bool,
) -> iced::widget::Column<'a, Message> {
    let input = text_input(placeholder, value)
        .secure(true)
        .on_input(move |value| match kind {
            CredentialKind::Password => Message::PasswordChanged(SecretInput(value)),
            CredentialKind::KeyPassphrase => Message::KeyPassphraseChanged(SecretInput(value)),
        })
        .width(Fill);
    let mut control = column![connection_field_label(label, required), input]
        .spacing(5)
        .width(Fill);
    if state != PreservedSecretState::Absent {
        let state_text = match (locale, state) {
            (Locale::English, PreservedSecretState::System) => {
                "Stored in the system credential store. Leave blank to keep it, or type to replace it."
            }
            (Locale::Chinese, PreservedSecretState::System) => {
                "已存入系统凭据库。留空会保留，输入新值会替换。"
            }
            (Locale::English, PreservedSecretState::Obscured) => {
                "Stored with rclone obscure. Leave blank to keep it, or type to replace it."
            }
            (Locale::Chinese, PreservedSecretState::Obscured) => {
                "已使用 rclone obscure 保存。留空会保留，输入新值会替换。"
            }
            (_, PreservedSecretState::Absent) => unreachable!(),
        };
        control = control.push(
            row![
                text(state_text).size(12),
                button(match locale {
                    Locale::English => "Clear stored value",
                    Locale::Chinese => "清除已存值",
                })
                .on_press(Message::ClearSecret(kind)),
            ]
            .spacing(8)
            .align_y(Center),
        );
    }
    control
}

fn connection_file_input<'a>(
    label: &'a str,
    value: &'a str,
    field: ConnectionField,
    browse: Message,
    browse_label: &'a str,
    required: bool,
) -> iced::widget::Column<'a, Message> {
    column![
        connection_field_label(label, required),
        row![
            text_input(label, value)
                .on_input(move |value| Message::ConnectionFieldChanged(field, value))
                .width(Fill),
            button(browse_label).on_press(browse),
        ]
        .spacing(8),
    ]
    .spacing(5)
    .width(Fill)
}

fn connection_field_label<'a>(label: &'a str, required: bool) -> iced::widget::Row<'a, Message> {
    let mut content = row![text(label).size(13)].spacing(3).align_y(Center);
    if required {
        content = content.push(text("*").size(13).color(Color::from_rgb8(210, 48, 48)));
    }
    content
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
        .or_else(|| Some(selected_custom_setting_option(kind, value, locale)));
    column![
        row![
            text(label).size(13),
            settings_help(setting_help(kind, locale))
        ]
        .spacing(5),
        pick_list(options, selected, Message::SettingOptionChanged).width(Fill),
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
        SettingKind::Transfers => &["4", "8", "12"],
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

fn custom_setting_label(locale: Locale, value: &str) -> String {
    match locale {
        Locale::English => format!("Custom: {value}"),
        Locale::Chinese => format!("自定义：{value}"),
    }
}

fn selected_custom_setting_option(kind: SettingKind, value: &str, locale: Locale) -> SettingOption {
    SettingOption {
        kind,
        value: value.to_owned(),
        label: custom_setting_label(locale, value),
        custom: true,
    }
}

fn setting_value(draft: &SettingsDraft, kind: SettingKind) -> &str {
    match kind {
        SettingKind::MaxSize => &draft.max_size,
        SettingKind::MaxAge => &draft.max_age,
        SettingKind::MinFreeSpace => &draft.min_free_space,
        SettingKind::WriteBack => &draft.write_back,
        SettingKind::DirCacheTime => &draft.dir_cache_time,
        SettingKind::BufferSize => &draft.buffer_size,
        SettingKind::Transfers => &draft.upload_transfers,
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
        SettingKind::Transfers => draft.upload_transfers = value,
    }
}

fn custom_units(kind: SettingKind) -> &'static [&'static str] {
    match kind {
        SettingKind::MaxSize | SettingKind::MinFreeSpace => &["Mi", "Gi", "Ti"],
        SettingKind::BufferSize => &["Ki", "Mi", "Gi"],
        SettingKind::MaxAge | SettingKind::WriteBack | SettingKind::DirCacheTime => {
            &["s", "m", "h", "d"]
        }
        SettingKind::Transfers => &[],
    }
}

fn split_custom_setting(kind: SettingKind, value: &str) -> (String, String) {
    let digits = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    let suffix = value.get(digits.len()..).unwrap_or_default();
    let units = custom_units(kind);
    if units.is_empty() {
        return (digits, String::new());
    }
    if value.is_empty() {
        return (String::new(), units[0].to_owned());
    }
    if let Some(unit) = units.iter().find(|unit| **unit == suffix) {
        (digits, (*unit).to_owned())
    } else {
        (value.to_owned(), String::new())
    }
}

fn custom_setting_is_supported(kind: SettingKind, value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if kind == SettingKind::Transfers {
        return value.chars().all(|character| character.is_ascii_digit());
    }
    let (digits, unit) = split_custom_setting(kind, value);
    !digits.is_empty()
        && digits.len() + unit.len() == value.len()
        && custom_units(kind).iter().any(|allowed| *allowed == unit)
}

fn custom_setting_value(custom: &CustomSettingDraft, locale: Locale) -> Result<String, String> {
    if custom.digits.is_empty() {
        return Err(match locale {
            Locale::English => "Custom value must contain digits".into(),
            Locale::Chinese => "自定义数值必须填写数字".into(),
        });
    }
    if let Some(raw_value) = &custom.raw_value {
        return Ok(raw_value.clone());
    }
    if custom.kind == SettingKind::Transfers {
        return validate_upload_transfers(&custom.digits, locale).map(|value| value.to_string());
    }
    Ok(format!("{}{}", custom.digits, custom.unit))
}

fn startup_servers(settings: &Settings, servers: &[ServerConfig]) -> Vec<ServerConfig> {
    servers
        .iter()
        .filter(|server| {
            (settings.startup_all || server.auto_mount_at_login)
                && server.connection_method != ConnectionMethod::Interactive
        })
        .cloned()
        .collect()
}

fn servers_require_system_credentials(servers: &[ServerConfig]) -> bool {
    servers.iter().any(|server| {
        (!server.password_credential.is_empty() && server.password_obscured.is_empty())
            || (!server.key_pass_credential.is_empty() && server.key_pass_obscured.is_empty())
    })
}

fn settings_need_system_credential_inference(
    settings: &Settings,
    servers: &[ServerConfig],
) -> bool {
    settings.credential_storage == CredentialStorage::Obscure
        && servers_require_system_credentials(servers)
}

fn connection_preference_updates(
    draft: &SettingsDraft,
    _locale: Locale,
) -> Result<Vec<storage::ServerPreferenceUpdate>, String> {
    Ok(draft
        .connection_preferences
        .iter()
        .map(|preference| storage::ServerPreferenceUpdate {
            id: preference.id.clone(),
            tags: None,
            auto_mount_at_login: Some(
                preference.auto_mount_at_login && preference.startup_available,
            ),
        })
        .collect())
}

fn validated_connection_tags(tags: &[String], locale: Locale) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for tag in tags {
        let tag = normalized_tag_name(tag, locale)?;
        if !normalized.iter().any(|candidate| candidate == &tag) {
            normalized.push(tag);
        }
    }
    if normalized.len() > MAX_CONNECTION_TAGS {
        return Err(match locale {
            Locale::English => format!("A connection may have at most {MAX_CONNECTION_TAGS} tags"),
            Locale::Chinese => format!("一个连接最多只能有 {MAX_CONNECTION_TAGS} 个标签"),
        });
    }
    Ok(normalized)
}

fn batch_tag_to_add(
    new_tag: &str,
    existing_tag: Option<&str>,
    locale: Locale,
) -> Result<String, String> {
    let tag = if new_tag.trim().is_empty() {
        existing_tag.unwrap_or_default()
    } else {
        new_tag
    };
    normalized_tag_name(tag, locale)
}

fn startup_integration_enabled(settings: &Settings, servers: &[ServerConfig]) -> bool {
    !startup_servers(settings, servers).is_empty()
}

fn reconcile_login_startup(paths: &AppPaths, startup_lock: &Mutex<()>) -> Result<(), String> {
    let _guard = startup_lock
        .lock()
        .map_err(|_| "login startup integration lock is poisoned".to_owned())?;
    let settings = storage::load_settings(paths).map_err(|error| error.to_string())?;
    let servers = storage::load_servers(paths).map_err(|error| error.to_string())?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Platform
        .set_login_startup(
            &executable,
            startup_integration_enabled(&settings, &servers),
        )
        .map_err(|error| error.to_string())
}

fn migrate_legacy_startup_preferences(
    paths: &AppPaths,
    settings: &Settings,
    servers: &[ServerConfig],
) -> Result<(Settings, Vec<ServerConfig>), String> {
    if !settings.startup_all {
        return Ok((settings.clone(), servers.to_vec()));
    }
    let mut migrated_servers = servers.to_vec();
    for server in &mut migrated_servers {
        server.auto_mount_at_login = server.connection_method != ConnectionMethod::Interactive;
    }
    storage::save_servers(paths, &migrated_servers).map_err(|error| error.to_string())?;
    let mut migrated_settings = settings.clone();
    migrated_settings.startup_all = false;
    if let Err(error) = storage::save_settings(paths, &migrated_settings) {
        let rollback = storage::save_servers(paths, servers).map_err(|error| error.to_string());
        let mut message = error.to_string();
        if let Err(rollback) = rollback {
            message.push_str(&format!("; server rollback failed: {rollback}"));
        }
        return Err(message);
    }
    Ok((migrated_settings, migrated_servers))
}

fn legacy_startup_migration_needed(settings: &Settings, servers_loaded: bool) -> bool {
    settings.startup_all && servers_loaded
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
        (SettingKind::Transfers, Locale::English) => {
            "Maximum number of different cached files uploaded at once on the next mount. Custom range: 1-32; rclone has no unlimited VFS upload setting. Extra files wait locally, and higher values increase SSH/SFTP and server load."
        }
        (SettingKind::Transfers, Locale::Chinese) => {
            "下次挂载时同时从缓存上传的不同文件数量上限。自定义范围为 1-32；rclone 没有无限 VFS 上传并发设置。超出数量的文件会留在本地排队，数值越高，SSH/SFTP 和服务器负载越大。"
        }
    }
}

fn validate_upload_transfers(value: &str, locale: Locale) -> Result<u16, String> {
    let parsed = value.trim().parse::<u16>().ok();
    match parsed {
        Some(value) if (MIN_VFS_UPLOAD_TRANSFERS..=MAX_VFS_UPLOAD_TRANSFERS).contains(&value) => {
            Ok(value)
        }
        _ => Err(match locale {
            Locale::English => format!(
                "Simultaneous uploads must be a whole number from {MIN_VFS_UPLOAD_TRANSFERS} to {MAX_VFS_UPLOAD_TRANSFERS}"
            ),
            Locale::Chinese => format!(
                "同时上传文件数必须是 {MIN_VFS_UPLOAD_TRANSFERS} 到 {MAX_VFS_UPLOAD_TRANSFERS} 之间的整数"
            ),
        }),
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

fn custom_mountpoint_draft_value(value: String) -> String {
    if value.trim().is_empty() {
        CUSTOM_MOUNTPOINT_PENDING.into()
    } else {
        value
    }
}

fn mountpoint_value_for_choice(choice: &str, custom_mountpoint: &str) -> String {
    match choice {
        "auto" => String::new(),
        "home" => HOME_MOUNTPOINT_VALUE.into(),
        "custom" => custom_mountpoint_draft_value(custom_mountpoint.into()),
        drive => drive.to_owned(),
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

fn mount_backend_settings_visible(os: &str) -> bool {
    os == "macos"
}

fn mount_backend_help(locale: Locale) -> &'static str {
    match locale {
        Locale::English => {
            "Experimental built-in NFS uses a loopback-only NFS service and does not require macFUSE or FUSE-T. Filesystem semantics, performance, and cache behavior can differ from FUSE. Changes apply to the next mount and do not interrupt active mounts."
        }
        Locale::Chinese => {
            "实验性的内置 NFS 使用仅限本机回环的 NFS 服务，不需要 macFUSE 或 FUSE-T。文件系统语义、性能和缓存行为可能与 FUSE 不同。设置变更仅影响下一次挂载，不会中断已有挂载。"
        }
    }
}

fn credential_storage_help(locale: Locale) -> &'static str {
    match locale {
        Locale::English => {
            "The compatible default uses reversible rclone obscure and is not strong encryption. The system store is opt-in and migrates passwords and private-key passphrases only after write-and-read verification. Private key files remain ordinary files."
        }
        Locale::Chinese => {
            "兼容默认使用可逆的 rclone obscure，并非强加密。系统凭据库需要手动启用；密码和私钥短语只有在写入并回读验证后才会迁移。私钥文件仍作为普通文件保存。"
        }
    }
}

fn credential_storage_confirmation(locale: Locale, target: CredentialStorage) -> &'static str {
    match (locale, target) {
        (Locale::English, CredentialStorage::System) => {
            "Enable the system credential store? Existing passwords and private-key passphrases will be revealed locally, written to the OS store, and read back for verification. A verified rclone-obscured compatibility copy remains in SSH MountMate's private configuration. System-store read failures never silently fall back to it. One-time codes are never stored."
        }
        (Locale::Chinese, CredentialStorage::System) => {
            "是否启用系统凭据库？现有密码和私钥短语会在本机解开，写入系统凭据库并回读验证。SSH MountMate 私有配置会保留一份经过验证的 rclone obscure 兼容副本；系统凭据读取失败时不会静默回退使用它。一次性验证码永远不会保存。"
        }
        (Locale::English, CredentialStorage::Obscure) => {
            "Return to rclone obscure storage? Vault secrets will be converted locally and saved in SSH MountMate's private configuration before vault entries are removed. This is less secure but more compatible."
        }
        (Locale::Chinese, CredentialStorage::Obscure) => {
            "是否恢复为 rclone obscure 存储？系统凭据会先在本机转换并保存到 SSH MountMate 私有配置，之后才删除凭据库条目。这种方式安全性较低但兼容性更好。"
        }
    }
}

fn interactive_auth_help(locale: Locale) -> &'static str {
    locale.text(TextKey::InteractiveTerminalHelp)
}

fn connection_settings_locked_help(locale: Locale) -> &'static str {
    match locale {
        Locale::English => {
            "This connection is mounted or busy. Settings are read-only; unmount it to make changes. Changes take effect on the next mount."
        }
        Locale::Chinese => {
            "此连接已挂载或正在执行操作。当前设置为只读；请先卸载再修改，变更会在下次挂载时生效。"
        }
    }
}

fn settings_recovery_message(
    recovered: &storage::RecoveredSettings,
    locale: Locale,
) -> Option<String> {
    let error = recovered.load_error.as_deref()?;
    Some(match locale {
        Locale::English if recovered.failure_stage.is_none() => recovered
            .backup_path
            .as_ref()
            .map(|path| {
                format!(
                    "The previous settings could not be read and were reset. The original was backed up at {}. Original error: {error}",
                    path.display()
                )
            })
            .unwrap_or_else(|| {
                format!("The previous settings could not be read and were reset. Original error: {error}")
            }),
        Locale::English => format!(
            "The previous settings could not be read. Built-in defaults are active in memory; the recovery dialog shows the exact settings and backup paths. Original error: {error}"
        ),
        Locale::Chinese if recovered.failure_stage.is_none() => recovered
            .backup_path
            .as_ref()
            .map(|path| {
                format!(
                    "旧设置无法读取，已重置；原文件备份在 {}。原始错误：{error}",
                    path.display()
                )
            })
            .unwrap_or_else(|| format!("旧设置无法读取，已重置。原始错误：{error}")),
        Locale::Chinese => format!(
            "旧设置无法读取，当前仅在内存中使用程序内置默认值；恢复弹窗列出了准确的设置路径和备份路径。原始错误：{error}"
        ),
    })
}

fn settings_recovery_dialog_message(
    recovered: &storage::RecoveredSettings,
    settings_path: &Path,
    locale: Locale,
) -> Option<String> {
    let load_error = recovered.load_error.as_deref()?;
    let backup_line = match (
        &recovered.backup_path,
        &recovered.attempted_backup_path,
        locale,
    ) {
        (Some(path), _, Locale::English) => format!("Backup saved at: {}", path.display()),
        (Some(path), _, Locale::Chinese) => format!("实际备份路径：{}", path.display()),
        (None, Some(path), Locale::English) => {
            format!("Attempted backup path: {}", path.display())
        }
        (None, Some(path), Locale::Chinese) => {
            format!("尝试备份路径：{}", path.display())
        }
        (None, None, Locale::English) => "Attempted backup path: unavailable".into(),
        (None, None, Locale::Chinese) => "尝试备份路径：不可用".into(),
    };
    let kind = match recovered.failure_stage {
        Some(
            storage::SettingsRecoveryStage::Backup
            | storage::SettingsRecoveryStage::RestoreOriginal,
        ) => recovered.backup_error_kind,
        Some(
            storage::SettingsRecoveryStage::AcquireLock
            | storage::SettingsRecoveryStage::PersistDefaults,
        ) => recovered.persistence_error_kind,
        None => None,
    };
    let hint = recovery_error_hint(recovered.failure_stage, kind, locale);
    let cache_root = recovered.settings.cache_root.display();

    Some(match locale {
        Locale::English => {
            let operation = match recovered.failure_stage {
                Some(storage::SettingsRecoveryStage::AcquireLock) => {
                    "Acquire the settings recovery lock"
                }
                Some(storage::SettingsRecoveryStage::Backup) => {
                    "Back up the unreadable settings file"
                }
                Some(storage::SettingsRecoveryStage::PersistDefaults) => {
                    "Write replacement default settings"
                }
                Some(storage::SettingsRecoveryStage::RestoreOriginal) => {
                    "Write defaults, then restore the original after that write failed"
                }
                None => "Back up the unreadable file and write replacement defaults",
            };
            let result = match recovered.failure_stage {
                Some(storage::SettingsRecoveryStage::AcquireLock) => format!(
                    "Recovery could not start. The original settings path was left unchanged at {}. Built-in defaults are active in memory only.",
                    settings_path.display()
                ),
                Some(storage::SettingsRecoveryStage::Backup) => format!(
                    "The original file remains at {}. Built-in defaults are active in memory only.",
                    settings_path.display()
                ),
                Some(storage::SettingsRecoveryStage::PersistDefaults) if recovered.original_restored => format!(
                    "Writing defaults failed, so the original was restored to {}. Built-in defaults are active in memory only.",
                    settings_path.display()
                ),
                Some(storage::SettingsRecoveryStage::PersistDefaults) => "Writing defaults failed. No original file was present when backup was attempted. Built-in defaults are active in memory only.".into(),
                Some(storage::SettingsRecoveryStage::RestoreOriginal) => recovered.backup_path.as_ref().map(|path| format!(
                    "Writing defaults failed and the original could not be restored to {}. The original remains at {}. Built-in defaults are active in memory only.",
                    settings_path.display(), path.display()
                )).unwrap_or_else(|| "Writing defaults and restoring the original both failed. Built-in defaults are active in memory only.".into()),
                None if recovered.source_was_present => "The unreadable original was backed up and built-in defaults were written successfully.".into(),
                None => "The source disappeared before backup; built-in defaults were written successfully.".into(),
            };
            let mut errors = format!("Original read error: {load_error}");
            if let Some(error) = &recovered.persistence_error {
                errors.push_str(&format!("\nDefault-settings write/lock error: {error}"));
            }
            if let Some(error) = &recovered.cleanup_error {
                errors.push_str(&format!("\nFailed-replacement cleanup error: {error}"));
            }
            if let Some(error) = &recovered.backup_error {
                errors.push_str(&format!("\nBackup/restore error: {error}"));
            }
            format!(
                "The previous settings could not be read.\n\nSettings file: {}\n{backup_line}\nOperation: {operation}\n{errors}\nReason: {hint}\n\nResult: {result}\nOpen Settings and choose Save to retry writing the displayed values when recovery did not finish. In-memory defaults are SSH MountMate's complete built-in settings, not values read from the old file. Current cache directory: {cache_root}. The values shown in Settings are the values Save will write.",
                settings_path.display(),
            )
        }
        Locale::Chinese => {
            let operation = match recovered.failure_stage {
                Some(storage::SettingsRecoveryStage::AcquireLock) => "取得设置恢复锁",
                Some(storage::SettingsRecoveryStage::Backup) => "备份无法读取的设置文件",
                Some(storage::SettingsRecoveryStage::PersistDefaults) => "写入替代用的默认设置",
                Some(storage::SettingsRecoveryStage::RestoreOriginal) => {
                    "默认设置写入失败后恢复原设置文件"
                }
                None => "备份无法读取的文件并写入默认设置",
            };
            let result = match recovered.failure_stage {
                Some(storage::SettingsRecoveryStage::AcquireLock) => format!("恢复尚未开始；原设置路径 {} 保持不变。当前仅在内存中使用程序内置默认设置。", settings_path.display()),
                Some(storage::SettingsRecoveryStage::Backup) => format!("原文件仍位于 {}。当前仅在内存中使用程序内置默认设置。", settings_path.display()),
                Some(storage::SettingsRecoveryStage::PersistDefaults) if recovered.original_restored => format!("默认设置写入失败，原文件已恢复到 {}。当前仅在内存中使用程序内置默认设置。", settings_path.display()),
                Some(storage::SettingsRecoveryStage::PersistDefaults) => "默认设置写入失败；尝试备份时原文件已经不存在。当前仅在内存中使用程序内置默认设置。".into(),
                Some(storage::SettingsRecoveryStage::RestoreOriginal) => recovered.backup_path.as_ref().map(|path| format!("默认设置写入失败，原文件也无法恢复到 {}；原文件仍位于 {}。当前仅在内存中使用程序内置默认设置。", settings_path.display(), path.display())).unwrap_or_else(|| "默认设置写入与原文件恢复均失败。当前仅在内存中使用程序内置默认设置。".into()),
                None if recovered.source_was_present => "无法读取的原文件已备份，程序内置默认设置已成功写入。".into(),
                None => "原文件在备份前已经消失，程序内置默认设置已成功写入。".into(),
            };
            let mut errors = format!("原始读取错误：{load_error}");
            if let Some(error) = &recovered.persistence_error {
                errors.push_str(&format!("\n默认设置写入/锁定错误：{error}"));
            }
            if let Some(error) = &recovered.cleanup_error {
                errors.push_str(&format!("\n失败替代文件清理错误：{error}"));
            }
            if let Some(error) = &recovered.backup_error {
                errors.push_str(&format!("\n备份/恢复原文件错误：{error}"));
            }
            format!(
                "旧设置无法读取。\n\n原设置路径：{}\n{backup_line}\n失败/执行操作：{operation}\n{errors}\n原因提示：{hint}\n\n结果：{result}\n恢复未完成时，请打开“设置”并点击“保存”重试写入页面显示的值。“内存默认值”指 SSH MountMate 程序内置的完整默认设置，并非从旧文件读取。当前缓存目录：{cache_root}。设置页当前显示的值就是点击“保存”时将写入的内容。",
                settings_path.display(),
            )
        }
    })
}

fn recovery_error_hint(
    stage: Option<storage::SettingsRecoveryStage>,
    kind: Option<std::io::ErrorKind>,
    locale: Locale,
) -> &'static str {
    match (stage, kind, locale) {
        (Some(storage::SettingsRecoveryStage::AcquireLock), _, Locale::English) => {
            "The settings lock could not be opened or is still held by another process"
        }
        (Some(storage::SettingsRecoveryStage::AcquireLock), _, Locale::Chinese) => {
            "设置锁无法打开，或仍被其他进程占用"
        }
        (_, Some(std::io::ErrorKind::PermissionDenied), Locale::English) => {
            "The settings directory or file is not writable, or another process is denying access"
        }
        (_, Some(std::io::ErrorKind::PermissionDenied), Locale::Chinese) => {
            "设置目录或文件没有写权限，或者其他进程正在拒绝访问"
        }
        (
            _,
            Some(
                std::io::ErrorKind::NotFound
                | std::io::ErrorKind::NotADirectory
                | std::io::ErrorKind::IsADirectory
                | std::io::ErrorKind::AlreadyExists
                | std::io::ErrorKind::InvalidInput,
            ),
            Locale::English,
        ) => "A settings path component disappeared or is unavailable",
        (
            _,
            Some(
                std::io::ErrorKind::NotFound
                | std::io::ErrorKind::NotADirectory
                | std::io::ErrorKind::IsADirectory
                | std::io::ErrorKind::AlreadyExists
                | std::io::ErrorKind::InvalidInput,
            ),
            Locale::Chinese,
        ) => "设置路径中的目录或文件已消失，或者当前不可用",
        (_, Some(std::io::ErrorKind::StorageFull), Locale::English) => {
            "The destination volume has no free space"
        }
        (_, Some(std::io::ErrorKind::StorageFull), Locale::Chinese) => "目标磁盘空间不足",
        (_, Some(_), Locale::English) => "The operating system rejected the file operation",
        (_, Some(_), Locale::Chinese) => "操作系统拒绝了文件操作",
        (_, None, Locale::English) => "No recovery write error was reported",
        (_, None, Locale::Chinese) => "未报告恢复写入错误",
    }
}

fn system_prefers_dark() -> bool {
    match dark_light::detect() {
        Ok(dark_light::Mode::Light) => false,
        Ok(dark_light::Mode::Dark | dark_light::Mode::Unspecified) | Err(_) => true,
    }
}

fn effective_appearance(
    settings: &Settings,
    draft: Option<&SettingsDraft>,
) -> (AppearanceMode, AccentColor) {
    draft.map_or((settings.appearance_mode, settings.accent_color), |draft| {
        (draft.appearance_mode, draft.accent_color)
    })
}

fn application_theme(mode: AppearanceMode, accent: AccentColor, system_dark: bool) -> Theme {
    let dark = match mode {
        AppearanceMode::System => system_dark,
        AppearanceMode::Light => false,
        AppearanceMode::Dark => true,
    };
    let mut palette = if dark { Palette::DARK } else { Palette::LIGHT };
    palette.primary = match (accent, dark) {
        (AccentColor::Blue, false) => Color::from_rgb8(52, 103, 209),
        (AccentColor::Blue, true) => Color::from_rgb8(110, 168, 254),
        (AccentColor::Green, false) => Color::from_rgb8(46, 125, 50),
        (AccentColor::Green, true) => Color::from_rgb8(102, 187, 106),
        (AccentColor::Amber, false) => Color::from_rgb8(154, 103, 0),
        (AccentColor::Amber, true) => Color::from_rgb8(255, 200, 87),
        (AccentColor::Purple, false) => Color::from_rgb8(123, 44, 191),
        (AccentColor::Purple, true) => Color::from_rgb8(187, 134, 252),
    };
    Theme::custom("SSH MountMate", palette)
}

fn localized_yes_no(locale: Locale, value: bool) -> &'static str {
    match (locale, value) {
        (Locale::English, true) => "Yes",
        (Locale::English, false) => "No",
        (Locale::Chinese, true) => "是",
        (Locale::Chinese, false) => "否",
    }
}

fn connection_secret_state_label(locale: Locale, state: PreservedSecretState) -> &'static str {
    match (locale, state) {
        (Locale::English, PreservedSecretState::System) => "Stored in system credentials",
        (Locale::English, PreservedSecretState::Obscured) => "Stored with rclone obscure",
        (Locale::English, PreservedSecretState::Absent) => "Not stored",
        (Locale::Chinese, PreservedSecretState::System) => "已存入系统凭据库",
        (Locale::Chinese, PreservedSecretState::Obscured) => "已使用 rclone obscure 保存",
        (Locale::Chinese, PreservedSecretState::Absent) => "未保存",
    }
}

fn display_draft_mountpoint(draft: &ConnectionDraft, locale: Locale) -> &str {
    if draft.mountpoint.is_empty()
        || draft.mountpoint == HOME_MOUNTPOINT_VALUE
        || draft.mountpoint.eq_ignore_ascii_case("auto")
    {
        locale.text(TextKey::AutoMountpoint)
    } else {
        &draft.mountpoint
    }
}

fn openssh_command_preview(config_path: &str, host_alias: &str) -> String {
    format!(
        "ssh -F {} {}",
        quote_command_preview_argument(config_path),
        quote_command_preview_argument(host_alias)
    )
}

fn quote_command_preview_argument(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._/:\\".contains(character))
    {
        value.into()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
    }
}

fn localize_service_error(locale: Locale, error: &ServiceError) -> String {
    if locale == Locale::English {
        return error.to_string();
    }
    match error {
        ServiceError::InteractiveSsh(InteractiveSshError::SessionMissing) => {
            "交互式 SSH 共享会话不可用。请在交互式 SSH 终端中完成登录，然后再次挂载。".into()
        }
        ServiceError::InteractiveSsh(InteractiveSshError::UnsupportedWindowsSshConfig) => {
            "此 SSH config 尚未解析出可用于 Windows 交互式 SSH 的 HostName、User 和别名。".into()
        }
        ServiceError::InteractiveSsh(InteractiveSshError::UnsupportedWindowsSshProxy) => {
            "交互式共享 SSH 不能绕过 SSH config 中的 ProxyJump 或 ProxyCommand；请使用 OpenSSH 传输。".into()
        }
        ServiceError::InteractiveSsh(InteractiveSshError::PlinkMissing) => {
            "当前 Windows 程序包缺少已校验的 Plink。".into()
        }
        ServiceError::InteractiveSsh(InteractiveSshError::OpenSshMissing) => {
            "未找到 OpenSSH。".into()
        }
        _ => error.to_string(),
    }
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
        DraftError::InvalidName => "名称不能包含控制字符".into(),
        DraftError::InvalidFolder => "文件夹不能包含控制字符".into(),
        DraftError::TooManyTags(limit) => format!("一个连接最多只能有 {limit} 个标签"),
        DraftError::TagTooLong(limit) => format!("标签最多只能有 {limit} 个字符"),
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
        "SSH config file" => "SSH 配置文件",
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
                existed: false,
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
    file.take(LOG_VIEW_LIMIT)
        .read_to_end(&mut bytes)
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
        existed: true,
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

struct ManagedProfileSnapshot {
    path: PathBuf,
    content: Option<Vec<u8>>,
}

fn capture_managed_profile(
    previous: Option<&ServerConfig>,
) -> Result<Option<ManagedProfileSnapshot>, String> {
    let Some(previous) = previous.filter(|server| server.ssh_config_managed) else {
        return Ok(None);
    };
    let path = PathBuf::from(previous.managed_ssh_config_path.trim());
    if path.as_os_str().is_empty() {
        return Ok(None);
    }
    let content = match fs::read(&path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    Ok(Some(ManagedProfileSnapshot { path, content }))
}

fn rollback_prepared_managed_profile(
    prepared: &ServerConfig,
    previous: Option<&ManagedProfileSnapshot>,
) -> Result<(), String> {
    if !prepared.ssh_config_managed {
        return Ok(());
    }
    let prepared_path = PathBuf::from(prepared.managed_ssh_config_path.trim());
    if let Some(previous) = previous.filter(|snapshot| snapshot.path == prepared_path)
        && let Some(content) = &previous.content
    {
        return storage::atomic_write(&previous.path, content).map_err(|error| error.to_string());
    }
    remove_managed_ssh_server(prepared).map_err(|error| error.to_string())
}

struct PreparedSecret {
    kind: CredentialKind,
    obscured: Option<String>,
    credential: Option<String>,
    change: Option<CredentialChange>,
}

impl PreparedSecret {
    fn apply(&self, server: &mut ServerConfig) {
        let Some(reference) = &self.credential else {
            return;
        };
        match self.kind {
            CredentialKind::Password => {
                server.password_credential.clone_from(reference);
            }
            CredentialKind::KeyPassphrase => {
                server.key_pass_credential.clone_from(reference);
            }
        }
    }
}

fn prepare_secret_action(
    service: &MountService,
    storage: CredentialStorage,
    server_id: &str,
    kind: CredentialKind,
    action: &SecretAction,
) -> Result<PreparedSecret, String> {
    let SecretAction::Obscure(secret) = action else {
        return Ok(PreparedSecret {
            kind,
            obscured: None,
            credential: None,
            change: None,
        });
    };
    let obscured = service
        .obscure_secret(secret)
        .map_err(|error| error.to_string())?;
    if storage == CredentialStorage::Obscure {
        return Ok(PreparedSecret {
            kind,
            obscured: Some(obscured),
            credential: None,
            change: None,
        });
    }
    let reference = credential_reference(server_id, kind);
    let change = replace_verified(&SystemCredentialStore, &reference, secret)
        .map_err(|error| error.to_string())?;
    Ok(PreparedSecret {
        kind,
        obscured: Some(obscured),
        credential: Some(reference),
        change: Some(change),
    })
}

fn rollback_prepared_secret(secret: &PreparedSecret) -> Result<(), String> {
    if let Some(change) = &secret.change {
        rollback_change(&SystemCredentialStore, change).map_err(|error| error.to_string())
    } else {
        Ok(())
    }
}

fn rollback_prepared_secrets<'a>(
    secrets: impl IntoIterator<Item = &'a PreparedSecret>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for secret in secrets {
        if let Err(error) = rollback_prepared_secret(secret) {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

struct CredentialStorageMigration {
    migrations: Vec<CredentialMigration>,
    retired_references: Vec<String>,
}

impl CredentialStorageMigration {
    fn retire_system_references(&self) -> Result<(), String> {
        delete_credential_references(&SystemCredentialStore, &self.retired_references)
            .map_err(|error| error.to_string())
    }
}

fn rollback_credential_storage_change(
    paths: &AppPaths,
    previous_servers: &[ServerConfig],
    migration: Option<&CredentialStorageMigration>,
    message: String,
) -> String {
    let Some(migration) = migration else {
        return message;
    };
    rollback_migrations_after_persistence(paths, previous_servers, &migration.migrations, message)
}

fn credential_presence_summary(stage: &str, servers: &[ServerConfig]) -> String {
    let records = servers
        .iter()
        .map(|server| {
            format!(
                "{}[password_obscured={},password_reference={},key_pass_obscured={},key_pass_reference={}]",
                server.id,
                !server.password_obscured.is_empty(),
                !server.password_credential.is_empty(),
                !server.key_pass_obscured.is_empty(),
                !server.key_pass_credential.is_empty(),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("credential migration {stage}: {records}")
}

fn rollback_migrations_after_persistence(
    paths: &AppPaths,
    previous_servers: &[ServerConfig],
    migrations: &[CredentialMigration],
    mut message: String,
) -> String {
    match storage::save_servers(paths, previous_servers) {
        Ok(()) => {
            for migration in migrations.iter().rev() {
                if let Err(rollback) = migration.rollback(&SystemCredentialStore) {
                    message.push_str(&format!("; credential rollback failed: {rollback}"));
                }
            }
        }
        Err(rollback) => {
            // Keep the verified system representation when the server record
            // could not be restored; deleting it here could remove the last
            // usable copy referenced by the persisted record.
            message.push_str(&format!("; server rollback failed: {rollback}"));
        }
    }
    message
}

fn migrate_servers_for_storage(
    paths: &AppPaths,
    service: &MountService,
    servers: &[ServerConfig],
    target: CredentialStorage,
) -> Result<CredentialStorageMigration, String> {
    let mut migrations = Vec::with_capacity(servers.len());
    for server in servers {
        let result = match target {
            CredentialStorage::System => {
                prepare_server_to_system(server, &SystemCredentialStore, |obscured| {
                    service
                        .reveal_secret(obscured)
                        .map_err(|error| CredentialError::Reveal(error.to_string()))
                })
            }
            CredentialStorage::Obscure => {
                prepare_server_to_obscure(server, &SystemCredentialStore, |secret| {
                    service
                        .obscure_secret(secret)
                        .map_err(|error| CredentialError::Obscure(error.to_string()))
                })
            }
        };
        match result {
            Ok(migration) => migrations.push(migration),
            Err(error) => {
                let mut message = error.to_string();
                for migration in migrations.iter().rev() {
                    if let Err(rollback) = migration.rollback(&SystemCredentialStore) {
                        message.push_str(&format!("; credential rollback failed: {rollback}"));
                    }
                }
                return Err(message);
            }
        }
    }
    let candidates = migrations
        .iter()
        .map(|migration| migration.candidate().clone())
        .collect::<Vec<_>>();
    let persisted_candidates =
        persist_and_reload_servers(paths, &candidates, servers, &migrations)?;
    let mut finalized = Vec::with_capacity(migrations.len());
    let mut retired_references = Vec::new();
    for migration in &migrations {
        let Some(persisted) = persisted_candidates
            .iter()
            .find(|server| server.id == migration.candidate().id)
        else {
            return Err(rollback_migrations_after_persistence(
                paths,
                servers,
                &migrations,
                "credential migration persistence verification failed".into(),
            ));
        };
        let finalized_server = match target {
            CredentialStorage::System => migration
                .finalize_to_system(persisted)
                .map(|server| (server, Vec::new())),
            CredentialStorage::Obscure => migration.finalize_to_obscure(persisted).map(|commit| {
                let retired = commit.retired_references().to_vec();
                (commit.into_server(), retired)
            }),
        };
        match finalized_server {
            Ok((server, retired)) => {
                finalized.push(server);
                retired_references.extend(retired);
            }
            Err(error) => {
                return Err(rollback_migrations_after_persistence(
                    paths,
                    servers,
                    &migrations,
                    error.to_string(),
                ));
            }
        }
    }
    persist_and_reload_servers(paths, &finalized, servers, &migrations)?;
    Ok(CredentialStorageMigration {
        migrations,
        retired_references,
    })
}

fn persist_and_reload_servers(
    paths: &AppPaths,
    candidate: &[ServerConfig],
    original: &[ServerConfig],
    migrations: &[CredentialMigration],
) -> Result<Vec<ServerConfig>, String> {
    let result = storage::save_servers(paths, candidate)
        .map_err(|error| error.to_string())
        .and_then(|_| storage::load_servers(paths).map_err(|error| error.to_string()))
        .and_then(|persisted| {
            (persisted == candidate)
                .then_some(persisted)
                .ok_or_else(|| "credential migration persistence verification failed".to_owned())
        });
    match result {
        Ok(persisted) => Ok(persisted),
        Err(error) => Err(rollback_migrations_after_persistence(
            paths, original, migrations, error,
        )),
    }
}

fn delete_retired_connection_credentials(
    previous: &ServerConfig,
    current: &ServerConfig,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for (old_reference, new_reference) in [
        (&previous.password_credential, &current.password_credential),
        (&previous.key_pass_credential, &current.key_pass_credential),
    ] {
        if !old_reference.is_empty()
            && old_reference != new_reference
            && let Err(error) = SystemCredentialStore.delete(old_reference)
        {
            errors.push(error.to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
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

fn status_completion_message(
    policy: StatusPublishPolicy,
    errors: &[String],
    startup_notice: &mut Option<String>,
    locale: Locale,
    can_publish: bool,
) -> Option<String> {
    if !can_publish {
        return None;
    }
    match policy {
        StatusPublishPolicy::Silent => None,
        StatusPublishPolicy::UserRefresh => Some(
            errors
                .first()
                .cloned()
                .unwrap_or_else(|| locale.text(TextKey::Ready).into()),
        ),
        StatusPublishPolicy::Initial => {
            let notice = startup_notice.take();
            match (notice, errors.first()) {
                (Some(notice), Some(error)) => Some(format!("{notice}; {error}")),
                (Some(notice), None) => Some(notice),
                (None, Some(error)) => Some(error.clone()),
                (None, None) => Some(locale.text(TextKey::Ready).into()),
            }
        }
    }
}

fn status_publication_is_current(
    generation: u64,
    current_generation: u64,
    expected_status: &str,
    current_status: &str,
) -> bool {
    generation == current_generation && expected_status == current_status
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
    synced_polls: u8,
) -> bool {
    transfer_unavailable || !snapshot.is_some_and(|snapshot| snapshot.synced) || synced_polls < 2
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

fn automatic_install_blocked_message(locale: Locale, error: &UpdateTrustError) -> String {
    match locale {
        Locale::English => format!("Automatic installation blocked: {error}"),
        Locale::Chinese => format!("自动安装已被签名验证阻止：{error}"),
    }
}

fn main_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(1120.0, 800.0),
        min_size: Some(Size::new(760.0, 560.0)),
        position: window::Position::Centered,
        exit_on_close_request: false,
        ..window::Settings::default()
    }
}

fn log_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(980.0, 680.0),
        min_size: Some(Size::new(680.0, 480.0)),
        position: window::Position::Centered,
        exit_on_close_request: false,
        ..window::Settings::default()
    }
}

fn terminal_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(1080.0, 740.0),
        min_size: Some(Size::new(720.0, 520.0)),
        position: window::Position::Centered,
        exit_on_close_request: false,
        ..window::Settings::default()
    }
}

fn strict_terminal_command(
    command: &InteractiveSshLoginCommand,
) -> Result<(String, Vec<String>), String> {
    let program = strict_terminal_text(command.program().as_os_str(), "executable path")?;
    let mut arguments = Vec::with_capacity(command.arguments().len());
    for argument in command.arguments() {
        arguments.push(strict_terminal_text(argument, "argument")?);
    }
    Ok((program, arguments))
}

fn strict_terminal_text(value: &OsStr, kind: &str) -> Result<String, String> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("interactive SSH {kind} is not valid Unicode"))
}

fn interactive_mount_resume_once(queued: bool, resumed: bool, ready: bool) -> bool {
    queued && !resumed && ready
}

fn interactive_readiness_result_is_current(
    lifecycle: InteractiveTerminalLifecycle,
    queued_mount: bool,
) -> bool {
    lifecycle == InteractiveTerminalLifecycle::Starting && queued_mount
}

fn interactive_mount_poll_eligible(id: &str, queued: bool, saving_id: Option<&str>) -> bool {
    queued && saving_id != Some(id)
}

fn interactive_terminal_is_live(lifecycle: InteractiveTerminalLifecycle) -> bool {
    matches!(
        lifecycle,
        InteractiveTerminalLifecycle::Starting | InteractiveTerminalLifecycle::Ready
    )
}

fn interactive_session_can_restart_or_end(status: Option<MountStatus>) -> bool {
    !matches!(status, Some(MountStatus::Mounted | MountStatus::Starting))
}

fn interactive_terminal_error<'a>(
    error: &'a Option<(String, String)>,
    server_id: &str,
) -> Option<&'a str> {
    error
        .as_ref()
        .filter(|(error_id, _)| error_id == server_id)
        .map(|(_, message)| message.as_str())
}

fn interactive_session_config_compatible(previous: &ServerConfig, next: &ServerConfig) -> bool {
    previous == next && next.connection_method == ConnectionMethod::Interactive
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
    let child = command.spawn().map_err(|error| match locale {
        Locale::English => format!("Could not open {}: {error}", path.display()),
        Locale::Chinese => format!("无法打开 {}：{error}", path.display()),
    })?;
    #[cfg(not(windows))]
    {
        let mut child = child;
        child.wait().map_err(|error| match locale {
            Locale::English => format!("Could not finish opening {}: {error}", path.display()),
            Locale::Chinese => format!("无法完成打开 {}：{error}", path.display()),
        })?;
    }
    #[cfg(windows)]
    drop(child);
    Ok(())
}

#[cfg(test)]
mod localization_tests {
    use super::*;

    #[test]
    fn mount_error_summary_is_bounded_to_two_lines_and_compact_content() {
        let cause = "first detail\nsecond detail\nthird detail that should stay in the log";
        let summary = mount_error_summary(Locale::English, cause);
        assert_eq!(summary.lines().count(), MOUNT_ERROR_SUMMARY_MAX_LINES);
        assert!(summary.ends_with('…'));
        assert!(summary.chars().count() <= MOUNT_ERROR_SUMMARY_MAX_CHARS);
        assert!(summary.contains("first detail"));
        assert!(summary.contains("second detail"));
        assert!(!summary.contains("third detail"));

        let long_line = "x".repeat(MOUNT_ERROR_SUMMARY_LINE_MAX_CHARS + 40);
        let summary = mount_error_summary(Locale::Chinese, &long_line);
        assert_eq!(summary.lines().count(), 1);
        assert!(summary.chars().count() <= MOUNT_ERROR_SUMMARY_MAX_CHARS);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn mount_error_buttons_route_to_durable_actions() {
        let retry = mount_error_message(
            "alpha".into(),
            MountOperation::Mount,
            MountErrorAction::Retry,
        );
        assert!(matches!(
            retry,
            Message::RetryOperation(id, MountOperation::Mount) if id == "alpha"
        ));

        let view_log = mount_error_message(
            "alpha".into(),
            MountOperation::Unmount,
            MountErrorAction::ViewLog,
        );
        assert!(matches!(view_log, Message::OpenOperationLog(id) if id == "alpha"));

        let dismiss = mount_error_message(
            "alpha".into(),
            MountOperation::Unmount,
            MountErrorAction::Dismiss,
        );
        assert!(matches!(dismiss, Message::DismissOperationError(id) if id == "alpha"));
    }

    #[test]
    fn silent_status_completion_cannot_overwrite_a_mount_failure() {
        let mut startup_notice = Some("Recovered settings".into());

        let message = status_completion_message(
            StatusPublishPolicy::Silent,
            &[],
            &mut startup_notice,
            Locale::English,
            true,
        );

        assert_eq!(message, None);
        assert_eq!(startup_notice.as_deref(), Some("Recovered settings"));
    }

    #[test]
    fn initial_and_user_status_completions_publish_intentionally() {
        let mut startup_notice = Some("Recovered settings".into());
        assert_eq!(
            status_completion_message(
                StatusPublishPolicy::Initial,
                &[],
                &mut startup_notice,
                Locale::English,
                true,
            ),
            Some("Recovered settings".into())
        );
        assert!(startup_notice.is_none());
        assert_eq!(
            status_completion_message(
                StatusPublishPolicy::UserRefresh,
                &[],
                &mut startup_notice,
                Locale::English,
                true,
            ),
            Some("Ready".into())
        );
    }

    #[test]
    fn stale_status_completion_cannot_overwrite_a_newer_failure() {
        let mut startup_notice = Some("Recovered settings".into());

        let message = status_completion_message(
            StatusPublishPolicy::Initial,
            &[],
            &mut startup_notice,
            Locale::English,
            false,
        );

        assert_eq!(message, None);
        assert_eq!(startup_notice.as_deref(), Some("Recovered settings"));
    }

    #[test]
    fn repeated_status_text_does_not_make_an_older_generation_current() {
        assert!(!status_publication_is_current(
            1,
            2,
            "Refreshing",
            "Refreshing",
        ));
        assert!(status_publication_is_current(
            2,
            2,
            "Refreshing",
            "Refreshing",
        ));
    }

    #[test]
    fn unsigned_updates_have_explicit_bilingual_block_reasons() {
        let error = UpdateTrustError::MissingSignature;
        let english = automatic_install_blocked_message(Locale::English, &error);
        let chinese = automatic_install_blocked_message(Locale::Chinese, &error);
        assert!(english.contains("Automatic installation blocked"));
        assert!(english.contains("signature is missing"));
        assert!(chinese.contains("自动安装已被签名验证阻止"));
        assert!(chinese.contains("signature is missing"));
    }

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
            SettingKind::Transfers,
        ] {
            assert!(!setting_help(kind, Locale::English).is_empty());
            assert!(!setting_help(kind, Locale::Chinese).is_empty());
        }
        assert!(custom_units(SettingKind::MaxSize).contains(&"Gi"));
        assert!(custom_units(SettingKind::MaxAge).contains(&"m"));
        assert!(custom_units(SettingKind::Transfers).is_empty());
    }

    #[test]
    fn platform_settings_are_hidden_outside_supported_desktop_targets() {
        assert!(file_manager_settings_visible("windows"));
        assert!(file_manager_settings_visible("linux"));
        assert!(file_manager_settings_visible("macos"));
        assert!(!file_manager_settings_visible("freebsd"));
        assert!(!file_manager_settings_visible("android"));
        assert!(mount_backend_settings_visible("macos"));
        assert!(!mount_backend_settings_visible("windows"));
        assert!(!mount_backend_settings_visible("linux"));
    }

    #[test]
    fn nfs_ui_copy_explains_experimental_loopback_and_semantic_differences() {
        for locale in [Locale::English, Locale::Chinese] {
            let label = locale.mount_backend(MountBackend::Nfs);
            let help = mount_backend_help(locale);
            assert!(label.contains("NFS"));
            assert!(help.contains("NFS"));
            assert!(help.contains("FUSE"));
        }
        let english = mount_backend_help(Locale::English);
        assert!(english.contains("loopback-only"));
        assert!(english.contains("Experimental"));
        assert!(english.contains("next mount"));
        let chinese = mount_backend_help(Locale::Chinese);
        assert!(chinese.contains("回环"));
        assert!(chinese.contains("实验"));
        assert!(chinese.contains("下一次挂载"));
    }

    #[test]
    fn credential_ui_keeps_obscure_as_default_and_explains_verified_opt_in() {
        assert_eq!(
            Settings::default().credential_storage,
            CredentialStorage::Obscure
        );
        for locale in [Locale::English, Locale::Chinese] {
            let help = credential_storage_help(locale);
            let confirmation = credential_storage_confirmation(locale, CredentialStorage::System);
            assert!(help.contains("rclone obscure"));
            assert!(confirmation.contains("SSH MountMate"));
        }
        assert!(credential_storage_help(Locale::English).contains("write-and-read verification"));
        assert!(credential_storage_help(Locale::Chinese).contains("回读验证"));
        assert!(
            credential_storage_confirmation(Locale::English, CredentialStorage::System)
                .contains("compatibility copy")
        );
        assert!(
            credential_storage_confirmation(Locale::Chinese, CredentialStorage::System)
                .contains("兼容副本")
        );
    }

    #[test]
    fn credential_migration_diagnostics_include_only_presence_state() {
        let server = ServerConfig {
            id: "alpha".into(),
            password_obscured: "password-secret-sentinel".into(),
            key_pass_credential: "credential-secret-sentinel".into(),
            ..ServerConfig::default()
        };
        let summary = credential_presence_summary("before", &[server]);
        assert!(summary.contains("password_obscured=true"));
        assert!(summary.contains("password_reference=false"));
        assert!(summary.contains("key_pass_reference=true"));
        assert!(!summary.contains("password-secret-sentinel"));
        assert!(!summary.contains("credential-secret-sentinel"));
    }

    #[test]
    fn system_secret_replacement_retains_the_obscured_compatibility_copy() {
        let mut server = ServerConfig {
            password_obscured: "obscured-password".into(),
            key_pass_obscured: "obscured-passphrase".into(),
            ..ServerConfig::default()
        };
        PreparedSecret {
            kind: CredentialKind::Password,
            obscured: Some("obscured-password".into()),
            credential: Some("password-reference".into()),
            change: None,
        }
        .apply(&mut server);
        PreparedSecret {
            kind: CredentialKind::KeyPassphrase,
            obscured: Some("obscured-passphrase".into()),
            credential: Some("passphrase-reference".into()),
            change: None,
        }
        .apply(&mut server);

        assert_eq!(server.password_obscured, "obscured-password");
        assert_eq!(server.key_pass_obscured, "obscured-passphrase");
        assert_eq!(server.password_credential, "password-reference");
        assert_eq!(server.key_pass_credential, "passphrase-reference");
    }

    #[test]
    fn windows_allows_resolved_config_profiles_to_use_interactive_transport() {
        assert!(ConnectionMethod::ALL.contains(&ConnectionMethod::Interactive));
    }

    #[test]
    fn mounted_connection_settings_copy_is_read_only_and_never_reveals_secrets() {
        assert_eq!(Locale::English.text(TextKey::Edit), "Settings");
        assert_eq!(Locale::Chinese.text(TextKey::Edit), "设置");
        assert!(connection_settings_locked_help(Locale::English).contains("read-only"));
        assert!(connection_settings_locked_help(Locale::Chinese).contains("只读"));
        assert_eq!(
            connection_secret_state_label(Locale::English, PreservedSecretState::System),
            "Stored in system credentials"
        );
        assert_eq!(
            connection_secret_state_label(Locale::Chinese, PreservedSecretState::Obscured),
            "已使用 rclone obscure 保存"
        );
        assert_eq!(localized_yes_no(Locale::English, true), "Yes");
        assert_eq!(localized_yes_no(Locale::Chinese, false), "否");
    }

    #[test]
    fn read_only_settings_use_the_same_mountpoint_display_rules_as_cards() {
        let mut draft = ConnectionDraft::default();
        assert_eq!(
            display_draft_mountpoint(&draft, Locale::English),
            Locale::English.text(TextKey::AutoMountpoint)
        );
        draft.mountpoint = HOME_MOUNTPOINT_VALUE.into();
        assert_eq!(
            display_draft_mountpoint(&draft, Locale::Chinese),
            Locale::Chinese.text(TextKey::AutoMountpoint)
        );
        draft.mountpoint = "Z:".into();
        assert_eq!(display_draft_mountpoint(&draft, Locale::English), "Z:");
    }

    #[test]
    fn openssh_command_preview_quotes_unsafe_arguments_without_interpreting_them() {
        assert_eq!(
            openssh_command_preview("C:\\Users\\A B\\.ssh\\config", "host alias"),
            "ssh -F \"C:\\\\Users\\\\A B\\\\.ssh\\\\config\" \"host alias\""
        );
        assert_eq!(
            openssh_command_preview("C:\\Users\\me\\.ssh\\config", "host-a"),
            "ssh -F C:\\Users\\me\\.ssh\\config host-a"
        );
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
        assert!(!empty.existed);

        let path = temp.path().join("large.log");
        let mut bytes = vec![b'x'; LOG_VIEW_LIMIT as usize + 32];
        bytes.extend_from_slice(b"\nfinal log line\n");
        std::fs::write(&path, bytes).unwrap();
        let loaded = read_mount_log(path.clone()).unwrap();
        assert_eq!(loaded.path, path);
        assert!(loaded.truncated);
        assert!(loaded.existed);
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
    fn managed_profile_rollback_restores_the_previous_contents() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("alpha.conf");
        fs::write(&path, b"old profile").unwrap();
        let prepared = ServerConfig {
            ssh_config_managed: true,
            managed_ssh_config_path: path.display().to_string(),
            ..ServerConfig::default()
        };
        let snapshot = ManagedProfileSnapshot {
            path: path.clone(),
            content: Some(b"old profile".to_vec()),
        };
        fs::write(&path, b"new profile").unwrap();

        rollback_prepared_managed_profile(&prepared, Some(&snapshot)).unwrap();

        assert_eq!(fs::read(path).unwrap(), b"old profile");
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
        assert_eq!(setting_presets(SettingKind::Transfers), &["4", "8", "12"]);
        assert_eq!(
            split_custom_setting(SettingKind::MaxSize, "250Gi"),
            ("250".into(), "Gi".into())
        );
        assert_eq!(
            split_custom_setting(SettingKind::MaxAge, "1h30m"),
            ("1h30m".into(), String::new())
        );
        assert_eq!(
            split_custom_setting(SettingKind::Transfers, "24"),
            ("24".into(), String::new())
        );
        assert!(!custom_setting_is_supported(SettingKind::MaxAge, "1h30m"));
        assert!(!custom_setting_is_supported(SettingKind::MaxSize, "20G"));
        assert!(custom_setting_is_supported(SettingKind::MaxAge, "90m"));
        assert!(custom_setting_is_supported(SettingKind::MaxSize, "20Gi"));
    }

    #[test]
    fn custom_picker_labels_preserve_exact_values_in_both_locales() {
        let values = [
            (SettingKind::MaxSize, "20G"),
            (SettingKind::MaxAge, "1h30m"),
            (SettingKind::MinFreeSpace, "20G"),
            (SettingKind::WriteBack, "250ms"),
            (SettingKind::DirCacheTime, "2h30m"),
            (SettingKind::BufferSize, "20G"),
            (SettingKind::Transfers, "24"),
        ];
        for (kind, value) in values {
            for (locale, expected) in [
                (Locale::English, format!("Custom: {value}")),
                (Locale::Chinese, format!("自定义：{value}")),
            ] {
                let selected = selected_custom_setting_option(kind, value, locale);
                assert!(selected.custom);
                assert_eq!(selected.kind, kind);
                assert_eq!(selected.value, value);
                assert_eq!(selected.label, expected);
                assert_eq!(
                    setting_options(kind, locale)
                        .into_iter()
                        .find(|option| option.custom)
                        .unwrap()
                        .label,
                    match locale {
                        Locale::English => "Custom...",
                        Locale::Chinese => "自定义...",
                    }
                );
            }
        }
    }

    #[test]
    fn unsupported_custom_values_round_trip_without_coercion() {
        for (kind, value) in [
            (SettingKind::MaxAge, "1h30m"),
            (SettingKind::MaxSize, "20G"),
        ] {
            let draft = CustomSettingDraft {
                kind,
                digits: value.into(),
                unit: String::new(),
                raw_value: Some(value.into()),
            };
            assert_eq!(
                custom_setting_value(&draft, Locale::English),
                Ok(value.into())
            );
            assert_eq!(
                custom_setting_value(&draft, Locale::Chinese),
                Ok(value.into())
            );
        }
    }

    #[test]
    fn startup_selection_mounts_only_enabled_non_interactive_connections() {
        let servers = vec![
            ServerConfig {
                id: "enabled".into(),
                auto_mount_at_login: true,
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "disabled".into(),
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "interactive".into(),
                auto_mount_at_login: true,
                connection_method: ConnectionMethod::Interactive,
                ..ServerConfig::default()
            },
        ];

        let selected = startup_servers(&Settings::default(), &servers);

        assert_eq!(
            selected
                .into_iter()
                .map(|server| server.id)
                .collect::<Vec<_>>(),
            vec!["enabled".to_owned()]
        );
    }

    #[test]
    fn recovered_settings_detect_existing_system_credential_references() {
        let system_only = ServerConfig {
            password_credential: "ssh-mountmate:alpha:password".into(),
            ..ServerConfig::default()
        };
        assert!(servers_require_system_credentials(std::slice::from_ref(
            &system_only
        )));
        assert!(settings_need_system_credential_inference(
            &Settings::default(),
            std::slice::from_ref(&system_only),
        ));
        assert!(!settings_need_system_credential_inference(
            &Settings {
                credential_storage: CredentialStorage::System,
                ..Settings::default()
            },
            &[system_only],
        ));
        assert!(!servers_require_system_credentials(&[ServerConfig {
            password_obscured: "obscured".into(),
            password_credential: "retained-reference".into(),
            ..ServerConfig::default()
        }]));
    }

    #[test]
    fn recovery_dialog_names_exact_paths_permission_and_memory_defaults() {
        let recovered = storage::RecoveredSettings {
            settings: Settings {
                cache_root: PathBuf::from("/home/alice/.cache/rclone-gui"),
                ..Settings::default()
            },
            load_error: Some("invalid JSON at line 1".into()),
            backup_path: None,
            attempted_backup_path: Some(PathBuf::from(
                "/home/alice/.config/rclone-gui/settings.json.invalid.bak",
            )),
            backup_error: Some("Permission denied (os error 13)".into()),
            backup_error_kind: Some(std::io::ErrorKind::PermissionDenied),
            persistence_error: None,
            persistence_error_kind: None,
            cleanup_error: None,
            failure_stage: Some(storage::SettingsRecoveryStage::Backup),
            source_was_present: true,
            original_restored: false,
        };

        let message = settings_recovery_dialog_message(
            &recovered,
            Path::new("/home/alice/.config/rclone-gui/settings.json"),
            Locale::Chinese,
        )
        .unwrap();

        assert!(message.contains("/home/alice/.config/rclone-gui/settings.json"));
        assert!(message.contains("settings.json.invalid.bak"));
        assert!(message.contains("原设置路径"));
        assert!(message.contains("尝试备份路径"));
        assert!(message.contains("Permission denied (os error 13)"));
        assert!(message.contains("没有写权限"));
        assert!(message.contains("内存"));
        assert!(message.contains("/home/alice/.cache/rclone-gui"));
    }

    #[test]
    fn recovery_dialog_reports_write_restore_and_cleanup_errors_with_both_paths() {
        let recovered = storage::RecoveredSettings {
            settings: Settings {
                cache_root: PathBuf::from("C:/Users/alice/AppData/Local/SSH MountMate/cache"),
                ..Settings::default()
            },
            load_error: Some("expected value at line 1 column 1".into()),
            backup_path: Some(PathBuf::from(
                "C:/Users/alice/AppData/Roaming/SSH MountMate/settings.json.invalid.1.bak",
            )),
            attempted_backup_path: Some(PathBuf::from(
                "C:/Users/alice/AppData/Roaming/SSH MountMate/settings.json.invalid.1.bak",
            )),
            backup_error: Some("Access is denied while restoring (os error 5)".into()),
            backup_error_kind: Some(std::io::ErrorKind::PermissionDenied),
            persistence_error: Some("The process cannot access the file (os error 32)".into()),
            persistence_error_kind: Some(std::io::ErrorKind::PermissionDenied),
            cleanup_error: Some("replacement is still in use (os error 32)".into()),
            failure_stage: Some(storage::SettingsRecoveryStage::RestoreOriginal),
            source_was_present: true,
            original_restored: false,
        };

        let settings_path = Path::new("C:/Users/alice/AppData/Roaming/SSH MountMate/settings.json");
        let message =
            settings_recovery_dialog_message(&recovered, settings_path, Locale::English).unwrap();

        assert!(message.contains(&settings_path.display().to_string()));
        assert!(message.contains("Backup saved at:"));
        assert!(message.contains("settings.json.invalid.1.bak"));
        assert!(message.contains("expected value at line 1 column 1"));
        assert!(message.contains("The process cannot access the file (os error 32)"));
        assert!(message.contains("replacement is still in use (os error 32)"));
        assert!(message.contains("Access is denied while restoring (os error 5)"));
    }

    #[test]
    fn legacy_startup_all_selects_every_non_interactive_connection() {
        let settings = Settings {
            startup_all: true,
            ..Settings::default()
        };
        let servers = vec![
            ServerConfig {
                id: "native".into(),
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "interactive".into(),
                connection_method: ConnectionMethod::Interactive,
                ..ServerConfig::default()
            },
        ];

        assert_eq!(startup_servers(&settings, &servers)[0].id, "native");
        assert!(startup_integration_enabled(&settings, &servers));
    }

    #[test]
    fn legacy_startup_all_migrates_to_per_connection_preferences() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let settings = Settings {
            startup_all: true,
            cache_root: paths.cache_dir.clone(),
            ..Settings::default()
        };
        let servers = vec![ServerConfig {
            id: "alpha".into(),
            name: "alpha".into(),
            ..ServerConfig::default()
        }];
        storage::save_settings(&paths, &settings).unwrap();
        storage::save_servers(&paths, &servers).unwrap();

        let (migrated_settings, migrated_servers) =
            migrate_legacy_startup_preferences(&paths, &settings, &servers).unwrap();

        assert!(!migrated_settings.startup_all);
        assert!(migrated_servers[0].auto_mount_at_login);
        assert_eq!(storage::load_settings(&paths).unwrap(), migrated_settings);
        assert_eq!(storage::load_servers(&paths).unwrap(), migrated_servers);
    }

    #[test]
    fn legacy_startup_migration_never_runs_on_a_server_load_fallback() {
        let settings = Settings {
            startup_all: true,
            ..Settings::default()
        };

        assert!(!legacy_startup_migration_needed(&settings, false));
        assert!(legacy_startup_migration_needed(&settings, true));
    }

    #[test]
    fn upload_transfer_validation_accepts_only_the_supported_range() {
        for value in ["1", "4", "8", "12", "32"] {
            assert_eq!(
                validate_upload_transfers(value, Locale::English).unwrap(),
                value.parse::<u16>().unwrap()
            );
        }
        for value in ["", "0", "33", "-1", "4.5", "unlimited"] {
            assert!(validate_upload_transfers(value, Locale::English).is_err());
            assert!(validate_upload_transfers(value, Locale::Chinese).is_err());
        }
    }

    #[test]
    fn settings_draft_round_trips_upload_transfer_limit() {
        let original = Settings {
            cache_root: PathBuf::from("cache"),
            vfs_upload_transfers: 12,
            macos_mount_backend: MountBackend::Nfs,
            credential_storage: CredentialStorage::System,
            appearance_mode: AppearanceMode::Light,
            accent_color: AccentColor::Green,
            ..Settings::default()
        };
        let draft = SettingsDraft::from_settings(&original);
        assert_eq!(draft.upload_transfers, "12");
        assert_eq!(draft.mount_backend, MountBackend::Nfs);
        assert_eq!(draft.credential_storage, CredentialStorage::System);
        assert_eq!(draft.appearance_mode, AppearanceMode::Light);
        assert_eq!(draft.accent_color, AccentColor::Green);
        assert_eq!(draft.font_scale, FontScale::Standard);
        let rebuilt = draft.build(&original, Locale::English).unwrap();
        assert_eq!(rebuilt.vfs_upload_transfers, 12);
        assert_eq!(rebuilt.macos_mount_backend, MountBackend::Nfs);
        assert_eq!(rebuilt.credential_storage, CredentialStorage::System);
        assert_eq!(rebuilt.appearance_mode, AppearanceMode::Light);
        assert_eq!(rebuilt.accent_color, AccentColor::Green);
        assert_eq!(rebuilt.font_scale, FontScale::Standard);
    }

    #[test]
    fn fresh_profile_settings_can_be_saved_without_picking_a_cache_directory() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            data_dir: temp.path().join("data"),
        };
        let settings = storage::load_settings(&paths).unwrap();
        let draft = SettingsDraft::from_settings(&settings);

        assert_eq!(settings.cache_root, paths.cache_dir);
        assert_eq!(draft.build(&settings, Locale::English), Ok(settings));
    }

    #[test]
    fn appearance_choices_are_bilingual_and_generate_contrasting_palettes() {
        for locale in [Locale::English, Locale::Chinese] {
            for mode in AppearanceMode::ALL {
                assert!(!locale.appearance_mode(mode).is_empty());
            }
            for accent in AccentColor::ALL {
                assert!(!locale.accent_color(accent).is_empty());
            }
            for scale in FontScale::ALL {
                assert!(!locale.font_scale(scale).is_empty());
                assert!((0.9..=1.3).contains(&scale.factor()));
            }
        }

        let followed_light =
            application_theme(AppearanceMode::System, AccentColor::Amber, false).palette();
        assert_eq!(followed_light.background, Palette::LIGHT.background);

        for accent in AccentColor::ALL {
            for mode in [AppearanceMode::Light, AppearanceMode::Dark] {
                let palette = application_theme(mode, accent, false).palette();
                let expected_background = if mode == AppearanceMode::Light {
                    Palette::LIGHT.background
                } else {
                    Palette::DARK.background
                };
                assert_eq!(palette.background, expected_background);
                assert!(contrast_ratio(palette.text, palette.background) >= 4.5);
                assert!(contrast_ratio(palette.primary, palette.background) >= 3.0);
                assert!(
                    contrast_ratio(palette.primary, Color::BLACK)
                        .max(contrast_ratio(palette.primary, Color::WHITE))
                        >= 4.5
                );
            }
        }
    }

    #[test]
    fn appearance_preview_cancel_and_save_select_the_expected_source() {
        let persisted = Settings {
            cache_root: PathBuf::from("cache"),
            appearance_mode: AppearanceMode::Dark,
            accent_color: AccentColor::Blue,
            ..Settings::default()
        };
        assert_eq!(
            effective_appearance(&persisted, None),
            (AppearanceMode::Dark, AccentColor::Blue)
        );

        let mut draft = SettingsDraft::from_settings(&persisted);
        draft.appearance_mode = AppearanceMode::Light;
        draft.accent_color = AccentColor::Purple;
        assert_eq!(
            effective_appearance(&persisted, Some(&draft)),
            (AppearanceMode::Light, AccentColor::Purple)
        );

        // Cancel removes the draft, while a successful save replaces the persisted settings.
        assert_eq!(
            effective_appearance(&persisted, None),
            (AppearanceMode::Dark, AccentColor::Blue)
        );
        let saved = draft.build(&persisted, Locale::English).unwrap();
        assert_eq!(
            effective_appearance(&saved, None),
            (AppearanceMode::Light, AccentColor::Purple)
        );
    }

    fn contrast_ratio(first: Color, second: Color) -> f32 {
        let lighter = relative_luminance(first).max(relative_luminance(second));
        let darker = relative_luminance(first).min(relative_luminance(second));
        (lighter + 0.05) / (darker + 0.05)
    }

    fn relative_luminance(color: Color) -> f32 {
        fn channel(value: f32) -> f32 {
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
    }

    #[test]
    fn interactive_login_states_are_explained_in_chinese() {
        let missing = ServiceError::InteractiveSsh(InteractiveSshError::SessionMissing);
        assert!(localize_service_error(Locale::Chinese, &missing).contains("交互式 SSH 终端"));

        let unsupported =
            ServiceError::InteractiveSsh(InteractiveSshError::UnsupportedWindowsSshProxy);
        let message = localize_service_error(Locale::Chinese, &unsupported);
        assert!(message.contains("OpenSSH"));
        assert!(message.contains("ProxyJump"));
    }

    #[test]
    fn terminal_event_debug_never_exposes_payload_bytes() {
        let event = RedactedTerminalEvent(iced_term::Event::BackendCall(
            7,
            iced_term::BackendCommand::Write(b"super-secret-password".to_vec()),
        ));
        let debug = format!("{event:?}");
        assert_eq!(debug, "TerminalEvent(<redacted>)");
        assert!(!debug.contains("super-secret"));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_command_conversion_rejects_non_unicode_without_lossy_fallback() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let invalid = OsString::from_vec(vec![b's', b's', 0xff]);
        let error = strict_terminal_text(&invalid, "argument").unwrap_err();
        assert!(error.contains("not valid Unicode"));
        assert!(!error.contains("ff"));
    }

    #[test]
    fn queued_interactive_mount_resumes_once_after_readiness() {
        assert!(interactive_mount_resume_once(true, false, true));
        assert!(!interactive_mount_resume_once(true, true, true));
        assert!(!interactive_mount_resume_once(true, false, false));
        assert!(!interactive_mount_resume_once(false, false, true));
    }

    #[test]
    fn queued_interactive_mount_pauses_while_its_connection_is_saving() {
        assert!(!interactive_mount_poll_eligible(
            "editing",
            true,
            Some("editing")
        ));
        assert!(interactive_mount_poll_eligible(
            "other",
            true,
            Some("editing")
        ));
        assert!(interactive_mount_poll_eligible("editing", true, None));
        assert!(!interactive_mount_poll_eligible("editing", false, None));
    }

    #[test]
    fn interactive_session_lifecycle_and_config_changes_require_explicit_cleanup() {
        assert!(interactive_terminal_is_live(
            InteractiveTerminalLifecycle::Starting
        ));
        assert!(interactive_terminal_is_live(
            InteractiveTerminalLifecycle::Ready
        ));
        assert!(!interactive_terminal_is_live(
            InteractiveTerminalLifecycle::Exited
        ));

        let previous = ServerConfig {
            id: "interactive".into(),
            connection_method: ConnectionMethod::Interactive,
            host: "old.example".into(),
            ..ServerConfig::default()
        };
        assert!(interactive_session_config_compatible(&previous, &previous));

        let mut changed = previous.clone();
        changed.host = "new.example".into();
        assert!(!interactive_session_config_compatible(&previous, &changed));

        let mut native = previous.clone();
        native.connection_method = ConnectionMethod::Native;
        assert!(!interactive_session_config_compatible(&previous, &native));
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
    fn connection_tags_filter_without_overriding_saved_or_selected_sort() {
        let servers = vec![
            ServerConfig {
                id: "zeta".into(),
                name: "Zeta".into(),
                tags: vec!["Work".into(), "Shared".into()],
                host: "b.example".into(),
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "alpha".into(),
                name: "Alpha".into(),
                tags: vec!["Work".into()],
                host: "a.example".into(),
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "home".into(),
                name: "Home".into(),
                host: "home.example".into(),
                ..ServerConfig::default()
            },
        ];
        let filtered = visible_connections(&servers, "work", None, ConnectionSort::Name);
        assert_eq!(
            filtered
                .iter()
                .map(|server| server.id.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );

        let shared = visible_connections(&servers, "", Some("Shared"), ConnectionSort::SavedOrder);
        assert_eq!(
            shared.iter().map(|server| &server.id).collect::<Vec<_>>(),
            [&"zeta"]
        );

        let saved = visible_connections(&servers, "", None, ConnectionSort::SavedOrder);
        assert_eq!(
            saved
                .iter()
                .map(|server| server.id.as_str())
                .collect::<Vec<_>>(),
            ["zeta", "alpha", "home"]
        );
        let by_host = visible_connections(&servers, "", None, ConnectionSort::Host);
        assert_eq!(
            by_host
                .iter()
                .map(|server| server.id.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta", "home"]
        );
    }

    #[test]
    fn custom_order_moves_freely_across_tag_boundaries() {
        let servers = vec![
            ServerConfig {
                id: "tagged-a".into(),
                tags: vec!["Work".into()],
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "plain-a".into(),
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "tagged-b".into(),
                tags: vec!["Home".into()],
                ..ServerConfig::default()
            },
            ServerConfig {
                id: "plain-b".into(),
                ..ServerConfig::default()
            },
        ];

        assert_eq!(
            moved_connection_order(&servers, "tagged-b", -1).unwrap(),
            ["tagged-a", "tagged-b", "plain-a", "plain-b"]
        );
        assert!(!can_move_connection(&servers, "tagged-a", -1));
        assert!(can_move_connection(&servers, "plain-a", 1));
    }

    #[test]
    fn tag_input_trims_and_deduplicates_without_losing_unicode() {
        assert_eq!(
            parse_tag_input(" Work,研究，Work, Shared "),
            ["Work", "研究", "Shared"]
        );
        assert!(normalized_tag_name("bad\ntag", Locale::English).is_err());
        assert!(normalized_tag_name(&"界".repeat(MAX_TAG_CHARS + 1), Locale::English).is_err());
        assert!(
            validated_connection_tags(
                &(0..=MAX_CONNECTION_TAGS)
                    .map(|index| format!("tag-{index}"))
                    .collect::<Vec<_>>(),
                Locale::English,
            )
            .is_err()
        );
    }

    #[test]
    fn batch_tag_addition_accepts_existing_or_new_tag_with_new_input_taking_priority() {
        assert_eq!(
            batch_tag_to_add("", Some("Work"), Locale::English).unwrap(),
            "Work"
        );
        assert_eq!(
            batch_tag_to_add("  Research ", Some("Work"), Locale::English).unwrap(),
            "Research"
        );
        assert!(batch_tag_to_add("", None, Locale::English).is_err());
    }

    #[test]
    fn mountpoint_preflight_blocks_save_until_current_value_is_valid() {
        assert!(MountpointPreflight::NotRequired.allows_save());
        assert!(!MountpointPreflight::Checking("C:\\mount".into()).allows_save());
        assert!(MountpointPreflight::Valid("C:\\mount".into()).allows_save());
        assert!(
            !MountpointPreflight::Invalid {
                value: "Z:\\mount".into(),
                error: "remote volume".into(),
            }
            .allows_save()
        );
    }

    #[test]
    fn mountpoint_preflight_rejects_stale_generations_even_for_the_same_path() {
        assert!(mountpoint_preflight_result_is_current(
            3,
            3,
            "C:\\mount",
            Some("C:\\mount")
        ));
        assert!(!mountpoint_preflight_result_is_current(
            1,
            3,
            "C:\\mount",
            Some("C:\\mount")
        ));
        assert!(!mountpoint_preflight_result_is_current(
            3,
            3,
            "C:\\old",
            Some("C:\\mount")
        ));
    }

    #[test]
    fn clearing_custom_mountpoint_preserves_custom_selection() {
        let cleared = custom_mountpoint_draft_value("  ".into());
        assert_eq!(cleared, CUSTOM_MOUNTPOINT_PENDING);
        assert_eq!(mountpoint_choice(&cleared), "custom");

        let cached_custom = custom_mountpoint_value(&cleared);
        assert_eq!(cached_custom, "");
        assert_eq!(mountpoint_value_for_choice("auto", &cached_custom), "");
        assert_eq!(
            mountpoint_value_for_choice("custom", &cached_custom),
            CUSTOM_MOUNTPOINT_PENDING
        );
        assert_eq!(
            custom_mountpoint_draft_value("C:\\mount".into()),
            "C:\\mount"
        );
    }

    #[test]
    fn readiness_results_require_a_starting_queued_session() {
        assert!(interactive_readiness_result_is_current(
            InteractiveTerminalLifecycle::Starting,
            true
        ));
        assert!(!interactive_readiness_result_is_current(
            InteractiveTerminalLifecycle::Exited,
            true
        ));
        assert!(!interactive_readiness_result_is_current(
            InteractiveTerminalLifecycle::Failed,
            true
        ));
        assert!(!interactive_readiness_result_is_current(
            InteractiveTerminalLifecycle::Starting,
            false
        ));
    }

    #[test]
    fn mounted_or_starting_interactive_sessions_cannot_be_restarted_or_ended() {
        assert!(!interactive_session_can_restart_or_end(Some(
            MountStatus::Mounted
        )));
        assert!(!interactive_session_can_restart_or_end(Some(
            MountStatus::Starting
        )));
        assert!(interactive_session_can_restart_or_end(Some(
            MountStatus::Unmounted
        )));
        assert!(interactive_session_can_restart_or_end(None));
    }

    #[test]
    fn terminal_errors_are_visible_only_for_their_own_connection() {
        let error = Some(("failed".into(), "authentication failed".into()));
        assert_eq!(
            interactive_terminal_error(&error, "failed"),
            Some("authentication failed")
        );
        assert_eq!(interactive_terminal_error(&error, "mounted"), None);
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
        assert!(unmount_needs_confirmation(Some(&synced), false, 1));
        assert!(!unmount_needs_confirmation(Some(&synced), false, 2));
        assert!(unmount_needs_confirmation(Some(&synced), true, 2));
        assert!(unmount_needs_confirmation(None, false, 2));
        let mut pending = synced.clone();
        pending.synced = false;
        pending.queued = 1;
        assert!(unmount_needs_confirmation(Some(&pending), false, 2));
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
