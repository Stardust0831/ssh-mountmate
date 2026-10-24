use crate::Error;
use oauth2::{HttpRequest, HttpResponse};
use serde::de::DeserializeOwned;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct Http {
    pub client: reqwest::Client,
}
impl<'c> oauth2::AsyncHttpClient<'c> for Http {
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, Error>> + Send + 'c>>;
    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(self.oauth(request))
    }
}
impl Http {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                // Preserve normal system proxy behavior; never redirect credentials.
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(20))
                .user_agent(concat!("SSH-MountMate/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|_| Error::Network)?,
        })
    }
    pub async fn send(&self, request: reqwest::RequestBuilder) -> Result<HttpResponse, Error> {
        self.execute(request.build().map_err(|_| Error::Contract)?)
            .await
    }
    async fn oauth(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        self.execute(request.try_into().map_err(|_| Error::Contract)?)
            .await
    }
    async fn execute(&self, request: reqwest::Request) -> Result<HttpResponse, Error> {
        let mut response = self
            .client
            .execute(request)
            .await
            .map_err(|_| Error::Network)?;
        let mut builder = oauth2::http::Response::builder().status(response.status());
        for (name, value) in response.headers() {
            builder = builder.header(name, value);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
            if body.len().saturating_add(chunk.len()) > 128 * 1024 {
                return Err(Error::Contract);
            }
            body.extend_from_slice(&chunk);
        }
        builder.body(body).map_err(|_| Error::Contract)
    }
}
pub(crate) fn json<T: DeserializeOwned>(response: HttpResponse) -> Result<T, Error> {
    match response.status().as_u16() {
        200..=299 => serde_json::from_slice(response.body()).map_err(|_| Error::Contract),
        401 => Err(Error::Expired),
        403 => Err(Error::Forbidden),
        _ => Err(Error::Network),
    }
}
