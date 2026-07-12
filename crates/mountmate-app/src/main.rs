use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use iced::widget::{Space, button, column, container, progress_bar, row, scrollable, text};
use iced::{Center, Element, Fill, Subscription, Task, Theme};
use mountmate_core::paths::AppPaths;
use mountmate_core::process::MountStatus;
use mountmate_core::service::MountService;
use mountmate_core::storage::{self, read_json};
use mountmate_core::transfer::TransferSnapshot;
use mountmate_core::{APP_NAME, MountState, ServerConfig, Settings, VERSION};

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .window_size((980.0, 720.0))
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
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    Refresh,
    StatusesLoaded(Vec<(String, Result<MountStatus, String>)>),
    TransferTick,
    TransfersLoaded(Vec<(String, Result<TransferSnapshot, String>)>),
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
    fn title(&self) -> String {
        format!("{APP_NAME} {VERSION}")
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(Duration::from_secs(1)).map(|_| Message::TransferTick)
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
            status,
        };
        let task = app.status_task();
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
                            self.transfers.insert(id.clone(), snapshot);
                            self.transfer_errors.remove(&id);
                        }
                        Err(error) => {
                            self.transfer_errors.insert(id, error);
                        }
                    }
                }
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
                            id,
                            match operation {
                                MountOperation::Mount => MountStatus::Mounted,
                                MountOperation::Unmount => MountStatus::Unmounted,
                            },
                        );
                        self.status = message;
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

    fn view(&self) -> Element<'_, Message> {
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
