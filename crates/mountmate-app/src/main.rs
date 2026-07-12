use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Center, Element, Fill, Task, Theme};
use mountmate_core::paths::AppPaths;
use mountmate_core::process::MountStatus;
use mountmate_core::service::MountService;
use mountmate_core::storage::{self, read_json};
use mountmate_core::{APP_NAME, MountState, ServerConfig, Settings, VERSION};

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
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
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    Refresh,
    StatusesLoaded(Vec<(String, Result<MountStatus, String>)>),
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

fn connection_card(server: &ServerConfig, status: MountStatus, busy: bool) -> Element<'_, Message> {
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
    container(
        row![
            column![
                text(server.display_name()).size(22),
                text(host).size(15),
                text(format!("{}  ->  {}", remote, display_mountpoint(server))).size(14),
                text(status_label(status)).size(13),
            ]
            .spacing(4)
            .width(Fill),
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
