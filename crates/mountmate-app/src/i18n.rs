use std::fmt;

use mountmate_core::connection::{ConnectionSource, ImportAction, ImportStatus};
use mountmate_core::rc::RefreshResult;
use mountmate_core::{
    AccentColor, AppearanceMode, AuthMethod, ConnectionMethod, CredentialStorage, MountBackend,
};

use super::CacheMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Locale {
    English,
    Chinese,
}

impl Locale {
    pub(crate) fn system() -> Self {
        sys_locale::get_locale()
            .as_deref()
            .map(Self::from_locale_name)
            .unwrap_or(Self::English)
    }

    pub(crate) fn from_preference(preference: LanguagePreference, system: Self) -> Self {
        match preference {
            LanguagePreference::Auto => system,
            LanguagePreference::English => Self::English,
            LanguagePreference::Chinese => Self::Chinese,
        }
    }

    fn from_locale_name(locale: &str) -> Self {
        let locale = locale.to_ascii_lowercase().replace('_', "-");
        if locale == "zh"
            || locale == "zh-cn"
            || locale.starts_with("zh-cn-")
            || locale == "zh-sg"
            || locale.starts_with("zh-sg-")
            || locale == "zh-hans"
            || locale.starts_with("zh-hans-")
        {
            Self::Chinese
        } else {
            Self::English
        }
    }

    pub(crate) fn text(self, key: TextKey) -> &'static str {
        match self {
            Self::English => english(key),
            Self::Chinese => chinese(key),
        }
    }

    pub(crate) fn choice<T>(self, value: T, label: &'static str) -> Choice<T> {
        Choice { value, label }
    }

    pub(crate) fn connection_source(self, value: ConnectionSource) -> &'static str {
        match (self, value) {
            (Self::English, ConnectionSource::Manual) => "Manual",
            (Self::English, ConnectionSource::SshConfig) => "SSH config",
            (Self::English, ConnectionSource::SshConfigBatch) => "SSH config (batch)",
            (Self::English, ConnectionSource::SaiCluster) => "SAI cluster",
            (Self::Chinese, ConnectionSource::Manual) => "手动配置",
            (Self::Chinese, ConnectionSource::SshConfig) => "SSH 配置",
            (Self::Chinese, ConnectionSource::SshConfigBatch) => "SSH 配置（批量）",
            (Self::Chinese, ConnectionSource::SaiCluster) => "SAI 集群",
        }
    }

    pub(crate) fn auth_method(self, value: AuthMethod) -> &'static str {
        match (self, value) {
            (Self::English, AuthMethod::Key) => "Private key",
            (Self::English, AuthMethod::Password) => "Password",
            (Self::Chinese, AuthMethod::Key) => "私钥",
            (Self::Chinese, AuthMethod::Password) => "密码",
        }
    }

    pub(crate) fn connection_method(self, value: ConnectionMethod) -> &'static str {
        match (self, value) {
            (Self::English, ConnectionMethod::Native) => "Native SFTP",
            (Self::English, ConnectionMethod::Openssh) => "OpenSSH",
            (Self::English, ConnectionMethod::Interactive) => "Interactive shared SSH",
            (Self::Chinese, ConnectionMethod::Native) => "原生 SFTP",
            (Self::Chinese, ConnectionMethod::Openssh) => "OpenSSH",
            (Self::Chinese, ConnectionMethod::Interactive) => "交互式共享 SSH",
        }
    }

    pub(crate) fn mount_backend(self, value: MountBackend) -> &'static str {
        match (self, value) {
            (Self::English, MountBackend::Fuse) => "FUSE (macFUSE / FUSE-T, default)",
            (Self::English, MountBackend::Nfs) => "rclone built-in NFS (Experimental)",
            (Self::Chinese, MountBackend::Fuse) => "FUSE（macFUSE / FUSE-T，默认）",
            (Self::Chinese, MountBackend::Nfs) => "rclone 内置 NFS（实验性）",
        }
    }

    pub(crate) fn credential_storage(self, value: CredentialStorage) -> &'static str {
        match (self, value) {
            (Self::English, CredentialStorage::Obscure) => "rclone obscure (compatible default)",
            (Self::English, CredentialStorage::System) => "System credential store",
            (Self::Chinese, CredentialStorage::Obscure) => "rclone obscure（兼容默认）",
            (Self::Chinese, CredentialStorage::System) => "系统凭据库",
        }
    }

    pub(crate) fn appearance_mode(self, value: AppearanceMode) -> &'static str {
        match (self, value) {
            (Self::English, AppearanceMode::System) => "Follow system",
            (Self::English, AppearanceMode::Light) => "Light",
            (Self::English, AppearanceMode::Dark) => "Dark",
            (Self::Chinese, AppearanceMode::System) => "跟随系统",
            (Self::Chinese, AppearanceMode::Light) => "浅色",
            (Self::Chinese, AppearanceMode::Dark) => "深色",
        }
    }

    pub(crate) fn accent_color(self, value: AccentColor) -> &'static str {
        match (self, value) {
            (Self::English, AccentColor::Blue) => "Blue",
            (Self::English, AccentColor::Green) => "Green",
            (Self::English, AccentColor::Amber) => "Amber",
            (Self::English, AccentColor::Purple) => "Purple",
            (Self::Chinese, AccentColor::Blue) => "蓝色",
            (Self::Chinese, AccentColor::Green) => "绿色",
            (Self::Chinese, AccentColor::Amber) => "琥珀色",
            (Self::Chinese, AccentColor::Purple) => "紫色",
        }
    }

    pub(crate) fn import_status(self, value: ImportStatus) -> &'static str {
        match (self, value) {
            (Self::English, ImportStatus::New) => "New",
            (Self::English, ImportStatus::Same) => "Same configuration",
            (Self::English, ImportStatus::SameHost) => "Same SSH Host",
            (Self::English, ImportStatus::SameTarget) => "Same target",
            (Self::English, ImportStatus::Invalid) => "Invalid",
            (Self::Chinese, ImportStatus::New) => "新配置",
            (Self::Chinese, ImportStatus::Same) => "配置相同",
            (Self::Chinese, ImportStatus::SameHost) => "SSH Host 相同",
            (Self::Chinese, ImportStatus::SameTarget) => "目标相同",
            (Self::Chinese, ImportStatus::Invalid) => "无效",
        }
    }

    pub(crate) fn import_action(self, value: ImportAction) -> &'static str {
        match (self, value) {
            (Self::English, ImportAction::Ignore) => "Ignore",
            (Self::English, ImportAction::Import) => "Import",
            (Self::English, ImportAction::Overwrite) => "Overwrite",
            (Self::Chinese, ImportAction::Ignore) => "忽略",
            (Self::Chinese, ImportAction::Import) => "导入",
            (Self::Chinese, ImportAction::Overwrite) => "覆盖",
        }
    }

    pub(crate) fn cache_mode(self, value: CacheMode) -> &'static str {
        match (self, value) {
            (Self::English, CacheMode::Off) => "Off",
            (Self::English, CacheMode::Minimal) => "Minimal",
            (Self::English, CacheMode::Writes) => "Writes",
            (Self::English, CacheMode::Full) => "Full",
            (Self::Chinese, CacheMode::Off) => "关闭",
            (Self::Chinese, CacheMode::Minimal) => "最小",
            (Self::Chinese, CacheMode::Writes) => "仅写入",
            (Self::Chinese, CacheMode::Full) => "完整",
        }
    }

    pub(crate) fn language(self, value: LanguagePreference) -> &'static str {
        match (self, value) {
            (Self::English, LanguagePreference::Auto) => "System default",
            (Self::English, LanguagePreference::English) => "English",
            (Self::English, LanguagePreference::Chinese) => "Simplified Chinese",
            (Self::Chinese, LanguagePreference::Auto) => "跟随系统",
            (Self::Chinese, LanguagePreference::English) => "English",
            (Self::Chinese, LanguagePreference::Chinese) => "简体中文",
        }
    }

    pub(crate) fn loaded_ssh_hosts(self, count: usize) -> String {
        match self {
            Self::English => format!("Loaded {count} SSH Host entries"),
            Self::Chinese => format!("已加载 {count} 个 SSH Host 条目"),
        }
    }

    pub(crate) fn editing(self, name: &str) -> String {
        match self {
            Self::English => format!("Editing {name}"),
            Self::Chinese => format!("正在编辑 {name}"),
        }
    }

    pub(crate) fn removing(self, id: &str) -> String {
        match self {
            Self::English => format!("Removing {id}..."),
            Self::Chinese => format!("正在删除 {id}..."),
        }
    }

    pub(crate) fn loading_path(self, path: &str) -> String {
        match self {
            Self::English => format!("Loading {path}..."),
            Self::Chinese => format!("正在加载 {path}..."),
        }
    }

    pub(crate) fn saving_connections(self, count: usize) -> String {
        match self {
            Self::English => format!("Saving {count} SSH connections..."),
            Self::Chinese => format!("正在保存 {count} 个 SSH 连接..."),
        }
    }

    pub(crate) fn mounting(self, id: &str) -> String {
        match self {
            Self::English => format!("Mounting {id}..."),
            Self::Chinese => format!("正在挂载 {id}..."),
        }
    }

    pub(crate) fn unmounting(self, id: &str) -> String {
        match self {
            Self::English => format!("Unmounting {id}..."),
            Self::Chinese => format!("正在卸载 {id}..."),
        }
    }

    pub(crate) fn opening(self, id: &str) -> String {
        match self {
            Self::English => format!("Opening {id}..."),
            Self::Chinese => format!("正在打开 {id}..."),
        }
    }

    pub(crate) fn refresh_complete(self, result: &RefreshResult) -> String {
        let directory = if result.relative_dir.is_empty() {
            match self {
                Self::English => "mount root",
                Self::Chinese => "挂载根目录",
            }
        } else {
            &result.relative_dir
        };
        match (self, result.pending_uploads) {
            (Self::English, 0) => format!(
                "Remote cache refreshed for {directory}; directory currently has {} direct entries",
                result.entries.len()
            ),
            (Self::English, pending) => format!(
                "Remote cache refreshed for {directory}; directory currently has {} direct entries; {pending} local file(s) still waiting to upload",
                result.entries.len()
            ),
            (Self::Chinese, 0) => format!(
                "已刷新 {directory} 的云端缓存；该目录当前有 {} 个直属条目",
                result.entries.len()
            ),
            (Self::Chinese, pending) => format!(
                "已刷新 {directory} 的云端缓存；该目录当前有 {} 个直属条目；仍有 {pending} 个本地文件等待上传",
                result.entries.len()
            ),
        }
    }

    pub(crate) fn tray_unavailable(self, error: &str) -> String {
        match self {
            Self::English => format!("System tray unavailable: {error}"),
            Self::Chinese => format!("系统托盘不可用：{error}"),
        }
    }

    pub(crate) fn exit_warning(
        self,
        active: usize,
        unknown: usize,
        interactive_sessions: usize,
    ) -> String {
        match self {
            Self::English => format!(
                "{active} mounted connection(s) still have queued or active uploads, {unknown} mounted connection(s) have an unknown cloud state, and {interactive_sessions} interactive SSH session(s) will end. Exiting the interface leaves rclone mounts running, but you will no longer see transfer status. Exit anyway?"
            ),
            Self::Chinese => format!(
                "仍有 {active} 个挂载连接存在排队或正在进行的上传，另有 {unknown} 个挂载连接的云端状态未知，并且 {interactive_sessions} 个交互式 SSH 会话将结束。退出界面不会停止 rclone 挂载，但你将无法继续查看传输状态。仍要退出吗？"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LanguagePreference {
    Auto,
    English,
    Chinese,
}

impl LanguagePreference {
    pub(crate) const ALL: [Self; 3] = [Self::Auto, Self::English, Self::Chinese];

    pub(crate) fn from_value(value: &str) -> Self {
        match value {
            "en" => Self::English,
            "zh" => Self::Chinese,
            _ => Self::Auto,
        }
    }

    pub(crate) fn value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en",
            Self::Chinese => "zh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Choice<T> {
    pub(crate) value: T,
    label: &'static str,
}

impl<T> fmt::Display for Choice<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TextKey {
    AddConnection,
    AllCloudSynced,
    Authentication,
    AutoMountpoint,
    Back,
    Browse,
    BufferSize,
    CacheRoot,
    Cancel,
    CheckUpdatesAutomatically,
    CheckingCloudState,
    CheckingTransferState,
    CloudSynced,
    ConfirmRemove,
    ConnectionEditorUnavailable,
    ConnectionGone,
    ConnectionRemoved,
    ConnectionSaved,
    CopyLog,
    CopyPrivateKey,
    DirectoryCacheTime,
    Edit,
    EditConnection,
    Exit,
    FileManagerIntegration,
    FileManagerIntegrationHelp,
    FileManagerMenuRegistered,
    FileManagerMenuRemoved,
    FileTransfer,
    HideDetails,
    Import,
    ImportSshConfig,
    IpHost,
    KeyPassphrase,
    Language,
    Load,
    LoadSshBeforeImport,
    Loading,
    LoadingLog,
    LoadingMountStatus,
    LogCopied,
    LogTruncated,
    Logs,
    LogsHelp,
    ManagedByOpenSsh,
    MaximumAge,
    MaximumSize,
    MinimumFreeSpace,
    UploadConcurrency,
    Mount,
    MountAll,
    MountAllAtLogin,
    MountConnectionForTransfers,
    Mountpoint,
    Name,
    NewConnection,
    NoMountedConnections,
    NoLogContent,
    NoSavedConnections,
    Open,
    OpenedMountpoint,
    Optional,
    Password,
    PasswordRequired,
    Port,
    PrivateKeyFile,
    Ready,
    Refresh,
    RefreshNow,
    Refreshing,
    RefreshingMountStatus,
    RegisterFileManagerMenu,
    RegisteringFileManagerMenu,
    RemotePath,
    Remove,
    RemoveFileManagerMenu,
    RemovingFileManagerMenu,
    RunningInBackground,
    Save,
    Saving,
    SavingConnection,
    SavingSettings,
    SelectCacheDirectory,
    SelectPrivateKey,
    SelectSshConfig,
    SelectSshHost,
    Settings,
    SettingsSaved,
    SettingsUnavailable,
    ShowTransferPopup,
    ShowDetails,
    ShowMainWindow,
    Source,
    SshConfigFile,
    SshConfigPathRequired,
    SshHost,
    SshHostAlias,
    TransferCenter,
    TransferCompleted,
    TransferStateUnavailable,
    Transfers,
    InteractiveTerminal,
    InteractiveTerminalHelp,
    InteractiveTerminalStarting,
    InteractiveTerminalReady,
    InteractiveTerminalExited,
    InteractiveTerminalFailed,
    OpenInteractiveTerminal,
    RetryTerminal,
    HideTerminal,
    EndInteractiveSession,
    Transport,
    Unmount,
    UnmountAll,
    UnmountBeforeRemove,
    User,
    VfsCacheMode,
    ViewLog,
    WaitingRemoteConfirmation,
    WriteBackDelay,
    WriteManagedProfile,
}

fn english(key: TextKey) -> &'static str {
    match key {
        TextKey::AddConnection => "Add connection",
        TextKey::AllCloudSynced => "All mounted connections are cloud synced",
        TextKey::Authentication => "Authentication",
        TextKey::AutoMountpoint => "Auto",
        TextKey::Back => "Back",
        TextKey::Browse => "Browse",
        TextKey::BufferSize => "Buffer size",
        TextKey::CacheRoot => "Cache root",
        TextKey::Cancel => "Cancel",
        TextKey::CheckUpdatesAutomatically => "Check for updates automatically",
        TextKey::CheckingCloudState => "Checking cloud state",
        TextKey::CheckingTransferState => "Checking transfer state",
        TextKey::CloudSynced => "Cloud synced",
        TextKey::ConfirmRemove => "Confirm remove",
        TextKey::ConnectionEditorUnavailable => "Connection editor unavailable",
        TextKey::ConnectionGone => "Connection no longer exists",
        TextKey::ConnectionRemoved => "Connection removed",
        TextKey::ConnectionSaved => "Connection saved",
        TextKey::CopyLog => "Copy log",
        TextKey::CopyPrivateKey => "Copy the private key into ~/.ssh",
        TextKey::DirectoryCacheTime => "Directory cache time",
        TextKey::Edit => "Settings",
        TextKey::EditConnection => "Connection settings",
        TextKey::Exit => "Exit",
        TextKey::FileManagerIntegration => "File manager integration",
        TextKey::FileManagerIntegrationHelp => {
            "Adds Refresh and Transfers commands for the current user's file manager."
        }
        TextKey::FileManagerMenuRegistered => "File-manager commands registered",
        TextKey::FileManagerMenuRemoved => "File-manager commands removed",
        TextKey::FileTransfer => "File transfer",
        TextKey::HideDetails => "Hide details",
        TextKey::Import => "Import",
        TextKey::ImportSshConfig => "Import SSH config",
        TextKey::IpHost => "IP / Host",
        TextKey::KeyPassphrase => "Key passphrase",
        TextKey::Language => "Language",
        TextKey::Load => "Load",
        TextKey::LoadSshBeforeImport => "Load an SSH config before importing",
        TextKey::Loading => "Loading...",
        TextKey::LoadingLog => "Loading log...",
        TextKey::LoadingMountStatus => "Loading mount status...",
        TextKey::LogCopied => "Log copied to the clipboard",
        TextKey::LogTruncated => "Showing the most recent 2 MiB of this log",
        TextKey::Logs => "Mount logs",
        TextKey::LogsHelp => {
            "Open a read-only mount log viewer. Select any range to copy it, or use Copy log for the full visible log."
        }
        TextKey::ManagedByOpenSsh => "Private key (managed by OpenSSH)",
        TextKey::MaximumAge => "Maximum age",
        TextKey::MaximumSize => "Maximum size",
        TextKey::MinimumFreeSpace => "Minimum free space",
        TextKey::UploadConcurrency => "Simultaneous uploads",
        TextKey::Mount => "Mount",
        TextKey::MountAll => "Mount all",
        TextKey::MountAllAtLogin => "Mount all saved connections at login",
        TextKey::MountConnectionForTransfers => {
            "Mount a connection to inspect its cloud transfer state"
        }
        TextKey::Mountpoint => "Mountpoint (Auto by default)",
        TextKey::Name => "Name",
        TextKey::NewConnection => "New connection",
        TextKey::NoMountedConnections => "No mounted connections",
        TextKey::NoLogContent => "No log content is available yet",
        TextKey::NoSavedConnections => "No saved connections",
        TextKey::Open => "Open",
        TextKey::OpenedMountpoint => "Opened mountpoint",
        TextKey::Optional => "Optional",
        TextKey::Password => "Password",
        TextKey::PasswordRequired => "Required for a new or changed target",
        TextKey::Port => "Port",
        TextKey::PrivateKeyFile => "Private key file",
        TextKey::Ready => "Ready",
        TextKey::Refresh => "Refresh",
        TextKey::RefreshNow => "Refresh now",
        TextKey::Refreshing => "Refreshing...",
        TextKey::RefreshingMountStatus => "Refreshing mount status...",
        TextKey::RegisterFileManagerMenu => "Register file-manager commands",
        TextKey::RegisteringFileManagerMenu => "Registering file-manager commands...",
        TextKey::RemotePath => "Remote path ($HOME by default)",
        TextKey::Remove => "Remove",
        TextKey::RemoveFileManagerMenu => "Remove file-manager commands",
        TextKey::RemovingFileManagerMenu => "Removing file-manager commands...",
        TextKey::RunningInBackground => "Running in the system tray",
        TextKey::Save => "Save",
        TextKey::Saving => "Saving...",
        TextKey::SavingConnection => "Saving connection...",
        TextKey::SavingSettings => "Saving settings...",
        TextKey::SelectCacheDirectory => "Select cache directory",
        TextKey::SelectPrivateKey => "Select private key",
        TextKey::SelectSshConfig => "Select SSH config",
        TextKey::SelectSshHost => "Select at least one SSH Host to import or overwrite",
        TextKey::Settings => "Settings",
        TextKey::SettingsSaved => "Settings saved",
        TextKey::SettingsUnavailable => "Settings unavailable",
        TextKey::ShowTransferPopup => "Show transfer popup automatically",
        TextKey::ShowDetails => "Details",
        TextKey::ShowMainWindow => "Show SSH MountMate",
        TextKey::Source => "Source",
        TextKey::SshConfigFile => "SSH config file",
        TextKey::SshConfigPathRequired => "SSH config path is required",
        TextKey::SshHost => "SSH Host",
        TextKey::SshHostAlias => "SSH Host alias",
        TextKey::TransferCenter => "Transfer center",
        TextKey::TransferCompleted => "Transfer completed",
        TextKey::TransferStateUnavailable => "Transfer state unavailable",
        TextKey::Transfers => "Transfers",
        TextKey::InteractiveTerminal => "Interactive SSH terminal",
        TextKey::InteractiveTerminalHelp => {
            "Complete SSH authentication in this terminal. Input and output stay in memory only."
        }
        TextKey::InteractiveTerminalStarting => "Starting interactive SSH...",
        TextKey::InteractiveTerminalReady => "SSH session ready; mount will resume automatically",
        TextKey::InteractiveTerminalExited => "SSH terminal exited",
        TextKey::InteractiveTerminalFailed => "SSH terminal failed",
        TextKey::OpenInteractiveTerminal => "Open terminal",
        TextKey::RetryTerminal => "Retry",
        TextKey::HideTerminal => "Hide",
        TextKey::EndInteractiveSession => "End session",
        TextKey::Transport => "Transport",
        TextKey::Unmount => "Unmount",
        TextKey::UnmountAll => "Unmount all",
        TextKey::UnmountBeforeRemove => "Unmount the connection before removing it",
        TextKey::User => "User",
        TextKey::VfsCacheMode => "VFS cache mode",
        TextKey::ViewLog => "View log",
        TextKey::WaitingRemoteConfirmation => "Waiting for remote confirmation",
        TextKey::WriteBackDelay => "Write-back delay",
        TextKey::WriteManagedProfile => "Write a managed OpenSSH profile",
    }
}

fn chinese(key: TextKey) -> &'static str {
    match key {
        TextKey::AddConnection => "添加连接",
        TextKey::AllCloudSynced => "所有已挂载连接均已同步到云端",
        TextKey::Authentication => "身份验证",
        TextKey::AutoMountpoint => "自动",
        TextKey::Back => "返回",
        TextKey::Browse => "浏览",
        TextKey::BufferSize => "缓冲区大小",
        TextKey::CacheRoot => "缓存目录",
        TextKey::Cancel => "取消",
        TextKey::CheckUpdatesAutomatically => "自动检查更新",
        TextKey::CheckingCloudState => "正在检查云端状态",
        TextKey::CheckingTransferState => "正在检查传输状态",
        TextKey::CloudSynced => "云端已同步",
        TextKey::ConfirmRemove => "确认删除",
        TextKey::ConnectionEditorUnavailable => "连接编辑器不可用",
        TextKey::ConnectionGone => "连接已不存在",
        TextKey::ConnectionRemoved => "连接已删除",
        TextKey::ConnectionSaved => "连接已保存",
        TextKey::CopyLog => "复制日志",
        TextKey::CopyPrivateKey => "将私钥复制到 ~/.ssh",
        TextKey::DirectoryCacheTime => "目录缓存时间",
        TextKey::Edit => "设置",
        TextKey::EditConnection => "连接设置",
        TextKey::Exit => "退出",
        TextKey::FileManagerIntegration => "文件管理器集成",
        TextKey::FileManagerIntegrationHelp => "为当前用户的文件管理器添加刷新与传输中心命令。",
        TextKey::FileManagerMenuRegistered => "文件管理器命令已注册",
        TextKey::FileManagerMenuRemoved => "文件管理器命令已移除",
        TextKey::FileTransfer => "文件传输",
        TextKey::HideDetails => "收起详情",
        TextKey::Import => "导入",
        TextKey::ImportSshConfig => "导入 SSH 配置",
        TextKey::IpHost => "IP / 主机名",
        TextKey::KeyPassphrase => "私钥密码",
        TextKey::Language => "语言",
        TextKey::Load => "加载",
        TextKey::LoadSshBeforeImport => "请先加载 SSH 配置再导入",
        TextKey::Loading => "正在加载...",
        TextKey::LoadingLog => "正在加载日志...",
        TextKey::LoadingMountStatus => "正在加载挂载状态...",
        TextKey::LogCopied => "日志已复制到剪贴板",
        TextKey::LogTruncated => "当前显示该日志最近的 2 MiB 内容",
        TextKey::Logs => "挂载日志",
        TextKey::LogsHelp => {
            "打开只读挂载日志窗口，可选中任意部分复制，或用“复制日志”复制当前显示的全部内容。"
        }
        TextKey::ManagedByOpenSsh => "私钥（由 OpenSSH 管理）",
        TextKey::MaximumAge => "最长保留时间",
        TextKey::MaximumSize => "最大大小",
        TextKey::MinimumFreeSpace => "最小剩余空间",
        TextKey::UploadConcurrency => "同时上传文件数",
        TextKey::Mount => "挂载",
        TextKey::MountAll => "全部挂载",
        TextKey::MountAllAtLogin => "登录时挂载所有已保存连接",
        TextKey::MountConnectionForTransfers => "挂载连接后可查看其云端传输状态",
        TextKey::Mountpoint => "挂载点（默认自动选择）",
        TextKey::Name => "名称",
        TextKey::NewConnection => "新建连接",
        TextKey::NoMountedConnections => "没有已挂载的连接",
        TextKey::NoLogContent => "暂时没有可显示的日志内容",
        TextKey::NoSavedConnections => "没有已保存的连接",
        TextKey::Open => "打开",
        TextKey::OpenedMountpoint => "已打开挂载点",
        TextKey::Optional => "可选",
        TextKey::Password => "密码",
        TextKey::PasswordRequired => "新建连接或目标变化时必填",
        TextKey::Port => "端口",
        TextKey::PrivateKeyFile => "私钥文件",
        TextKey::Ready => "就绪",
        TextKey::Refresh => "刷新",
        TextKey::RefreshNow => "立即刷新",
        TextKey::Refreshing => "正在刷新...",
        TextKey::RefreshingMountStatus => "正在刷新挂载状态...",
        TextKey::RegisterFileManagerMenu => "注册文件管理器命令",
        TextKey::RegisteringFileManagerMenu => "正在注册文件管理器命令...",
        TextKey::RemotePath => "远端路径（默认为 $HOME）",
        TextKey::Remove => "删除",
        TextKey::RemoveFileManagerMenu => "移除文件管理器命令",
        TextKey::RemovingFileManagerMenu => "正在移除文件管理器命令...",
        TextKey::RunningInBackground => "正在系统托盘中运行",
        TextKey::Save => "保存",
        TextKey::Saving => "正在保存...",
        TextKey::SavingConnection => "正在保存连接...",
        TextKey::SavingSettings => "正在保存设置...",
        TextKey::SelectCacheDirectory => "选择缓存目录",
        TextKey::SelectPrivateKey => "选择私钥",
        TextKey::SelectSshConfig => "选择 SSH 配置文件",
        TextKey::SelectSshHost => "请至少选择一个要导入或覆盖的 SSH Host",
        TextKey::Settings => "设置",
        TextKey::SettingsSaved => "设置已保存",
        TextKey::SettingsUnavailable => "设置不可用",
        TextKey::ShowTransferPopup => "自动显示传输进度弹窗",
        TextKey::ShowDetails => "展开详情",
        TextKey::ShowMainWindow => "显示 SSH MountMate",
        TextKey::Source => "来源",
        TextKey::SshConfigFile => "SSH 配置文件",
        TextKey::SshConfigPathRequired => "必须填写 SSH 配置路径",
        TextKey::SshHost => "SSH Host",
        TextKey::SshHostAlias => "SSH Host 别名",
        TextKey::TransferCenter => "传输中心",
        TextKey::TransferCompleted => "传输已完成",
        TextKey::TransferStateUnavailable => "传输状态不可用",
        TextKey::Transfers => "传输",
        TextKey::InteractiveTerminal => "交互式 SSH 终端",
        TextKey::InteractiveTerminalHelp => {
            "请在此终端中完成 SSH 身份验证。输入和输出仅保留在内存中。"
        }
        TextKey::InteractiveTerminalStarting => "正在启动交互式 SSH…",
        TextKey::InteractiveTerminalReady => "SSH 会话已就绪；挂载将自动继续",
        TextKey::InteractiveTerminalExited => "SSH 终端已退出",
        TextKey::InteractiveTerminalFailed => "SSH 终端启动失败",
        TextKey::OpenInteractiveTerminal => "打开终端",
        TextKey::RetryTerminal => "重试",
        TextKey::HideTerminal => "隐藏",
        TextKey::EndInteractiveSession => "结束会话",
        TextKey::Transport => "传输方式",
        TextKey::Unmount => "卸载",
        TextKey::UnmountAll => "全部卸载",
        TextKey::UnmountBeforeRemove => "请先卸载连接再删除",
        TextKey::User => "用户",
        TextKey::VfsCacheMode => "VFS 缓存模式",
        TextKey::ViewLog => "查看日志",
        TextKey::WaitingRemoteConfirmation => "等待远端确认",
        TextKey::WriteBackDelay => "回写延迟",
        TextKey::WriteManagedProfile => "写入由程序管理的 OpenSSH 配置",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_locale_detection_handles_common_chinese_forms() {
        assert_eq!(Locale::from_locale_name("zh-CN"), Locale::Chinese);
        assert_eq!(Locale::from_locale_name("zh_Hans_CN"), Locale::Chinese);
        assert_eq!(Locale::from_locale_name("en-US"), Locale::English);
        assert_eq!(Locale::from_locale_name("zh-TW"), Locale::English);
        assert_eq!(Locale::from_locale_name("zh-HK"), Locale::English);
        assert_eq!(Locale::from_locale_name("zh-Hant"), Locale::English);
    }

    #[test]
    fn automatic_preference_uses_detected_system_locale() {
        assert_eq!(
            Locale::from_preference(LanguagePreference::Auto, Locale::Chinese),
            Locale::Chinese
        );
        assert_eq!(
            Locale::from_preference(LanguagePreference::English, Locale::Chinese),
            Locale::English
        );
    }

    #[test]
    fn choice_keeps_typed_value_and_localized_label() {
        let choice = Locale::Chinese.choice(AuthMethod::Key, "私钥");
        assert_eq!(choice.value, AuthMethod::Key);
        assert_eq!(choice.to_string(), "私钥");
    }

    #[test]
    fn interactive_connection_method_has_bilingual_labels() {
        assert_eq!(ConnectionMethod::ALL.len(), 3);
        assert_eq!(
            Locale::English.connection_method(ConnectionMethod::Interactive),
            "Interactive shared SSH"
        );
        assert_eq!(
            Locale::Chinese.connection_method(ConnectionMethod::Interactive),
            "交互式共享 SSH"
        );
    }

    #[test]
    fn exit_warning_counts_interactive_sessions_in_both_languages() {
        let english = Locale::English.exit_warning(1, 2, 3);
        assert!(english.contains("3 interactive SSH session(s) will end"));
        let chinese = Locale::Chinese.exit_warning(1, 2, 3);
        assert!(chinese.contains("3 个交互式 SSH 会话将结束"));
    }

    #[test]
    fn refresh_message_distinguishes_cache_refresh_from_directory_entry_count() {
        let result = RefreshResult {
            pending_uploads: 0,
            relative_dir: "folder/child".into(),
            entries: Vec::new(),
        };
        assert_eq!(
            Locale::Chinese.refresh_complete(&result),
            "已刷新 folder/child 的云端缓存；该目录当前有 0 个直属条目"
        );
        assert_eq!(
            Locale::English.refresh_complete(&result),
            "Remote cache refreshed for folder/child; directory currently has 0 direct entries"
        );
    }
}
