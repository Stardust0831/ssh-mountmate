use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use reqwest::blocking::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;

use crate::transfer::{
    CoreStatsResponse, QueueResponse, TransferSnapshot, VfsStatsResponse, build_transfer_snapshot,
};

#[derive(Debug, Error)]
pub enum RcError {
    #[error("invalid or non-loopback rclone RC address: {0}")]
    InvalidAddress(String),
    #[error("rclone RC {method} failed: {message}")]
    Request { method: String, message: String },
    #[error("rclone RC {method} returned invalid JSON: {message}")]
    InvalidResponse { method: String, message: String },
}

pub trait RcApi {
    fn call(&self, method: &str, params: Value) -> Result<Value, RcError>;
}

pub struct HttpRcClient {
    base_url: String,
    client: Client,
}

impl HttpRcClient {
    pub fn new(rc_addr: &str, timeout: Duration) -> Result<Self, RcError> {
        let address: SocketAddr = rc_addr
            .parse()
            .map_err(|_| RcError::InvalidAddress(rc_addr.into()))?;
        if !is_loopback(address.ip()) {
            return Err(RcError::InvalidAddress(rc_addr.into()));
        }
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| RcError::Request {
                method: "client initialization".into(),
                message: error.to_string(),
            })?;
        Ok(Self {
            base_url: format!("http://{address}"),
            client,
        })
    }

    pub fn transfer_snapshot(&self) -> Result<TransferSnapshot, RcError> {
        transfer_snapshot(self)
    }

    pub fn refresh_remote(
        &self,
        remote: &str,
        relative_dir: &str,
        recursive: bool,
    ) -> Result<RefreshResult, RcError> {
        refresh_remote_snapshot(self, remote, relative_dir, recursive)
    }
}

impl RcApi for HttpRcClient {
    fn call(&self, method: &str, params: Value) -> Result<Value, RcError> {
        let url = format!("{}/{method}", self.base_url);
        let response = self
            .client
            .post(url)
            .json(&params)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| RcError::Request {
                method: method.into(),
                message: error.to_string(),
            })?;
        response.json().map_err(|error| RcError::InvalidResponse {
            method: method.into(),
            message: error.to_string(),
        })
    }
}

fn is_loopback(address: IpAddr) -> bool {
    address.is_loopback()
}

fn decode<T: DeserializeOwned>(method: &str, value: Value) -> Result<T, RcError> {
    serde_json::from_value(value).map_err(|error| RcError::InvalidResponse {
        method: method.into(),
        message: error.to_string(),
    })
}

pub fn transfer_snapshot(api: &impl RcApi) -> Result<TransferSnapshot, RcError> {
    let queue: QueueResponse = decode("vfs/queue", api.call("vfs/queue", json!({}))?)?;
    let vfs: VfsStatsResponse = decode("vfs/stats", api.call("vfs/stats", json!({}))?)?;
    let core: CoreStatsResponse = decode("core/stats", api.call("core/stats", json!({}))?)?;
    Ok(build_transfer_snapshot(queue, vfs, core))
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshResult {
    pub pending_uploads: usize,
    pub entries: Vec<Value>,
}

pub fn refresh_remote_snapshot(
    api: &impl RcApi,
    remote: &str,
    relative_dir: &str,
    recursive: bool,
) -> Result<RefreshResult, RcError> {
    let queue: QueueResponse = decode("vfs/queue", api.call("vfs/queue", json!({}))?)?;
    let path_params = if relative_dir.is_empty() {
        json!({})
    } else {
        json!({"dir": relative_dir})
    };
    api.call("vfs/forget", path_params.clone())?;
    let mut refresh_params = path_params;
    if recursive {
        refresh_params["recursive"] = Value::Bool(true);
    }
    api.call("vfs/refresh", refresh_params)?;
    let listing = api.call(
        "operations/list",
        json!({"fs": remote, "remote": relative_dir}),
    )?;
    let entries = listing
        .get("list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(RefreshResult {
        pending_uploads: queue.queue.len(),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;

    struct FakeRc {
        calls: RefCell<Vec<(String, Value)>>,
        responses: RefCell<VecDeque<Value>>,
    }

    impl FakeRc {
        fn new(responses: impl IntoIterator<Item = Value>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                responses: RefCell::new(responses.into_iter().collect()),
            }
        }
    }

    impl RcApi for FakeRc {
        fn call(&self, method: &str, params: Value) -> Result<Value, RcError> {
            self.calls.borrow_mut().push((method.into(), params));
            Ok(self.responses.borrow_mut().pop_front().unwrap_or(json!({})))
        }
    }

    #[test]
    fn rc_rejects_non_loopback_addresses() {
        assert!(matches!(
            HttpRcClient::new("192.0.2.1:5572", Duration::from_secs(1)),
            Err(RcError::InvalidAddress(_))
        ));
        assert!(HttpRcClient::new("127.0.0.1:5572", Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn refresh_forgets_refreshes_then_verifies_remote_listing() {
        let api = FakeRc::new([
            json!({"queue": [{"name": "pending.bin"}]}),
            json!({}),
            json!({}),
            json!({"list": [{"Name": "remote.bin"}]}),
        ]);
        let result = refresh_remote_snapshot(&api, "alpha:folder", "subdir", false).unwrap();
        assert_eq!(result.pending_uploads, 1);
        assert_eq!(result.entries.len(), 1);
        let calls = api.calls.borrow();
        assert_eq!(
            calls
                .iter()
                .map(|(method, _)| method.as_str())
                .collect::<Vec<_>>(),
            ["vfs/queue", "vfs/forget", "vfs/refresh", "operations/list"]
        );
        assert_eq!(
            calls[3].1,
            json!({"fs": "alpha:folder", "remote": "subdir"})
        );
    }

    #[test]
    fn root_refresh_never_sends_the_legacy_quote_remote() {
        let api = FakeRc::new([
            json!({"queue": []}),
            json!({}),
            json!({}),
            json!({"list": []}),
        ]);
        refresh_remote_snapshot(&api, "alpha:", "", false).unwrap();
        let calls = api.calls.borrow();
        assert_eq!(calls[3].1["remote"], "");
        assert_ne!(calls[3].1["remote"], "\"");
    }
}
