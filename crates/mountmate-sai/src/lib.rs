//! SAI account setup adapter. Produces ordinary MountMate connections.
//! See docs/sai-account-setup.md for the proposed, not yet deployed service contract.

mod enrollment;
mod key;
mod transport;

pub use enrollment::EnrollmentProgress;
use enrollment::{Enrollment, Status as EnrollmentStatus};

use mountmate_core::connection::{ConnectionDraft, ConnectionSource};
use mountmate_core::credential::CredentialKind;
use mountmate_core::{AuthMethod, ConnectionMethod};
use oauth2::basic::{BasicClient, BasicTokenType};
use oauth2::{
    ClientId, DeviceAuthorizationUrl, DeviceCodeErrorResponseType, EndpointNotSet, EndpointSet,
    RequestTokenError, Scope, StandardDeviceAuthorizationResponse, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use url::Url;
use zeroize::Zeroizing;

pub const PORTAL: &str = "https://www.open-sai.com";
const ISSUER: &str = "https://id.open-sai.com/realms/sai";
const CLIENT_ID: &str = "ssh-mountmate";
const RESOURCE: &str = "sai-portal";
const SCOPES: [&str; 3] = ["openid", "sai.ssh.read", "sai.ssh.keys.write"];
const LOGIN_LIMIT: Duration = Duration::from_secs(900);
const SYNC_LIMIT: Duration = Duration::from_secs(120);
type DeviceClient =
    BasicClient<EndpointNotSet, EndpointSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

/// Never includes server bodies, OAuth secrets or request URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("This service has not enabled SAI account setup for MountMate yet.")]
    Unavailable,
    #[error("The service returned incompatible or unsafe connection settings.")]
    Contract,
    #[error("Could not reach the service. Check your connection and retry.")]
    Network,
    #[error("SAI authorization failed. Start sign-in again.")]
    Authorization,
    #[error("SAI authorization was denied.")]
    Denied,
    #[error("SAI authorization expired. Start sign-in again.")]
    Expired,
    #[error("The account cannot perform this action. Check SSH access in the SAI console.")]
    Forbidden,
    #[error("Could not securely create or read the local SSH key.")]
    KeyStorage,
    #[error("The public key is still being applied. Retry with the same local key.")]
    SyncPending,
    #[error("Browser confirmation is still required. Retry with the same local key.")]
    VerificationPending,
    #[error("The service could not apply this public key. Check the SAI console.")]
    SyncFailed,
    #[error("Browser confirmation expired. Retry to confirm the same local key.")]
    ApprovalExpired,
    #[error("This SSH key was revoked. Authorize with a new key to connect again.")]
    KeyRevoked,
    #[error("This SSH key expired. Authorize with a new key to connect again.")]
    KeyExpired,
}

#[derive(Clone)]
pub struct Client {
    http: transport::Http,
    portal: Url,
    issuer: Url,
    oauth: DeviceClient,
    browser_verification: bool,
}
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SaiClient")
    }
}
#[derive(Deserialize)]
struct Configuration {
    schema_version: String,
    identity_issuer: String,
    oidc_client_id: String,
    oidc_resource: String,
    ssh_access: String,
}
#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    token_endpoint: String,
    device_authorization_endpoint: String,
}
#[derive(Clone)]
pub struct Login {
    details: StandardDeviceAuthorizationResponse,
    deadline: Instant,
}
impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SaiLogin(<redacted>)")
    }
}
impl Login {
    pub fn verification_url(&self) -> &str {
        self.details.verification_uri().as_str()
    }
    pub fn user_code(&self) -> &str {
        self.details.user_code().secret()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Connection {
    pub site_id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
}
impl std::fmt::Display for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}@{}:{})",
            self.name, self.username, self.host, self.port
        )
    }
}
#[derive(Deserialize)]
struct Directory {
    schema_version: String,
    // Stable Portal account identifier, never inferred from an OIDC claim.
    account_id: String,
    connections: Vec<Connection>,
}
pub struct Session {
    client: Client,
    token: Zeroizing<String>,
    expires_at: Instant,
    directory: Directory,
}
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SaiSession(<redacted>)")
    }
}
#[derive(Debug, Clone)]
pub struct ConfiguredConnection {
    connection: Connection,
    key_file: PathBuf,
}
impl ConfiguredConnection {
    /// Preserve mount paths and editor identity; reuse ordinary validation and save.
    pub fn apply_to(&self, draft: &mut ConnectionDraft) {
        draft.source = ConnectionSource::SaiCluster;
        if draft.name.is_empty() || draft.name == "SAI" {
            draft.name = format!(
                "SAI-{}-{}",
                self.connection.site_id, self.connection.username
            );
        }
        draft.host.clone_from(&self.connection.host);
        draft.user.clone_from(&self.connection.username);
        draft.port = self.connection.port.to_string();
        draft.auth = AuthMethod::Key;
        draft.connection_method = ConnectionMethod::Native;
        draft.key_file = self.key_file.to_string_lossy().into_owned();
        draft.password.clear();
        draft.key_passphrase.clear();
        draft.clear_preserved_secret(CredentialKind::Password);
        draft.clear_preserved_secret(CredentialKind::KeyPassphrase);
        draft.ssh_config_managed = false;
        draft.copy_key_to_ssh_dir = false;
    }
}

impl Client {
    pub async fn discover() -> Result<Self, Error> {
        Self::discover_at(
            Url::parse(PORTAL).map_err(|_| Error::Contract)?,
            Url::parse(ISSUER).map_err(|_| Error::Contract)?,
        )
        .await
    }
    async fn discover_at(portal: Url, issuer: Url) -> Result<Self, Error> {
        let http = transport::Http::new()?;
        let response = http
            .send(
                http.client.get(
                    portal
                        .join("/v1/auth/config?client=ssh-mountmate")
                        .map_err(|_| Error::Contract)?,
                ),
            )
            .await?;
        if matches!(response.status().as_u16(), 400 | 404 | 501 | 503) {
            return Err(Error::Unavailable);
        }
        let config: Configuration = transport::json(response)?;
        if config.schema_version != "ssh-mountmate.connection/v1"
            || config.identity_issuer != issuer.as_str().trim_end_matches('/')
            || config.oidc_client_id != CLIENT_ID
            || config.oidc_resource != RESOURCE
            || !matches!(
                config.ssh_access.as_str(),
                "public-key/v1" | "public-key/v2"
            )
        {
            return Err(Error::Contract);
        }
        let discovery: Discovery = transport::json(
            http.send(http.client.get(format!(
                "{}/.well-known/openid-configuration",
                config.identity_issuer
            )))
            .await?,
        )?;
        if discovery.issuer != config.identity_issuer {
            return Err(Error::Contract);
        }
        let token = trusted_url(&discovery.token_endpoint, &issuer)?;
        let device = trusted_url(&discovery.device_authorization_endpoint, &issuer)?;
        let oauth = BasicClient::new(ClientId::new(CLIENT_ID.into()))
            .set_token_uri(TokenUrl::from_url(token))
            .set_device_authorization_url(DeviceAuthorizationUrl::from_url(device));
        Ok(Self {
            http,
            portal,
            issuer,
            oauth,
            browser_verification: config.ssh_access == "public-key/v2",
        })
    }
    pub async fn start_login(&self) -> Result<Login, Error> {
        let details: StandardDeviceAuthorizationResponse = self
            .oauth
            .exchange_device_code()
            .add_scopes(SCOPES.map(|scope| Scope::new(scope.into())))
            .add_extra_param("resource", RESOURCE)
            .request_async(&self.http)
            .await
            .map_err(|_| Error::Authorization)?;
        trusted_url(details.verification_uri().as_str(), &self.issuer)?;
        // The browser receives the plain verification URL, not a URL containing the code.
        if details.expires_in().is_zero()
            || details.interval().is_zero()
            || details.interval() > LOGIN_LIMIT
            || !display_value(details.user_code().secret(), 128)
        {
            return Err(Error::Contract);
        }
        let deadline = Instant::now() + details.expires_in().min(LOGIN_LIMIT);
        Ok(Login { details, deadline })
    }
    /// Dropping this future cancels polling and in-flight HTTP requests.
    pub async fn finish_login(&self, login: &Login) -> Result<Session, Error> {
        let remaining = login
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or(Error::Expired)?;
        let request = self
            .oauth
            .exchange_device_access_token(&login.details)
            .add_extra_param("resource", RESOURCE)
            .set_max_backoff_interval(LOGIN_LIMIT);
        let token = tokio::time::timeout(remaining, async {
            tokio::time::sleep(login.details.interval()).await;
            request
                .request_async(&self.http, tokio::time::sleep, Some(remaining))
                .await
        })
        .await
        .map_err(|_| Error::Expired)?
        .map_err(|error| match error {
            RequestTokenError::ServerResponse(response) => match response.error() {
                DeviceCodeErrorResponseType::AccessDenied => Error::Denied,
                DeviceCodeErrorResponseType::ExpiredToken => Error::Expired,
                _ => Error::Authorization,
            },
            _ => Error::Authorization,
        })?;
        if token.access_token().secret().is_empty()
            || token.token_type() != &BasicTokenType::Bearer
            || token.scopes().is_some_and(|scopes| {
                SCOPES
                    .iter()
                    .any(|required| !scopes.iter().any(|s| s.as_str() == *required))
            })
        {
            return Err(Error::Authorization);
        }
        let lifetime = token
            .expires_in()
            .filter(|value| !value.is_zero())
            .ok_or(Error::Authorization)?;
        let token = Zeroizing::new(token.access_token().secret().clone());
        let expires_at = Instant::now() + lifetime.min(LOGIN_LIMIT);
        let directory: Directory = transport::json(
            self.http
                .send(
                    self.http
                        .client
                        .get(
                            self.portal
                                .join("/v1/account/ssh/connections")
                                .map_err(|_| Error::Contract)?,
                        )
                        .bearer_auth(token.as_str()),
                )
                .await?,
        )?;
        directory.validate()?;
        Ok(Session {
            client: self.clone(),
            token,
            expires_at,
            directory,
        })
    }
}
impl Directory {
    fn validate(&self) -> Result<(), Error> {
        let mut ids = HashSet::new();
        if self.schema_version != "ssh.connections/v1"
            || !display_value(&self.account_id, 255)
            || self.connections.len() > 100
            || self.connections.iter().any(|site| {
                !identifier(&site.site_id)
                    || !ids.insert(&site.site_id)
                    || !display_value(&site.name, 128)
                    || !identifier(&site.username)
                    || site.port == 0
                    || !valid_host(&site.host)
            })
        {
            return Err(Error::Contract);
        }
        Ok(())
    }
}
impl Session {
    pub fn connections(&self) -> &[Connection] {
        &self.directory.connections
    }
    /// Persist the key before registration; retries after restart use the same request ID.
    pub async fn enroll(
        &self,
        site: &Connection,
        data_dir: &Path,
    ) -> Result<ConfiguredConnection, Error> {
        self.enroll_with_progress(site, data_dir, |_| {}).await
    }

    /// Reports the one approved browser destination while continuing site polling.
    pub async fn enroll_with_progress(
        &self,
        site: &Connection,
        data_dir: &Path,
        on_progress: impl FnMut(EnrollmentProgress),
    ) -> Result<ConfiguredConnection, Error> {
        self.enroll_key(site, data_dir, false, on_progress).await
    }

    /// Explicit recovery: replace only a currently confirmed revoked or expired key.
    pub async fn reauthorize_with_progress(
        &self,
        site: &Connection,
        data_dir: &Path,
        on_progress: impl FnMut(EnrollmentProgress),
    ) -> Result<ConfiguredConnection, Error> {
        self.enroll_key(site, data_dir, true, on_progress).await
    }

    async fn enroll_key(
        &self,
        site: &Connection,
        data_dir: &Path,
        replace_retired: bool,
        mut on_progress: impl FnMut(EnrollmentProgress),
    ) -> Result<ConfiguredConnection, Error> {
        self.check_expiry()?;
        if !self.connections().contains(site) {
            return Err(Error::Contract);
        }
        let context = serde_json::to_vec(&(
            self.client.portal.as_str(),
            self.client.issuer.as_str(),
            &self.directory.account_id,
            &site.site_id,
        ))
        .map_err(|_| Error::Contract)?;
        let load_dir = data_dir.to_owned();
        let load_context = context.clone();
        let mut key =
            tokio::task::spawn_blocking(move || key::load_or_create(&load_dir, &load_context))
                .await
                .map_err(|_| Error::KeyStorage)??;
        let mut response = self.register_key(site, &key).await?;
        if replace_retired
            && matches!(
                response.validate(
                    &key.request_id,
                    &site.site_id,
                    &key.fingerprint,
                    &self.client.portal,
                    self.client.browser_verification,
                ),
                Err(Error::KeyRevoked | Error::KeyExpired)
            )
        {
            let data_dir = data_dir.to_owned();
            let retired = key.request_id.clone();
            key = tokio::task::spawn_blocking(move || key::renew(&data_dir, &context, &retired))
                .await
                .map_err(|_| Error::KeyStorage)??;
            response = self.register_key(site, &key).await?;
        }
        self.wait_for_key(response, &key, site, SYNC_LIMIT, &mut on_progress)
            .await?;
        Ok(ConfiguredConnection {
            connection: site.clone(),
            key_file: key.path,
        })
    }

    async fn register_key(
        &self,
        site: &Connection,
        key: &key::LocalKey,
    ) -> Result<Enrollment, Error> {
        self.check_expiry()?;
        let registration = Registration {
            request_id: &key.request_id,
            site_id: &site.site_id,
            public_key: &key.public_key,
            label: "SSH MountMate",
        };
        transport::json(
            self.client
                .http
                .send(
                    self.client
                        .http
                        .client
                        .post(
                            self.client
                                .portal
                                .join("/v1/account/ssh/keys")
                                .map_err(|_| Error::Contract)?,
                        )
                        .bearer_auth(self.token.as_str())
                        .json(&registration),
                )
                .await?,
        )
    }
    async fn wait_for_key(
        &self,
        mut response: Enrollment,
        key: &key::LocalKey,
        site: &Connection,
        limit: Duration,
        on_progress: &mut impl FnMut(EnrollmentProgress),
    ) -> Result<(), Error> {
        let mut verification_reported = false;
        let mut waiting_for_verification = false;
        let wait = async {
            loop {
                self.check_expiry()?;
                match response.validate(
                    &key.request_id,
                    &site.site_id,
                    &key.fingerprint,
                    &self.client.portal,
                    self.client.browser_verification,
                )? {
                    EnrollmentStatus::Applied => return Ok(()),
                    EnrollmentStatus::Pending(Some(url)) => {
                        waiting_for_verification = true;
                        if !verification_reported {
                            verification_reported = true;
                            on_progress(EnrollmentProgress::VerificationRequired(url));
                        }
                    }
                    EnrollmentStatus::Pending(None) => {
                        if waiting_for_verification {
                            on_progress(EnrollmentProgress::Applying);
                        }
                        waiting_for_verification = false;
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                self.check_expiry()?;
                response = transport::json(
                    self.client
                        .http
                        .send(
                            self.client
                                .http
                                .client
                                .get(
                                    self.client
                                        .portal
                                        .join(&format!("/v1/account/ssh/keys/{}", key.request_id))
                                        .map_err(|_| Error::Contract)?,
                                )
                                .bearer_auth(self.token.as_str()),
                        )
                        .await?,
                )?;
            }
        };
        tokio::time::timeout(limit, wait).await.map_err(|_| {
            if waiting_for_verification {
                Error::VerificationPending
            } else {
                Error::SyncPending
            }
        })?
    }
    fn check_expiry(&self) -> Result<(), Error> {
        if Instant::now() >= self.expires_at {
            Err(Error::Expired)
        } else {
            Ok(())
        }
    }
}
#[derive(Serialize)]
struct Registration<'a> {
    request_id: &'a str,
    site_id: &'a str,
    public_key: &'a str,
    label: &'a str,
}
fn trusted_url(value: &str, issuer: &Url) -> Result<Url, Error> {
    let url = Url::parse(value).map_err(|_| Error::Contract)?;
    if url.origin() != issuer.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Contract);
    }
    Ok(url)
}
fn display_value(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}
fn valid_host(value: &str) -> bool {
    value.parse::<std::net::IpAddr>().is_ok()
        || (value.len() <= 253
            && value.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }))
}
#[cfg(test)]
mod tests;
