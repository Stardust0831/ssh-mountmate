use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Center, Element, Fill, Task, Theme};
use mountmate_core::paths::AppPaths;
use mountmate_core::storage;
use mountmate_core::{APP_NAME, ServerConfig, Settings, VERSION};

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
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    Refresh,
    AddConnection,
    OpenSettings,
    Mount(String),
    Open(String),
    Edit(String),
    Remove(String),
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
        let settings = storage::load_settings(&paths).unwrap_or_default();
        let (servers, status) = match storage::load_servers(&paths) {
            Ok(servers) => (servers, "Ready".into()),
            Err(error) => (
                Vec::new(),
                format!("Could not load existing configuration: {error}"),
            ),
        };
        (
            Self {
                paths,
                settings,
                servers,
                status,
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Refresh => match storage::load_servers(&self.paths) {
                Ok(servers) => {
                    self.servers = servers;
                    self.status = "Configuration refreshed".into();
                }
                Err(error) => self.status = error.to_string(),
            },
            Message::AddConnection => {
                self.status = "Connection editor is the next implemented surface".into()
            }
            Message::OpenSettings => {
                self.status = format!("Cache profile: {}", self.settings.vfs_cache_mode)
            }
            Message::Mount(id) => self.status = format!("Mount requested for {id}"),
            Message::Open(id) => self.status = format!("Open requested for {id}"),
            Message::Edit(id) => self.status = format!("Edit requested for {id}"),
            Message::Remove(id) => self.status = format!("Remove requested for {id}"),
        }
        Task::none()
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
                connections = connections.push(connection_card(server));
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

fn connection_card(server: &ServerConfig) -> Element<'_, Message> {
    let id = server.id.clone();
    let host = format!("{}@{}:{}", server.user, server.host, server.port);
    let remote = if server.remote_path.is_empty() {
        "~".to_owned()
    } else {
        server.remote_path.clone()
    };
    container(
        row![
            column![
                text(server.display_name()).size(22),
                text(host).size(15),
                text(format!("{}  ->  {}", remote, display_mountpoint(server))).size(14),
            ]
            .spacing(4)
            .width(Fill),
            button("Mount").on_press(Message::Mount(id.clone())),
            button("Open").on_press(Message::Open(id.clone())),
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
