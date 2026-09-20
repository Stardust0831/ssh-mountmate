use iced::futures::{SinkExt, channel::mpsc};
use mountmate_core::host_key::HostKeyReview;
use mountmate_core::service::{MountService, ServiceError};
use mountmate_core::{MountState, ServerConfig, Settings};

use crate::Message;
use crate::i18n::Locale;

#[derive(Debug, Clone)]
pub struct Prompt {
    pub title: String,
    pub description: String,
    pub accept: String,
    pub cancel: String,
    pub reply: async_channel::Sender<bool>,
}

async fn ask(
    output: &mut mpsc::Sender<Message>,
    title: &str,
    description: String,
    accept: &str,
    cancel: &str,
) -> bool {
    let (reply, response) = async_channel::bounded(1);
    if output
        .send(Message::HostKeyPrompt(Prompt {
            title: title.into(),
            description,
            accept: accept.into(),
            cancel: cancel.into(),
            reply,
        }))
        .await
        .is_err()
    {
        return false;
    }
    response.recv().await.unwrap_or(false)
}

// Multiple directory mappings can encounter the same unknown server together.
// Serialize dialogs, then check whether a sibling already saved these keys.
static DIALOG_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn mount(
    service: MountService,
    server: ServerConfig,
    settings: Settings,
    locale: Locale,
    output: &mut mpsc::Sender<Message>,
) -> Result<Option<MountState>, String> {
    loop {
        let worker = service.clone();
        let target = server.clone();
        let options = settings.clone();
        let result = tokio::task::spawn_blocking(move || worker.mount(&target, &options))
            .await
            .map_err(|error| error.to_string())?;
        match result {
            Ok(state) => return Ok(Some(state)),
            Err(ServiceError::HostKeyConfirmation(review)) => {
                let _guard = DIALOG_LOCK.lock().await;
                if service.host_key_is_confirmed(&review) {
                    continue;
                }
                let (title, description, accept, cancel) =
                    review_text(locale, server.display_name(), &review);
                if !ask(output, title, description, accept, cancel).await {
                    return Ok(None);
                }
                let worker = service.clone();
                tokio::task::spawn_blocking(move || worker.confirm_host_key(&review))
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| crate::localize_service_error(locale, &error))?;
            }
            Err(error @ ServiceError::HostKeyProbe { .. }) => {
                let _guard = DIALOG_LOCK.lock().await;
                let (title, retry, cancel, explanation) = match locale {
                    Locale::English => (
                        "Could not read server fingerprint",
                        "Retry",
                        "Cancel",
                        "No server public key was received, so there is no fingerprint to confirm yet. Check the server address, SSH port and network connection, then retry. Your login credentials have not been used.",
                    ),
                    Locale::Chinese => (
                        "暂时无法读取服务器指纹",
                        "重试",
                        "取消",
                        "尚未获取到服务器公钥，因此暂时没有可确认的指纹。请检查服务器地址、SSH 端口和网络连接后重试。此探测过程未使用你的登录凭据。",
                    ),
                };
                if !ask(
                    output,
                    title,
                    format!("{}\n\n{explanation}\n\n{error}", server.display_name()),
                    retry,
                    cancel,
                )
                .await
                {
                    return Ok(None);
                }
            }
            Err(error) => return Err(crate::localize_service_error(locale, &error)),
        }
    }
}

fn review_text(
    locale: Locale,
    name: &str,
    review: &HostKeyReview,
) -> (&'static str, String, &'static str, &'static str) {
    let address = format!("{}:{}", review.host(), review.port());
    let fingerprints = review.fingerprints();
    match (locale, review.changed()) {
        (Locale::English, false) => (
            "Confirm server fingerprint",
            format!(
                "{name}\nServer: {address}\n\nThis server has no saved host key yet. Confirm its fingerprint to continue. This identifies the server; it is not your private key.\n\n{fingerprints}\n\nTrusting saves these keys in SSH MountMate and continues mounting. Cancel leaves the trust file unchanged."
            ),
            "Trust and mount",
            "Cancel",
        ),
        (Locale::Chinese, false) => (
            "确认服务器指纹",
            format!(
                "{name}\n服务器：{address}\n\n首次连接尚未保存过此服务器的指纹，这是正常情况。请确认下方指纹后继续。它用于识别服务器，并不是你的登录私钥。\n\n{fingerprints}\n\n确认后，SSH MountMate 会保存此指纹并继续挂载；取消不会保存。"
            ),
            "信任并挂载",
            "取消",
        ),
        (Locale::English, true) => (
            "Server fingerprint has changed",
            format!(
                "{name}\nServer: {address}\n\nThe server key differs from the saved key. This can follow a server reinstall or key rotation, but can also indicate an intercepted connection. Confirm the change with the server administrator before replacing it.\n\nSaved:\n{}\n\nReceived:\n{fingerprints}\n\nThis updates SSH MountMate's trust record only; your SSH config and known_hosts files remain unchanged.",
                review.previous_fingerprints()
            ),
            "Replace key and mount",
            "Cancel",
        ),
        (Locale::Chinese, true) => (
            "服务器指纹已变化",
            format!(
                "{name}\n服务器：{address}\n\n本次指纹与已保存的记录不一致。可能是服务器重装或更换密钥，也可能是连接遭到冒充。请向服务器管理员核实后再更新。\n\n原指纹：\n{}\n\n本次指纹：\n{fingerprints}\n\n确认后只更新 SSH MountMate 的信任记录，不修改你自己的 SSH 配置和 known_hosts 文件。",
                review.previous_fingerprints()
            ),
            "更新指纹并挂载",
            "取消",
        ),
    }
}
