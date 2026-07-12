use std::fmt;

use mountmate_core::connection::{ConnectionSource, ImportAction, ImportStatus};
use mountmate_core::rc::RefreshResult;
use mountmate_core::{AuthMethod, ConnectionMethod};

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
        let locale = locale.to_ascii_lowercase();
        if locale == "zh" || locale.starts_with("zh-") || locale.starts_with("zh_") {
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
            (Self::Chinese, ConnectionMethod::Native) => "原生 SFTP",
            (Self::Chinese, ConnectionMethod::Openssh) => "OpenSSH",
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
        match (self, result.pending_uploads) {
            (Self::English, 0) => {
                format!("Remote refreshed: {} entries", result.entries.len())
            }
            (Self::English, pending) => format!(
                "Remote refreshed: {} entries; {pending} local file(s) still waiting to upload",
                result.entries.len()
            ),
            (Self::Chinese, 0) => format!("云端已刷新：{} 个条目", result.entries.len()),
            (Self::Chinese, pending) => format!(
                "云端已刷新：{} 个条目；仍有 {pending} 个本地文件等待上传",
                result.entries.len()
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
    CopyPrivateKey,
    DirectoryCacheTime,
    Edit,
    EditConnection,
    FileManagerIntegration,
    FileManagerIntegrationHelp,
    FileManagerMenuRegistered,
    FileManagerMenuRemoved,
    FileTransfer,
    Import,
    ImportSshConfig,
    IpHost,
    KeyPassphrase,
    Language,
    Load,
    LoadSshBeforeImport,
    Loading,
    LoadingMountStatus,
    ManagedByOpenSsh,
    MaximumAge,
    MaximumSize,
    MinimumFreeSpace,
    Mount,
    MountAllAtLogin,
    MountConnectionForTransfers,
    Mountpoint,
    Name,
    NewConnection,
    NoMountedConnections,
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
    Source,
    SshConfigFile,
    SshConfigPathRequired,
    SshHost,
    SshHostAlias,
    TransferCenter,
    TransferCompleted,
    TransferStateUnavailable,
    Transfers,
    Transport,
    Unmount,
    UnmountBeforeRemove,
    User,
    VfsCacheMode,
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
        TextKey::CopyPrivateKey => "Copy the private key into ~/.ssh",
        TextKey::DirectoryCacheTime => "Directory cache time",
        TextKey::Edit => "Edit",
        TextKey::EditConnection => "Edit connection",
        TextKey::FileManagerIntegration => "Explorer integration",
        TextKey::FileManagerIntegrationHelp => {
            "Adds Refresh and Transfers to folder and drive context menus for the current user."
        }
        TextKey::FileManagerMenuRegistered => "Explorer context-menu commands registered",
        TextKey::FileManagerMenuRemoved => "Explorer context-menu commands removed",
        TextKey::FileTransfer => "File transfer",
        TextKey::Import => "Import",
        TextKey::ImportSshConfig => "Import SSH config",
        TextKey::IpHost => "IP / Host",
        TextKey::KeyPassphrase => "Key passphrase",
        TextKey::Language => "Language",
        TextKey::Load => "Load",
        TextKey::LoadSshBeforeImport => "Load an SSH config before importing",
        TextKey::Loading => "Loading...",
        TextKey::LoadingMountStatus => "Loading mount status...",
        TextKey::ManagedByOpenSsh => "Private key (managed by OpenSSH)",
        TextKey::MaximumAge => "Maximum age",
        TextKey::MaximumSize => "Maximum size",
        TextKey::MinimumFreeSpace => "Minimum free space",
        TextKey::Mount => "Mount",
        TextKey::MountAllAtLogin => "Mount all saved connections at login",
        TextKey::MountConnectionForTransfers => {
            "Mount a connection to inspect its cloud transfer state"
        }
        TextKey::Mountpoint => "Mountpoint (Auto by default)",
        TextKey::Name => "Name",
        TextKey::NewConnection => "New connection",
        TextKey::NoMountedConnections => "No mounted connections",
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
        TextKey::RegisterFileManagerMenu => "Register Explorer commands",
        TextKey::RegisteringFileManagerMenu => "Registering Explorer commands...",
        TextKey::RemotePath => "Remote path ($HOME by default)",
        TextKey::Remove => "Remove",
        TextKey::RemoveFileManagerMenu => "Remove Explorer commands",
        TextKey::RemovingFileManagerMenu => "Removing Explorer commands...",
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
        TextKey::Source => "Source",
        TextKey::SshConfigFile => "SSH config file",
        TextKey::SshConfigPathRequired => "SSH config path is required",
        TextKey::SshHost => "SSH Host",
        TextKey::SshHostAlias => "SSH Host alias",
        TextKey::TransferCenter => "Transfer center",
        TextKey::TransferCompleted => "Transfer completed",
        TextKey::TransferStateUnavailable => "Transfer state unavailable",
        TextKey::Transfers => "Transfers",
        TextKey::Transport => "Transport",
        TextKey::Unmount => "Unmount",
        TextKey::UnmountBeforeRemove => "Unmount the connection before removing it",
        TextKey::User => "User",
        TextKey::VfsCacheMode => "VFS cache mode",
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
        TextKey::CopyPrivateKey => "将私钥复制到 ~/.ssh",
        TextKey::DirectoryCacheTime => "目录缓存时间",
        TextKey::Edit => "编辑",
        TextKey::EditConnection => "编辑连接",
        TextKey::FileManagerIntegration => "资源管理器集成",
        TextKey::FileManagerIntegrationHelp => {
            "为当前用户的文件夹和驱动器右键菜单添加刷新与传输中心命令。"
        }
        TextKey::FileManagerMenuRegistered => "资源管理器右键菜单已注册",
        TextKey::FileManagerMenuRemoved => "资源管理器右键菜单已移除",
        TextKey::FileTransfer => "文件传输",
        TextKey::Import => "导入",
        TextKey::ImportSshConfig => "导入 SSH 配置",
        TextKey::IpHost => "IP / 主机名",
        TextKey::KeyPassphrase => "私钥密码",
        TextKey::Language => "语言",
        TextKey::Load => "加载",
        TextKey::LoadSshBeforeImport => "请先加载 SSH 配置再导入",
        TextKey::Loading => "正在加载...",
        TextKey::LoadingMountStatus => "正在加载挂载状态...",
        TextKey::ManagedByOpenSsh => "私钥（由 OpenSSH 管理）",
        TextKey::MaximumAge => "最长保留时间",
        TextKey::MaximumSize => "最大大小",
        TextKey::MinimumFreeSpace => "最小剩余空间",
        TextKey::Mount => "挂载",
        TextKey::MountAllAtLogin => "登录时挂载所有已保存连接",
        TextKey::MountConnectionForTransfers => "挂载连接后可查看其云端传输状态",
        TextKey::Mountpoint => "挂载点（默认自动选择）",
        TextKey::Name => "名称",
        TextKey::NewConnection => "新建连接",
        TextKey::NoMountedConnections => "没有已挂载的连接",
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
        TextKey::RegisterFileManagerMenu => "注册资源管理器命令",
        TextKey::RegisteringFileManagerMenu => "正在注册资源管理器命令...",
        TextKey::RemotePath => "远端路径（默认为 $HOME）",
        TextKey::Remove => "删除",
        TextKey::RemoveFileManagerMenu => "移除资源管理器命令",
        TextKey::RemovingFileManagerMenu => "正在移除资源管理器命令...",
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
        TextKey::Source => "来源",
        TextKey::SshConfigFile => "SSH 配置文件",
        TextKey::SshConfigPathRequired => "必须填写 SSH 配置路径",
        TextKey::SshHost => "SSH Host",
        TextKey::SshHostAlias => "SSH Host 别名",
        TextKey::TransferCenter => "传输中心",
        TextKey::TransferCompleted => "传输已完成",
        TextKey::TransferStateUnavailable => "传输状态不可用",
        TextKey::Transfers => "传输",
        TextKey::Transport => "传输方式",
        TextKey::Unmount => "卸载",
        TextKey::UnmountBeforeRemove => "请先卸载连接再删除",
        TextKey::User => "用户",
        TextKey::VfsCacheMode => "VFS 缓存模式",
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
}
