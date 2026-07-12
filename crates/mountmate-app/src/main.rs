use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use iced::widget::{
    Space, button, checkbox, column, container, pick_list, progress_bar, row, scrollable, text,
    text_input,
};
use iced::{Center, Element, Fill, Length, Point, Size, Subscription, Task, Theme, window};
use mountmate_core::connection::{
    ConnectionDraft, ConnectionSource, DraftError, ImportAction, ImportStatus, SecretAction,
    SshImportPlan,
};
use mountmate_core::paths::AppPaths;
use mountmate_core::process::MountStatus;
use mountmate_core::service::MountService;
use mountmate_core::ssh::{
    default_ssh_config_path, prepare_managed_ssh_server, remove_managed_ssh_server,
};
use mountmate_core::storage::{self, read_json};
use mountmate_core::transfer::TransferSnapshot;
use mountmate_core::{
    APP_NAME, AuthMethod, ConnectionMethod, MountState, ServerConfig, Settings, VERSION,
};
use mountmate_platform::Platform;

mod i18n;
mod transfer_center;

use i18n::{Choice, LanguagePreference as Language, Locale, TextKey};
use transfer_center::{connection_view as transfer_connection_view, totals as transfer_totals};

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
    system_locale: Locale,
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
    screen: Screen,
    connection_draft: Option<ConnectionDraft>,
    settings_draft: Option<SettingsDraft>,
    editor_saving: bool,
    ssh_import_loading: bool,
    ssh_import_plan: Option<SshImportPlan>,
    ssh_import_actions: Vec<ImportAction>,
    pending_delete: Option<String>,
    status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Connections,
    TransferCenter,
    ConnectionEditor,
    Settings,
}

#[derive(Debug, Clone, Copy)]
enum ConnectionField {
    Name,
    HostAlias,
    Host,
    User,
    Port,
    KeyFile,
    RemotePath,
    Mountpoint,
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
    MaxSize,
    MaxAge,
    MinFreeSpace,
    WriteBack,
    DirCacheTime,
    BufferSize,
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
    Refresh,
    StatusesLoaded(Vec<(String, Result<MountStatus, String>)>),
    TransferTick,
    TransfersLoaded(Vec<(String, Result<TransferSnapshot, String>)>),
    PopupOpened(window::Id),
    ClosePopup(window::Id),
    WindowClosed(window::Id),
    AddConnection,
    OpenTransfers,
    CloseTransfers,
    OpenSettings,
    CancelEditor,
    ConnectionSourceChanged(ConnectionSource),
    ConnectionFieldChanged(ConnectionField, String),
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
    StartupAllChanged(bool),
    AutoTransfersChanged(bool),
    AutoUpdatesChanged(bool),
    LanguageChanged(Language),
    SaveSettings,
    SettingsSaved(Result<Settings, String>),
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
    CancelRemove,
    ConfirmRemove,
    RemoveFinished(Result<Vec<ServerConfig>, String>),
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
            iced::time::every(Duration::from_secs(1)).map(|_| Message::TransferTick),
            window::close_events().map(Message::WindowClosed),
        ])
    }

    fn new() -> (Self, Task<Message>) {
        let paths = AppPaths::discover();
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
        let app = Self {
            paths,
            settings,
            system_locale,
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
            screen: Screen::Connections,
            connection_draft: None,
            settings_draft: None,
            editor_saving: false,
            ssh_import_loading: false,
            ssh_import_plan: None,
            ssh_import_actions: Vec::new(),
            pending_delete: None,
            status,
        };
        let task = Task::batch([open_window.discard(), app.status_task()]);
        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let locale = self.locale();
        match message {
            Message::Refresh => match storage::load_servers(&self.paths) {
                Ok(servers) => {
                    self.servers = servers;
                    self.status = locale.text(TextKey::RefreshingMountStatus).into();
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
                self.status = errors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| locale.text(TextKey::Ready).into());
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
                let mut draft = ConnectionDraft::default();
                draft.ssh_config_path = default_ssh_config_path().display().to_string();
                self.connection_draft = Some(draft);
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
                self.screen = Screen::Connections;
                self.status = locale.text(TextKey::Ready).into();
            }
            Message::OpenSettings => {
                self.settings_draft = Some(SettingsDraft::from_settings(&self.settings));
                self.screen = Screen::Settings;
                self.status = locale.text(TextKey::Settings).into();
            }
            Message::CancelEditor => {
                if !self.editor_saving {
                    self.connection_draft = None;
                    self.settings_draft = None;
                    self.ssh_import_plan = None;
                    self.ssh_import_actions.clear();
                    self.screen = Screen::Connections;
                    self.status = locale.text(TextKey::Ready).into();
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
                        ConnectionField::RemotePath => draft.remote_path = value,
                        ConnectionField::Mountpoint => draft.mountpoint = value,
                        ConnectionField::SshConfigPath => {
                            draft.ssh_config_path = value;
                            self.ssh_import_plan = None;
                            self.ssh_import_actions.clear();
                        }
                    }
                }
            }
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
                        SettingsField::MaxSize => draft.max_size = value,
                        SettingsField::MaxAge => draft.max_age = value,
                        SettingsField::MinFreeSpace => draft.min_free_space = value,
                        SettingsField::WriteBack => draft.write_back = value,
                        SettingsField::DirCacheTime => draft.dir_cache_time = value,
                        SettingsField::BufferSize => draft.buffer_size = value,
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
            Message::StartupAllChanged(value) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.startup_all = value;
                }
            }
            Message::AutoTransfersChanged(value) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.auto_show_transfers = value;
                }
            }
            Message::AutoUpdatesChanged(value) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.auto_check_updates = value;
                }
            }
            Message::LanguageChanged(language) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.language = language;
                }
                self.status = self.locale().text(TextKey::Settings).into();
            }
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
                Ok(()) => self.status = locale.text(TextKey::OpenedMountpoint).into(),
                Err(error) => self.status = error,
            },
            Message::Edit(id) => {
                if self.can_modify(&id)
                    && let Some(server) = self.servers.iter().find(|server| server.id == id)
                {
                    self.connection_draft = Some(ConnectionDraft::from_server(server));
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
                self.status = locale.removing(&id);
                let paths = self.paths.clone();
                let server = self.servers.iter().find(|server| server.id == id).cloned();
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
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    storage::save_settings(&paths, &settings)
                        .map(|()| result_settings)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Message::SettingsSaved,
        )
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
            MountOperation::Mount => self.locale().mounting(&id),
            MountOperation::Unmount => self.locale().unmounting(&id),
        };
        let service = self.service.clone();
        let settings = self.settings.clone();
        let locale = self.locale();
        let server = self.servers.iter().find(|server| server.id == id).cloned();
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
                                    state.remote,
                                    state.mountpoint.display()
                                ),
                                Locale::Chinese => format!(
                                    "已将 {} 挂载到 {}",
                                    state.remote,
                                    state.mountpoint.display()
                                ),
                            })
                            .map_err(|error| error.to_string())
                    }
                    MountOperation::Unmount => service
                        .unmount(&id)
                        .map(|()| match locale {
                            Locale::English => format!("Unmounted {id}"),
                            Locale::Chinese => format!("已卸载 {id}"),
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

    fn open_mountpoint(&mut self, id: String) -> Task<Message> {
        let state_file = self.paths.state_file(&id);
        let locale = self.locale();
        self.status = locale.opening(&id);
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

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        if window == self.main_window {
            match self.screen {
                Screen::Connections => self.main_view(),
                Screen::TransferCenter => self.transfer_center_view(),
                Screen::ConnectionEditor => self.connection_editor_view(),
                Screen::Settings => self.settings_view(),
            }
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
                        can_modify: self.can_modify(&server.id),
                        confirming_remove: self.pending_delete.as_deref() == Some(&server.id),
                    },
                    locale,
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

        let overview = column![
            text(summary).size(18),
            progress_bar(0.0..=100.0, totals.percentage as f32),
            text(if totals.progress_available {
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
            .size(13),
        ]
        .spacing(8);

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
        let paths = row![
            connection_input(
                locale.text(TextKey::RemotePath),
                &draft.remote_path,
                ConnectionField::RemotePath,
            ),
            connection_input(
                locale.text(TextKey::Mountpoint),
                &draft.mountpoint,
                ConnectionField::Mountpoint,
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
            labeled_control(
                locale.text(TextKey::VfsCacheMode),
                pick_list(
                    localized_choices(CacheMode::ALL, locale, Locale::cache_mode),
                    Some(locale.choice(draft.cache_mode, locale.cache_mode(draft.cache_mode),)),
                    |mode| Message::CacheModeChanged(mode.value)
                )
                .width(Fill),
            ),
        ]
        .spacing(12);
        let cache_limits = row![
            settings_input(
                locale.text(TextKey::MaximumSize),
                &draft.max_size,
                SettingsField::MaxSize
            ),
            settings_input(
                locale.text(TextKey::MaximumAge),
                &draft.max_age,
                SettingsField::MaxAge
            ),
            settings_input(
                locale.text(TextKey::MinimumFreeSpace),
                &draft.min_free_space,
                SettingsField::MinFreeSpace,
            ),
        ]
        .spacing(12);
        let cache_timing = row![
            settings_input(
                locale.text(TextKey::WriteBackDelay),
                &draft.write_back,
                SettingsField::WriteBack
            ),
            settings_input(
                locale.text(TextKey::DirectoryCacheTime),
                &draft.dir_cache_time,
                SettingsField::DirCacheTime,
            ),
            settings_input(
                locale.text(TextKey::BufferSize),
                &draft.buffer_size,
                SettingsField::BufferSize
            ),
        ]
        .spacing(12);
        let behavior = column![
            checkbox(draft.startup_all)
                .label(locale.text(TextKey::MountAllAtLogin))
                .on_toggle(Message::StartupAllChanged),
            checkbox(draft.auto_show_transfers)
                .label(locale.text(TextKey::ShowTransferPopup))
                .on_toggle(Message::AutoTransfersChanged),
            checkbox(draft.auto_check_updates)
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
        let content = column![cache_profile, cache_limits, cache_timing, behavior]
            .spacing(18)
            .max_width(900);
        editor_shell(header, scrollable(content), &self.status)
    }

    fn transfer_popup_view(&self, window: window::Id) -> Element<'_, Message> {
        let locale = self.locale();
        let Some(server_id) = self.popup_windows.get(&window) else {
            return container(text(locale.text(TextKey::TransferCompleted)))
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
            .map(|snapshot| transfer_label(locale, snapshot))
            .unwrap_or_else(|| locale.text(TextKey::CheckingTransferState).into());
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
            .unwrap_or_else(|| locale.text(TextKey::WaitingRemoteConfirmation).into());

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

struct ConnectionCardState<'a> {
    status: MountStatus,
    busy: bool,
    transfer: Option<&'a TransferSnapshot>,
    transfer_unavailable: bool,
    can_modify: bool,
    confirming_remove: bool,
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
        can_modify,
        confirming_remove,
    } = state;
    let id = server.id.clone();
    let host = format!("{}@{}:{}", server.user, server.host, server.port);
    let remote = if server.remote_path.is_empty() {
        "~".to_owned()
    } else {
        server.remote_path.clone()
    };
    let operation_label = if matches!(status, MountStatus::Mounted | MountStatus::Starting) {
        locale.text(TextKey::Unmount)
    } else {
        locale.text(TextKey::Mount)
    };
    let mut operation = button(operation_label);
    if !busy {
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
        if transfer_unavailable {
            details = details.push(text(locale.text(TextKey::TransferStateUnavailable)).size(13));
        } else if let Some(snapshot) = transfer {
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

fn settings_input<'a>(
    label: &'a str,
    value: &'a str,
    field: SettingsField,
) -> iced::widget::Column<'a, Message> {
    labeled_control(
        label,
        text_input(label, value)
            .on_input(move |value| Message::SettingsFieldChanged(field, value))
            .width(Fill),
    )
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
    if snapshot.errors > 0 {
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
                "{} file(s) queued - {}",
                snapshot.queued,
                format_bytes(snapshot.queued_bytes)
            ),
            Locale::Chinese => format!(
                "{} 个文件排队中 - {}",
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
}
