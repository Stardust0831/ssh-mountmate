//! Editor-only SAI setup; authentication never enters the mount service or saved settings.
use crate::i18n::Locale;
use crate::program_log::Level;
use iced::widget::{button, column, container, pick_list, text};
use iced::{Element, Fill, Task};
use mountmate_sai::{
    Client, ConfiguredConnection, Connection, EnrollmentProgress, Error, Login, PORTAL, Session,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Message {
    Start,
    Cancel,
    LoginStarted(u64, Result<Box<(Client, Login)>, Error>),
    Authorized(u64, Result<Arc<Session>, Error>),
    Select(Connection),
    Enroll,
    Reauthorize,
    EnrollmentProgress(u64, EnrollmentProgress),
    Finished(u64, Result<ConfiguredConnection, Error>),
    OpenBrowser,
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
}
impl State {
    pub fn reset(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        *self = Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }
    pub fn in_progress(&self) -> bool {
        self.pending || self.session.is_some()
    }
    /// Only human-readable state goes to the program log. Device codes, URLs,
    /// tokens and connection/key material must stay out of its history.
    pub fn feedback(&self, locale: Locale) -> Option<(Level, String)> {
        let tr = |en: &str, zh: &str| {
            if locale == Locale::English {
                en.to_owned()
            } else {
                zh.to_owned()
            }
        };
        if self.browser_failed {
            return Some((
                Level::Error,
                tr(
                    "SAI: Could not open the browser. Open the address in the setup panel manually.",
                    "SAI：无法打开浏览器，请手动打开配置面板中的地址。",
                ),
            ));
        }
        if let Some(error) = self.error {
            return Some((Level::Error, error_text(error, locale)));
        }
        if self.ready {
            return Some((
                Level::Info,
                tr(
                    "SAI: Public key applied. Review the mount paths and save the connection.",
                    "SAI：公钥已生效，请检查挂载路径并保存连接。",
                ),
            ));
        }
        if self.login.is_some() {
            return Some((
                Level::Info,
                tr(
                    "SAI: Open the sign-in page and enter the code shown in the setup panel. Waiting for authorization…",
                    "SAI：请打开登录页面并输入配置面板中的登录码，正在等待授权…",
                ),
            ));
        }
        if let Some(session) = &self.session {
            if session.connections().is_empty() {
                return Some((
                    Level::Warning,
                    tr(
                        "SAI: Signed in, but no SSH sites are available. Check access in the SAI console.",
                        "SAI：登录成功，但此账号暂无可用 SSH 站点，请在 SAI 控制台查看接入权限。",
                    ),
                ));
            }
            return Some((
                Level::Info,
                if self.verification_url.is_some() {
                    tr(
                        "SAI: Confirm this connection in the browser to continue.",
                        "SAI：请在浏览器确认此连接，确认后会继续配置。",
                    )
                } else if self.pending {
                    tr(
                        "SAI: Registering the public key and waiting for the site to apply it…",
                        "SAI：正在登记公钥，等待站点确认生效…",
                    )
                } else {
                    tr(
                        "SAI: Signed in. Choose a site and continue setup.",
                        "SAI：登录成功，请选择站点并继续配置。",
                    )
                },
            ));
        }
        self.pending.then(|| {
            (
                Level::Info,
                tr(
                    "SAI: Checking service and starting sign-in…",
                    "SAI：正在检查服务并发起登录…",
                ),
            )
        })
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
        locale: Locale,
    ) -> (Task<Message>, Option<ConfiguredConnection>) {
        match message {
            Message::Start if !self.pending => {
                self.reset();
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
                        self.login = Some(login.clone());
                        let task = Task::perform(
                            async move { client.finish_login(&login).await.map(Arc::new) },
                            move |result| Message::Authorized(generation, result),
                        );
                        return (self.track(task), None);
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
                    self.selected = Some(site);
                    self.error = None;
                }
            }
            action @ (Message::Enroll | Message::Reauthorize) if !self.pending => {
                let replace_retired = matches!(action, Message::Reauthorize);
                if replace_retired && !self.needs_new_key() {
                    return (Task::none(), None);
                }
                if let (Some(session), Some(site)) = (self.session.clone(), self.selected.clone()) {
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
                        return (self.open_browser(&url, locale), None);
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
                        self.reset();
                        self.ready = true;
                        return (Task::none(), Some(configured));
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
                    return (self.open_browser(url, locale), None);
                }
            }
            Message::OpenConsole => return (self.open_browser(PORTAL, locale), None),
            Message::BrowserOpened(generation, ok) if generation == self.generation => {
                self.browser_failed = !ok
            }
            _ => {}
        }
        (Task::none(), None)
    }
    fn open_browser(&self, url: &str, locale: Locale) -> Task<Message> {
        let path = PathBuf::from(url);
        let generation = self.generation;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || super::open_path(&path, locale))
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
            content = content
                .push(text(tr(
                    "Open the page and enter this code:",
                    "打开登录页面并输入登录码：",
                )))
                .push(text(login.verification_url()).size(14))
                .push(text(login.user_code()).size(24))
                .push(
                    button(tr("Open sign-in page", "打开登录页面")).on_press(Message::OpenBrowser),
                )
                .push(text(tr("Waiting for authorization…", "正在等待授权…")));
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
                content = content
                    .push(
                        pick_list(
                            session.connections().to_vec(),
                            self.selected.clone(),
                            Message::Select,
                        )
                        .width(Fill),
                    )
                    .push(if self.needs_new_key() {
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
            content = content.push(
                container(text(error_text(error, locale)).size(16))
                    .padding(12)
                    .width(Fill)
                    .style(|theme| Level::Error.style(theme)),
            );
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
    fn unavailable_sign_in_produces_noticeable_feedback_and_ignores_stale_results() {
        let mut state = State {
            pending: true,
            ..State::default()
        };
        assert!(
            state
                .feedback(Locale::Chinese)
                .unwrap()
                .1
                .contains("正在检查服务")
        );
        let _ = state.update(
            Message::LoginStarted(state.generation, Err(Error::Unavailable)),
            Path::new("unused"),
            Locale::Chinese,
        );
        let (level, message) = state.feedback(Locale::Chinese).unwrap();
        assert_eq!(level, Level::Error);
        assert!(message.contains("尚未启用"));
        state.reset();
        let _ = state.update(
            Message::LoginStarted(state.generation.wrapping_sub(1), Err(Error::Network)),
            Path::new("unused"),
            Locale::Chinese,
        );
        assert_eq!(state.feedback(Locale::Chinese), None);
    }

    #[test]
    fn browser_feedback_does_not_copy_confirmation_urls_to_the_log() {
        let state = State {
            browser_failed: true,
            verification_url: Some("https://example.invalid/confirm?secret=fixture".into()),
            ..State::default()
        };
        let (level, message) = state.feedback(Locale::English).unwrap();
        assert_eq!(level, Level::Error);
        assert!(!message.contains("fixture"));
        assert!(!message.contains("https://"));
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
        );
        let _ = state.update(
            Message::LoginStarted(generation, Err(Error::Unavailable)),
            Path::new("unused"),
            Locale::English,
        );
        let _ = state.update(
            Message::Finished(generation, Err(Error::SyncFailed)),
            Path::new("unused"),
            Locale::English,
        );
        assert!(!state.in_progress());
        assert!(state.error.is_none());
        assert!(!state.ready);
        assert!(state.verification_url.is_none());
    }
}
