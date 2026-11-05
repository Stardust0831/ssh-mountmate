//! Editor-only SAI setup; authentication never enters the mount service or saved settings.
use crate::i18n::Locale;
use iced::widget::{button, checkbox, column, container, pick_list, scrollable, text};
use iced::{Element, Fill, Task};
use mountmate_core::connection::ConnectionDraft;
use mountmate_sai::{
    Client, ConfiguredConnection, Connection, EnrollmentProgress, Error, Login, MountRole,
    MountRoot, MountRootPage, PORTAL, Session,
};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Message {
    Start,
    Cancel,
    LoginStarted(u64, Result<Box<(Client, Login)>, Error>),
    Authorized(u64, Result<Arc<Session>, Error>),
    Select(Connection),
    LoadMountRoots,
    MountRootsLoaded(u64, u64, Result<MountRootPage, Error>),
    ToggleMountRoot(String, bool),
    Enroll,
    Reauthorize,
    EnrollmentProgress(u64, EnrollmentProgress),
    Finished(u64, Result<ConfiguredConnection, Error>),
    OpenBrowser,
    ToggleDeviceCode,
    OpenConsole,
    BrowserOpened(u64, bool),
}
#[derive(Default)]
pub struct State {
    generation: u64,
    task: Option<iced::task::Handle>,
    login: Option<Login>,
    session: Option<Arc<Session>>,
    selected: Option<Connection>,
    pending: bool,
    ready: bool,
    error: Option<Error>,
    browser_failed: bool,
    verification_url: Option<String>,
    show_device_code: bool,
    roots: Vec<MountRoot>,
    selected_roots: HashSet<String>,
    roots_cursor: Option<String>,
    roots_loaded: bool,
    roots_pending: bool,
    roots_error: Option<Error>,
    roots_generation: u64,
    roots_task: Option<iced::task::Handle>,
    existing_mount_paths: HashSet<String>,
    mount_slots: usize,
}
impl State {
    pub fn reset(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.reset_roots();
        *self = Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }
    pub fn in_progress(&self) -> bool {
        self.pending || self.session.is_some()
    }
    fn needs_new_key(&self) -> bool {
        matches!(self.error, Some(Error::KeyRevoked | Error::KeyExpired))
    }
    fn track(&mut self, task: Task<Message>) -> Task<Message> {
        let (task, handle) = task.abortable();
        self.task = Some(handle.abort_on_drop());
        task
    }
    pub fn update(
        &mut self,
        message: Message,
        data_dir: &Path,
        _locale: Locale,
        draft: &ConnectionDraft,
    ) -> (Task<Message>, Option<ConfiguredConnection>) {
        match message {
            Message::Start if !self.pending => {
                self.reset();
                self.existing_mount_paths = draft
                    .mounts
                    .iter()
                    .map(|mapping| mapping.remote_path.clone())
                    .collect();
                if draft.primary_mount {
                    self.existing_mount_paths.insert(draft.remote_path.clone());
                }
                let unused_primary = draft.editing_id.is_none()
                    && draft.primary_mount
                    && draft.remote_path.is_empty()
                    && draft.mountpoint.is_empty();
                self.mount_slots = 64usize
                    .saturating_sub(draft.mounts.len() + usize::from(draft.primary_mount))
                    + usize::from(unused_primary);
                self.pending = true;
                let generation = self.generation;
                let task = Task::perform(
                    async {
                        let client = Client::discover().await?;
                        let login = client.start_login().await?;
                        Ok(Box::new((client, login)))
                    },
                    move |result| Message::LoginStarted(generation, result),
                );
                return (self.track(task), None);
            }
            Message::Cancel => self.reset(),
            Message::LoginStarted(generation, result) if generation == self.generation => {
                match result {
                    Ok(started) => {
                        let (client, login) = *started;
                        let open_browser = self.open_browser(login.verification_url());
                        self.login = Some(login.clone());
                        let task = Task::perform(
                            async move { client.finish_login(&login).await.map(Arc::new) },
                            move |result| Message::Authorized(generation, result),
                        );
                        return (Task::batch([self.track(task), open_browser]), None);
                    }
                    Err(error) => {
                        self.pending = false;
                        self.error = Some(error);
                    }
                }
            }
            Message::Authorized(generation, result) if generation == self.generation => {
                self.pending = false;
                self.login = None;
                match result {
                    Ok(session) => {
                        self.selected = session.connections().first().cloned();
                        self.session = Some(session);
                        return (self.load_roots(), None);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Message::Select(site) if !self.pending => {
                if self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.connections().contains(&site))
                {
                    self.reset_roots();
                    self.selected = Some(site);
                    self.error = None;
                    return (self.load_roots(), None);
                }
            }
            Message::LoadMountRoots if !self.pending && !self.roots_pending => {
                return (self.load_roots(), None);
            }
            Message::MountRootsLoaded(generation, request, result)
                if generation == self.generation
                    && request == self.roots_generation
                    && self.roots_pending =>
            {
                self.roots_pending = false;
                match result {
                    Ok(page)
                        if page
                            .items
                            .iter()
                            .all(|item| self.roots.iter().all(|old| old.id != item.id)) =>
                    {
                        self.roots.extend(page.items);
                        self.roots_cursor = page.next_cursor;
                        self.roots_loaded = true;
                    }
                    Ok(_) => self.roots_error = Some(Error::Contract),
                    Err(error) => self.roots_error = Some(error),
                }
            }
            Message::ToggleMountRoot(id, checked) if !self.pending => {
                if let Some(root) = self.roots.iter().find(|root| root.id == id) {
                    if checked
                        && (self.existing_mount_paths.contains(&root.path)
                            || self.additional_selections() < self.mount_slots)
                    {
                        self.selected_roots.insert(id);
                    } else if !checked {
                        self.selected_roots.remove(&id);
                    }
                }
            }
            action @ (Message::Enroll | Message::Reauthorize) if !self.pending => {
                let replace_retired = matches!(action, Message::Reauthorize);
                if replace_retired && !self.needs_new_key() {
                    return (Task::none(), None);
                }
                if let (Some(session), Some(site)) = (self.session.clone(), self.selected.clone()) {
                    if let Some(task) = self.roots_task.take() {
                        task.abort();
                    }
                    self.roots_pending = false;
                    self.roots_generation = self.roots_generation.wrapping_add(1);
                    self.pending = true;
                    self.error = None;
                    self.verification_url = None;
                    self.browser_failed = false;
                    self.generation = self.generation.wrapping_add(1);
                    let generation = self.generation;
                    let data_dir = data_dir.to_owned();
                    let task = Task::run(
                        iced::stream::channel(2, async move |mut output| {
                            use iced::futures::SinkExt;
                            let progress = |progress| {
                                let _ = output
                                    .try_send(Message::EnrollmentProgress(generation, progress));
                            };
                            let result = if replace_retired {
                                session
                                    .reauthorize_with_progress(&site, &data_dir, progress)
                                    .await
                            } else {
                                session
                                    .enroll_with_progress(&site, &data_dir, progress)
                                    .await
                            };
                            let _ = output.send(Message::Finished(generation, result)).await;
                        }),
                        |message| message,
                    );
                    return (self.track(task), None);
                }
            }
            Message::EnrollmentProgress(generation, progress)
                if generation == self.generation && self.pending =>
            {
                match progress {
                    EnrollmentProgress::VerificationRequired(url) => {
                        self.verification_url = Some(url.clone());
                        self.browser_failed = false;
                        return (self.open_browser(&url), None);
                    }
                    EnrollmentProgress::Applying => {
                        self.verification_url = None;
                        self.browser_failed = false;
                    }
                }
            }
            Message::Finished(generation, result) if generation == self.generation => {
                self.pending = false;
                self.verification_url = None;
                match result {
                    Ok(configured) => {
                        let roots = self
                            .roots
                            .iter()
                            .filter(|root| self.selected_roots.contains(&root.id))
                            .cloned()
                            .collect();
                        self.reset();
                        self.ready = true;
                        return (Task::none(), Some(configured.with_mount_roots(roots)));
                    }
                    Err(error) => {
                        if error == Error::Expired {
                            self.reset();
                        }
                        self.error = Some(error);
                    }
                }
            }
            Message::OpenBrowser => {
                if let Some(url) = self
                    .verification_url
                    .as_deref()
                    .or_else(|| self.login.as_ref().map(Login::verification_url))
                {
                    return (self.open_browser(url), None);
                }
            }
            Message::ToggleDeviceCode if self.login.is_some() => {
                self.show_device_code = !self.show_device_code;
            }
            Message::OpenConsole => return (self.open_browser(PORTAL), None),
            Message::BrowserOpened(generation, ok) if generation == self.generation => {
                self.browser_failed = !ok
            }
            _ => {}
        }
        (Task::none(), None)
    }
    fn reset_roots(&mut self) {
        if let Some(task) = self.roots_task.take() {
            task.abort();
        }
        self.roots_generation = self.roots_generation.wrapping_add(1);
        self.roots.clear();
        self.selected_roots.clear();
        self.roots_cursor = None;
        self.roots_loaded = false;
        self.roots_pending = false;
        self.roots_error = None;
    }
    fn additional_selections(&self) -> usize {
        self.roots
            .iter()
            .filter(|root| {
                self.selected_roots.contains(&root.id)
                    && !self.existing_mount_paths.contains(&root.path)
            })
            .map(|root| &root.path)
            .collect::<HashSet<_>>()
            .len()
    }
    fn load_roots(&mut self) -> Task<Message> {
        let (Some(session), Some(site)) = (self.session.clone(), self.selected.clone()) else {
            return Task::none();
        };
        if !session.has_mount_directory_discovery()
            || (self.roots_loaded && self.roots_cursor.is_none())
        {
            return Task::none();
        }
        self.roots_generation = self.roots_generation.wrapping_add(1);
        let (generation, request) = (self.generation, self.roots_generation);
        let cursor = self.roots_cursor.clone();
        self.roots_pending = true;
        self.roots_error = None;
        let (task, handle) = Task::perform(
            async move { session.mount_roots(&site, cursor.as_deref()).await },
            move |result| Message::MountRootsLoaded(generation, request, result),
        )
        .abortable();
        self.roots_task = Some(handle.abort_on_drop());
        task
    }
    fn open_browser(&self, url: &str) -> Task<Message> {
        let url = url.to_owned();
        let generation = self.generation;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || mountmate_platform::open_browser(&url))
                    .await
                    .is_ok_and(|result| result.is_ok())
            },
            move |ok| Message::BrowserOpened(generation, ok),
        )
    }
    pub fn view(&self, locale: Locale) -> Element<'_, Message> {
        let tr = |en, zh| match locale {
            Locale::English => en,
            Locale::Chinese => zh,
        };
        let mut content = column![
            text(tr("Set up with a SAI account", "使用 SAI 账号配置")).size(18),
            text(tr("Sign in, choose a site, and register this device's public key to fill the connection settings.",
                "登录后选择站点，登记本机公钥，自动填写连接信息。")).size(14),
        ].spacing(10);
        if let Some(login) = &self.login {
            if !login.has_complete_verification_url()
                || self.show_device_code
                || self.browser_failed
            {
                content = content
                    .push(text(login.manual_verification_url()).size(14))
                    .push(text(login.user_code()).size(24));
            }
            content = content
                .push(
                    button(tr("Open sign-in page", "打开登录页面")).on_press(Message::OpenBrowser),
                )
                .push(text(tr("Waiting for authorization…", "正在等待授权…")));
            if login.has_complete_verification_url() {
                content = content.push(
                    button(if self.show_device_code {
                        tr("Hide sign-in code", "收起登录码")
                    } else {
                        tr("Use another device", "在其他设备登录")
                    })
                    .on_press(Message::ToggleDeviceCode),
                );
            }
            if self.browser_failed {
                content = content.push(text(tr(
                    "Could not open the browser. Open the address above manually.",
                    "无法打开浏览器，请手动打开上方地址。",
                )));
            }
        } else if let Some(session) = &self.session {
            if session.connections().is_empty() {
                content = content.push(text(tr(
                    "You are signed in, but this account has no available SSH sites. Check access in the SAI console.",
                    "登录成功，但此账号暂无可用的 SSH 站点。请在 SAI 控制台查看接入权限。",
                )))
                .push(button(tr("Open SAI console", "打开 SAI 控制台")).on_press(Message::OpenConsole));
            } else if self.pending {
                if let Some(url) = &self.verification_url {
                    content = content
                        .push(text(tr(
                            "Confirm this connection in the browser. Setup will continue automatically after confirmation.",
                            "请在浏览器确认此连接，确认后会自动完成配置。",
                        )))
                        .push(button(tr("Open confirmation page", "打开确认页面")).on_press(Message::OpenBrowser));
                    if self.browser_failed {
                        content = content
                            .push(text(tr(
                                "Could not open the browser. Open this address manually:",
                                "无法打开浏览器，请手动打开以下地址：",
                            )))
                            .push(text(url).size(14));
                    }
                } else {
                    content = content.push(text(tr(
                        "Registering the public key and waiting for the site to apply it…",
                        "正在登记公钥，等待站点确认生效…",
                    )));
                }
            } else {
                content = content.push(
                    pick_list(
                        session.connections().to_vec(),
                        self.selected.clone(),
                        Message::Select,
                    )
                    .width(Fill),
                );
                if session.has_mount_directory_discovery() {
                    content = content.push(self.directory_view(locale));
                }
                content = content.push(if self.needs_new_key() {
                    button(tr("Authorize with a new key", "使用新密钥重新授权"))
                        .on_press(Message::Reauthorize)
                } else {
                    button(tr("Continue setup", "继续配置")).on_press(Message::Enroll)
                });
                if self.needs_new_key() {
                    content = content.push(
                        text(tr(
                            "The previous private key stays available for existing connections.",
                            "原私钥会保留，已有连接不会被改写。",
                        ))
                        .size(14),
                    );
                }
            }
            content = content.push(text(tr("The private key stays on this device. After registration, cancelling setup or deleting a connection does not revoke access; remove the key in the SAI console.",
                "私钥保存在本机。登记后，取消配置或删除连接不会撤销授权；请在 SAI 控制台移除公钥。")).size(14));
        } else if self.pending {
            content = content.push(text(tr(
                "Checking service and starting sign-in…",
                "正在检查服务并发起登录…",
            )));
        } else {
            if self.ready {
                content = content.push(text(tr(
                    "Public key applied. Review the mount paths below and save the connection.",
                    "公钥已生效。请检查下方挂载路径并保存连接。",
                )));
            }
            content = content
                .push(button(tr("Sign in to SAI", "登录 SAI 账号")).on_press(Message::Start));
        }
        if let Some(error) = self.error {
            content = content.push(text(error_text(error, locale)));
            if matches!(
                error,
                Error::Forbidden | Error::KeyRevoked | Error::KeyExpired | Error::SyncFailed
            ) {
                content = content.push(
                    button(tr("Open SAI console", "打开 SAI 控制台"))
                        .on_press(Message::OpenConsole),
                );
            }
        }
        if self.browser_failed && self.login.is_none() && self.verification_url.is_none() {
            content = content
                .push(text(tr(
                    "Could not open the browser. Open the SAI console manually:",
                    "无法打开浏览器，请手动打开 SAI 控制台：",
                )))
                .push(text(PORTAL).size(14));
        }
        if self.in_progress() {
            content = content
                .push(button(tr("Cancel account setup", "取消账号配置")).on_press(Message::Cancel));
        }
        container(content)
            .padding(16)
            .width(Fill)
            .style(container::rounded_box)
            .into()
    }
    fn directory_view(&self, locale: Locale) -> Element<'_, Message> {
        let tr = |en, zh| if locale == Locale::Chinese { zh } else { en };
        let mut content =
            column![text(tr("Mount directories (optional)", "挂载目录（可选）"))].spacing(8);
        let mut choices = column![].spacing(8);
        let has_room = self.additional_selections() < self.mount_slots;
        for root in &self.roots {
            let selected = self.selected_roots.contains(&root.id);
            let id = root.id.clone();
            let role = match root.role {
                MountRole::Read => tr("Read only", "只读"),
                MountRole::Write => tr("Read / write", "读写"),
            };
            let choice = checkbox(selected)
                .label(format!("{} · {role}", root.display_name))
                .width(Fill)
                .text_wrapping(text::Wrapping::WordOrGlyph);
            choices = choices.push(
                column![
                    choice.on_toggle_maybe(
                        (selected || has_room || self.existing_mount_paths.contains(&root.path))
                            .then_some(move |checked| {
                                Message::ToggleMountRoot(id.clone(), checked)
                            })
                    ),
                    text(&root.path)
                        .size(13)
                        .width(Fill)
                        .wrapping(text::Wrapping::WordOrGlyph),
                ]
                .spacing(3),
            );
        }
        if !self.roots.is_empty() {
            content = content.push(
                scrollable(choices.width(Fill))
                    .width(Fill)
                    .height(iced::Length::Fixed(200.0)),
            );
        }
        if self.roots_pending {
            content = content.push(text(tr("Loading directories…", "正在读取目录…")));
        } else if let Some(error) = self.roots_error {
            let message = match error {
                Error::Unavailable => tr(
                    "Directory discovery is unavailable. Enter paths manually or retry.",
                    "目录发现暂不可用。可手动填写路径或重试。",
                ),
                Error::Forbidden => tr(
                    "Directory access was not authorized. Enter paths manually or sign in again.",
                    "未获目录读取授权。可手动填写路径或重新登录。",
                ),
                Error::Network => tr(
                    "Could not load directories. Check your connection and retry.",
                    "无法读取目录，请检查网络后重试。",
                ),
                Error::Contract => tr(
                    "The directory response is incompatible. Contact the SAI service administrator.",
                    "目录数据不兼容，请联系 SAI 服务管理员。",
                ),
                _ => {
                    return column![
                        content,
                        text(error_text(error, locale)),
                        button(tr("Sign in again", "重新登录")).on_press(Message::Start)
                    ]
                    .into();
                }
            };
            content = content.push(text(message));
            content = content.push(if error == Error::Forbidden {
                button(tr("Sign in again", "重新登录")).on_press(Message::Start)
            } else {
                button(tr("Retry directories", "重试读取目录")).on_press(Message::LoadMountRoots)
            });
        } else if self.roots_cursor.is_some() {
            content = content
                .push(button(tr("More directories", "更多目录")).on_press(Message::LoadMountRoots));
        } else if self.roots_loaded && self.roots.is_empty() {
            content = content.push(text(tr("No available directories.", "暂无可用目录。")));
        }
        content.into()
    }
}
fn error_text(error: Error, locale: Locale) -> String {
    if locale == Locale::English {
        return error.to_string();
    }
    match error {
        Error::Unavailable => "SAI 服务尚未启用 MountMate 账号配置。你仍可在下方手动填写连接信息。",
        Error::Contract => "服务返回的连接信息不兼容或不可信，请联系 SAI 服务管理员。",
        Error::Network => "无法访问服务，请检查网络后重试。登记重试会使用原来的本机密钥。",
        Error::Authorization => "SAI 授权失败，请重新登录。",
        Error::Denied => "SAI 授权被拒绝。",
        Error::Expired => "SAI 授权已过期，请重新登录。",
        Error::Forbidden => "此账号无法执行该操作，请在 SAI 控制台检查 SSH 接入权限。",
        Error::KeyStorage => "无法安全创建或读取本机 SSH 密钥，请检查应用数据目录权限。",
        Error::SyncPending => "公钥仍在同步，请重试。重试会使用原来的本机密钥。",
        Error::VerificationPending => "仍需在浏览器确认此连接，请重试。重试会使用原来的本机密钥。",
        Error::SyncFailed => "服务未能将公钥应用到站点，请在 SAI 控制台检查。",
        Error::ApprovalExpired => "浏览器确认已过期，请重试并重新确认。重试会使用原来的本机密钥。",
        Error::KeyRevoked => "此设备的 SSH 密钥已被撤销。如需再次连接，请使用新密钥重新授权。",
        Error::KeyExpired => "此设备的 SSH 密钥已过期。请使用新密钥重新授权。",
    }
    .into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_selection_uses_the_existing_editors_remaining_mapping_slots() {
        let mut draft = ConnectionDraft::default();
        draft.editing_id = Some("existing".into());
        draft.mounts = (0..62)
            .map(|_| mountmate_core::model::MountMapping::default())
            .collect();
        let mut state = State::default();
        let _ = state.update(Message::Start, Path::new("unused"), Locale::English, &draft);
        state.pending = false;
        state.roots = (0..2)
            .map(|index| MountRoot {
                id: format!("directory-{index}"),
                display_name: format!("Directory {index}"),
                kind: mountmate_sai::MountRootKind::ProjectRoot,
                path: format!("/data/{index}"),
                role: MountRole::Read,
            })
            .collect();
        for id in ["directory-0", "directory-1"] {
            let _ = state.update(
                Message::ToggleMountRoot(id.into(), true),
                Path::new("unused"),
                Locale::English,
                &draft,
            );
        }
        assert_eq!(state.selected_roots, HashSet::from(["directory-0".into()]));
        state.reset();
        assert!(state.selected_roots.is_empty());
    }
    #[test]
    fn cancelled_generation_ignores_late_results() {
        let mut state = State::default();
        let generation = state.generation;
        state.reset();
        let _ = state.update(
            Message::EnrollmentProgress(
                generation,
                EnrollmentProgress::VerificationRequired("https://www.open-sai.com/portal/".into()),
            ),
            Path::new("unused"),
            Locale::English,
            &ConnectionDraft::default(),
        );
        let _ = state.update(
            Message::LoginStarted(generation, Err(Error::Unavailable)),
            Path::new("unused"),
            Locale::English,
            &ConnectionDraft::default(),
        );
        let _ = state.update(
            Message::Finished(generation, Err(Error::SyncFailed)),
            Path::new("unused"),
            Locale::English,
            &ConnectionDraft::default(),
        );
        let _ = state.update(
            Message::BrowserOpened(generation, false),
            Path::new("unused"),
            Locale::English,
            &ConnectionDraft::default(),
        );
        state.roots_pending = true;
        let _ = state.update(
            Message::MountRootsLoaded(generation, state.roots_generation, Err(Error::Forbidden)),
            Path::new("unused"),
            Locale::English,
            &ConnectionDraft::default(),
        );
        assert!(state.roots_error.is_none());
        let old_request = state.roots_generation;
        state.reset_roots();
        state.roots_pending = true;
        let _ = state.update(
            Message::MountRootsLoaded(state.generation, old_request, Err(Error::Forbidden)),
            Path::new("unused"),
            Locale::English,
            &ConnectionDraft::default(),
        );
        assert!(state.roots_error.is_none());
        assert!(!state.in_progress());
        assert!(state.error.is_none());
        assert!(!state.ready);
        assert!(state.verification_url.is_none());
        assert!(!state.browser_failed);
    }
}
