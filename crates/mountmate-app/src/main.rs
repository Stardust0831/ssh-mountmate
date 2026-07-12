use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use iced::widget::{Space, button, column, container, progress_bar, row, scrollable, text};
use iced::{Center, Element, Fill, Point, Size, Subscription, Task, Theme, window};
use mountmate_core::paths::AppPaths;
use mountmate_core::process::MountStatus;
use mountmate_core::service::MountService;
use mountmate_core::storage::{self, read_json};
use mountmate_core::transfer::TransferSnapshot;
use mountmate_core::{APP_NAME, MountState, ServerConfig, Settings, VERSION};

fn main() -> iced::Result {
    iced::daemon(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .run()
}

#[derive(Debug)]
struct App {
    paths: AppPaths,
    settings: Settings,
    servers: Vec<ServerConfig>,
    service: MountService,
    mount_statuses: HashMap<String, MountStatus>,
    busy: HashSet<String>,
    transfers: HashMap<String, TransferSnapshot>,
    transfer_errors: HashMap<String, String>,
    transfer_refreshing: bool,
    main_window: window::Id,
    popup_windows: HashMap<window::Id, String>,
    popup_order: Vec<window::Id>,
    dismissed_popups: HashSet<String>,
    synced_polls: HashMap<String, u8>,
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    Refresh,
    StatusesLoaded(Vec<(String, Result<MountStatus, String>)>),
    TransferTick,
    TransfersLoaded(Vec<(String, Result<TransferSnapshot, String>)>),
    PopupOpened(window::Id),
    ClosePopup(window::Id),
    WindowClosed(window::Id),
    AddConnection,
    OpenSettings,
    Mount(String),
    MountFinished {
        id: String,
        operation: MountOperation,
        result: Result<String, String>,
    },
    Open(String),
    OpenFinished(Result<(), String>),
    Edit(String),
    Remove(String),
}

#[derive(Debug, Clone, Copy)]
enum MountOperation {
    Mount,
    Unmount,
}

impl App {
    fn title(&self, window: window::Id) -> String {
        if window == self.main_window {
            format!("{APP_NAME} {VERSION}")
        } else {
            "File transfer".into()
        }
    }

    fn theme(&self, _window: window::Id) -> Theme {
        Theme::Dark
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_secs(1)).map(|_| Message::TransferTick),
            window::close_events().map(Message::WindowClosed),
        ])
    }

    fn new() -> (Self, Task<Message>) {
        let paths = AppPaths::discover();
        let service = MountService::new(paths.clone(), application_root());
        let settings = storage::load_settings(&paths).unwrap_or_default();
        let (servers, status) = match storage::load_servers(&paths) {
            Ok(servers) => (servers, "Loading mount status...".into()),
            Err(error) => (
                Vec::new(),
                format!("Could not load existing configuration: {error}"),
            ),
        };
        let (main_window, open_window) = window::open(main_window_settings());
        let app = Self {
            paths,
            settings,
            servers,
            service,
            mount_statuses: HashMap::new(),
            busy: HashSet::new(),
            transfers: HashMap::new(),
            transfer_errors: HashMap::new(),
            transfer_refreshing: false,
            main_window,
            popup_windows: HashMap::new(),
            popup_order: Vec::new(),
            dismissed_popups: HashSet::new(),
            synced_polls: HashMap::new(),
            status,
        };
        let task = Task::batch([open_window.discard(), app.status_task()]);
        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Refresh => match storage::load_servers(&self.paths) {
                Ok(servers) => {
                    self.servers = servers;
                    self.status = "Refreshing mount status...".into();
                    return self.status_task();
                }
                Err(error) => self.status = error.to_string(),
            },
            Message::StatusesLoaded(results) => {
                let mut errors = Vec::new();
                for (id, result) in results {
                    match result {
                        Ok(status) => {
                            self.mount_statuses.insert(id, status);
                        }
                        Err(error) => errors.push(error),
                    }
                }
                self.status = errors.first().cloned().unwrap_or_else(|| "Ready".into());
            }
            Message::TransferTick => return self.transfer_task(),
            Message::TransfersLoaded(results) => {
                self.transfer_refreshing = false;
                for (id, result) in results {
                    match result {
                        Ok(snapshot) => {
                            if snapshot.synced {
                                let polls = self.synced_polls.entry(id.clone()).or_default();
                                *polls = polls.saturating_add(1);
                            } else {
                                self.synced_polls.remove(&id);
                            }
                            self.transfers.insert(id.clone(), snapshot);
                            self.transfer_errors.remove(&id);
                        }
                        Err(error) => {
                            self.transfer_errors.insert(id, error);
                        }
                    }
                }
                return self.reconcile_transfer_popups();
            }
            Message::PopupOpened(id) => {
                let index = self
                    .popup_order
                    .iter()
                    .position(|popup| *popup == id)
                    .unwrap_or(0);
                return configure_popup_window(id, index);
            }
            Message::ClosePopup(id) => {
                if let Some(server_id) = self.popup_windows.remove(&id) {
                    self.dismissed_popups.insert(server_id);
                }
                self.popup_order.retain(|popup| *popup != id);
                return window::close(id);
            }
            Message::WindowClosed(id) if id == self.main_window => return iced::exit(),
            Message::WindowClosed(id) => {
                if let Some(server_id) = self.popup_windows.remove(&id) {
                    self.dismissed_popups.insert(server_id);
                }
                self.popup_order.retain(|popup| *popup != id);
            }
            Message::AddConnection => {
                self.status = "Connection editor is the next implemented surface".into()
            }
            Message::OpenSettings => {
                self.status = format!("Cache profile: {}", self.settings.vfs_cache_mode)
            }
            Message::Mount(id) => return self.start_mount_operation(id),
            Message::MountFinished {
                id,
                operation,
                result,
            } => {
                self.busy.remove(&id);
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
                            return self.close_popups_for_server(&id);
                        }
                    }
                    Err(error) => {
                        self.status = error;
                        return self.status_task();
                    }
                }
            }
            Message::Open(id) => return self.open_mountpoint(id),
            Message::OpenFinished(result) => match result {
                Ok(()) => self.status = "Opened mountpoint".into(),
                Err(error) => self.status = error,
            },
            Message::Edit(id) => self.status = format!("Edit requested for {id}"),
            Message::Remove(id) => self.status = format!("Remove requested for {id}"),
        }
        Task::none()
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
            return Task::none();
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
        let active: HashSet<_> = self
            .transfers
            .iter()
            .filter(|(id, snapshot)| {
                transfer_is_active(snapshot)
                    && self.mount_statuses.get(*id) == Some(&MountStatus::Mounted)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut tasks = Vec::new();

        let completed_windows: Vec<_> = self
            .popup_windows
            .iter()
            .filter(|(_, server_id)| {
                self.mount_statuses.get(*server_id) != Some(&MountStatus::Mounted)
                    || (!active.contains(*server_id)
                        && self.synced_polls.get(*server_id).copied().unwrap_or(0) >= 2)
            })
            .map(|(window, _)| *window)
            .collect();
        for popup in completed_windows {
            if let Some(server_id) = self.popup_windows.remove(&popup) {
                self.dismissed_popups.remove(&server_id);
            }
            self.popup_order.retain(|window| *window != popup);
            tasks.push(window::close(popup));
        }

        for server_id in self
            .synced_polls
            .iter()
            .filter(|(_, polls)| **polls >= 2)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
        {
            self.dismissed_popups.remove(&server_id);
        }

        if self.settings.auto_show_transfers {
            for server_id in active {
                let already_open = self
                    .popup_windows
                    .values()
                    .any(|existing| existing == &server_id);
                if already_open || self.dismissed_popups.contains(&server_id) {
                    continue;
                }
                let (popup, open) = window::open(transfer_window_settings());
                self.popup_windows.insert(popup, server_id);
                self.popup_order.push(popup);
                tasks.push(open.map(Message::PopupOpened));
            }
        }

        Task::batch(tasks)
    }

    fn close_popups_for_server(&mut self, server_id: &str) -> Task<Message> {
        let windows: Vec<_> = self
            .popup_windows
            .iter()
            .filter(|(_, existing)| existing.as_str() == server_id)
            .map(|(window, _)| *window)
            .collect();
        let mut tasks = Vec::new();
        for popup in windows {
            self.popup_windows.remove(&popup);
            self.popup_order.retain(|window| *window != popup);
            tasks.push(window::close(popup));
        }
        self.dismissed_popups.remove(server_id);
        self.transfers.remove(server_id);
        self.transfer_errors.remove(server_id);
        self.synced_polls.remove(server_id);
        Task::batch(tasks)
    }

    fn start_mount_operation(&mut self, id: String) -> Task<Message> {
        if !self.busy.insert(id.clone()) {
            return Task::none();
        }
        let mounted = matches!(
            self.mount_statuses.get(&id),
            Some(MountStatus::Mounted | MountStatus::Starting)
        );
        let operation = if mounted {
            MountOperation::Unmount
        } else {
            MountOperation::Mount
        };
        self.mount_statuses
            .insert(id.clone(), MountStatus::Starting);
        self.status = match operation {
            MountOperation::Mount => format!("Mounting {id}..."),
            MountOperation::Unmount => format!("Unmounting {id}..."),
        };
        let service = self.service.clone();
        let settings = self.settings.clone();
        let server = self.servers.iter().find(|server| server.id == id).cloned();
        let result_id = id.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || match operation {
                    MountOperation::Mount => {
                        let server =
                            server.ok_or_else(|| "Connection no longer exists".to_owned())?;
                        service
                            .mount(&server, &settings)
                            .map(|state| {
                                format!(
                                    "Mounted {} at {}",
                                    state.remote,
                                    state.mountpoint.display()
                                )
                            })
                            .map_err(|error| error.to_string())
                    }
                    MountOperation::Unmount => service
                        .unmount(&id)
                        .map(|()| format!("Unmounted {id}"))
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

    fn open_mountpoint(&mut self, id: String) -> Task<Message> {
        let state_file = self.paths.state_file(&id);
        self.status = format!("Opening {id}...");
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let state: MountState =
                        read_json(&state_file).map_err(|error| error.to_string())?;
                    open_path(&state.mountpoint)
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::OpenFinished,
        )
    }

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        if window == self.main_window {
            self.main_view()
        } else {
            self.transfer_popup_view(window)
        }
    }

    fn main_view(&self) -> Element<'_, Message> {
        let toolbar = row![
            text(APP_NAME).size(28),
            Space::new().width(Fill),
            button("Refresh").on_press(Message::Refresh),
            button("Add connection").on_press(Message::AddConnection),
            button("Settings").on_press(Message::OpenSettings),
        ]
        .spacing(10)
        .align_y(Center);

        let mut connections = column![].spacing(8);
        if self.servers.is_empty() {
            connections = connections.push(
                container(text("No saved connections").size(20))
                    .padding(28)
                    .width(Fill)
                    .center_x(Fill),
            );
        } else {
            for server in &self.servers {
                connections = connections.push(connection_card(
                    server,
                    self.mount_statuses
                        .get(&server.id)
                        .copied()
                        .unwrap_or(MountStatus::Unmounted),
                    self.busy.contains(&server.id),
                    self.transfers.get(&server.id),
                    self.transfer_errors.contains_key(&server.id),
                ));
            }
        }

        container(
            column![
                toolbar,
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

    fn transfer_popup_view(&self, window: window::Id) -> Element<'_, Message> {
        let Some(server_id) = self.popup_windows.get(&window) else {
            return container(text("Transfer completed"))
                .padding(16)
                .width(Fill)
                .height(Fill)
                .into();
        };
        let name = self
            .servers
            .iter()
            .find(|server| &server.id == server_id)
            .map_or(server_id.as_str(), ServerConfig::display_name);
        let snapshot = self.transfers.get(server_id);
        let summary = snapshot
            .map(transfer_label)
            .unwrap_or_else(|| "Checking transfer state".into());
        let percentage = snapshot.map_or(0.0, |snapshot| snapshot.percentage as f32);
        let current_file = snapshot
            .and_then(|snapshot| snapshot.files.iter().find(|file| file.uploading))
            .map(|file| {
                let filename = Path::new(&file.name)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                format!("{} - {}/s", filename, format_bytes(file.speed as u64))
            })
            .unwrap_or_else(|| "Waiting for remote confirmation".into());

        container(
            column![
                row![
                    text(name).size(18),
                    Space::new().width(Fill),
                    button("x").on_press(Message::ClosePopup(window)),
                ]
                .align_y(Center),
                text(summary).size(14),
                progress_bar(0.0..=100.0, percentage),
                text(current_file).size(12),
            ]
            .spacing(8),
        )
        .padding(14)
        .width(Fill)
        .height(Fill)
        .into()
    }
}

fn connection_card<'a>(
    server: &'a ServerConfig,
    status: MountStatus,
    busy: bool,
    transfer: Option<&'a TransferSnapshot>,
    transfer_unavailable: bool,
) -> Element<'a, Message> {
    let id = server.id.clone();
    let host = format!("{}@{}:{}", server.user, server.host, server.port);
    let remote = if server.remote_path.is_empty() {
        "~".to_owned()
    } else {
        server.remote_path.clone()
    };
    let operation_label = if matches!(status, MountStatus::Mounted | MountStatus::Starting) {
        "Unmount"
    } else {
        "Mount"
    };
    let mut operation = button(operation_label);
    if !busy {
        operation = operation.on_press(Message::Mount(id.clone()));
    }
    let mut open = button("Open");
    if status == MountStatus::Mounted && !busy {
        open = open.on_press(Message::Open(id.clone()));
    }
    let mut details = column![
        text(server.display_name()).size(22),
        text(host).size(15),
        text(format!("{}  ->  {}", remote, display_mountpoint(server))).size(14),
        text(status_label(status)).size(13),
    ]
    .spacing(4)
    .width(Fill);
    if status == MountStatus::Mounted {
        if transfer_unavailable {
            details = details.push(text("Transfer state unavailable").size(13));
        } else if let Some(snapshot) = transfer {
            details = details
                .push(text(transfer_label(snapshot)).size(13))
                .push(progress_bar(0.0..=100.0, snapshot.percentage as f32));
        }
    }
    container(
        row![
            details,
            operation,
            open,
            button("Edit").on_press(Message::Edit(id.clone())),
            button("Remove").on_press(Message::Remove(id)),
        ]
        .spacing(8)
        .align_y(Center),
    )
    .padding(16)
    .width(Fill)
    .style(container::rounded_box)
    .into()
}

fn display_mountpoint(server: &ServerConfig) -> &str {
    if server.mountpoint.is_empty() {
        "Auto"
    } else {
        &server.mountpoint
    }
}

fn status_label(status: MountStatus) -> &'static str {
    match status {
        MountStatus::Mounted => "Mounted",
        MountStatus::Unmounted => "Unmounted",
        MountStatus::Starting => "Starting",
        MountStatus::Stale => "Stale state",
    }
}

fn transfer_label(snapshot: &TransferSnapshot) -> String {
    if snapshot.errors > 0 {
        format!("{} upload error(s)", snapshot.errors)
    } else if snapshot.uploading > 0 {
        if snapshot.files.is_empty() {
            format!(
                "Uploading {} file(s) - progress unavailable",
                snapshot.uploading
            )
        } else {
            format!(
                "Uploading {} file(s) - {:.0}%",
                snapshot.uploading, snapshot.percentage
            )
        }
    } else if snapshot.queued > 0 {
        format!(
            "{} file(s) queued - {}",
            snapshot.queued,
            format_bytes(snapshot.queued_bytes)
        )
    } else if snapshot.synced {
        "Cloud synced".into()
    } else {
        "Checking cloud state".into()
    }
}

fn transfer_is_active(snapshot: &TransferSnapshot) -> bool {
    snapshot.queued > 0 || snapshot.uploading > 0 || snapshot.errors > 0
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
        .and_then(|path| path.parent().map(Path::to_owned))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn main_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(980.0, 720.0),
        position: window::Position::Centered,
        ..window::Settings::default()
    }
}

fn transfer_window_settings() -> window::Settings {
    let settings = window::Settings {
        size: Size::new(380.0, 150.0),
        position: window::Position::SpecificWith(bottom_right_position),
        visible: !cfg!(windows),
        resizable: false,
        minimizable: false,
        decorations: false,
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

fn bottom_right_position(window: Size, monitor: Size) -> Point {
    Point::new(
        (monitor.width - window.width - 20.0).max(0.0),
        (monitor.height - window.height - 64.0).max(0.0),
    )
}

fn configure_popup_window(id: window::Id, index: usize) -> Task<Message> {
    window::monitor_size(id).then(move |monitor| {
        let monitor = monitor.unwrap_or(Size::new(1920.0, 1080.0));
        let size = Size::new(380.0, 150.0);
        let mut position = bottom_right_position(size, monitor);
        position.y = (position.y - index as f32 * (size.height + 12.0)).max(0.0);
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
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const HWND_TOPMOST: isize = -1;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const SW_SHOWNOACTIVATE: i32 = 4;

    // The handle belongs to Iced for this callback; these calls only adjust window styles.
    unsafe {
        let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
        SetWindowLongPtrW(
            window,
            GWL_EXSTYLE,
            style | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        );
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

fn open_path(path: &Path) -> Result<(), String> {
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
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open {}: {error}", path.display()))
}
