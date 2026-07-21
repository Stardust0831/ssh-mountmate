use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, de::DeserializeOwned};
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
    user: String,
    password: String,
}

impl HttpRcClient {
    pub fn new(rc_addr: &str, timeout: Duration) -> Result<Self, RcError> {
        Self::with_credentials(rc_addr, "", "", timeout)
    }

    pub fn with_credentials(
        rc_addr: &str,
        user: &str,
        password: &str,
        timeout: Duration,
    ) -> Result<Self, RcError> {
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
            user: user.into(),
            password: password.into(),
        })
    }

    pub fn transfer_snapshot(&self) -> Result<TransferSnapshot, RcError> {
        transfer_snapshot(self)
    }

    pub fn process_id(&self) -> Result<u32, RcError> {
        process_id(self)
    }

    pub fn quit(&self) -> Result<(), RcError> {
        quit(self)
    }

    pub fn refresh_remote(
        &self,
        remote: &str,
        relative_dir: &str,
        recursive: bool,
    ) -> Result<RefreshResult, RcError> {
        refresh_remote_snapshot(self, remote, relative_dir, recursive)
    }

    /// Invalidate and refresh only the VFS cache entry.  This deliberately
    /// avoids operations/list and vfs/queue so Explorer navigation can remain
    /// fire-and-forget and cannot expose transfer state.
    pub fn refresh_remote_cache(
        &self,
        relative_dir: &str,
    ) -> Result<(), RcError> {
        refresh_remote_cache(self, relative_dir)
    }
}

impl RcApi for HttpRcClient {
    fn call(&self, method: &str, params: Value) -> Result<Value, RcError> {
        let url = format!("{}/{method}", self.base_url);
        let mut request = self.client.post(url).json(&params);
        if !self.user.is_empty() || !self.password.is_empty() {
            request = request.basic_auth(&self.user, Some(&self.password));
        }
        let response = request
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

pub fn process_id(api: &impl RcApi) -> Result<u32, RcError> {
    let response = api.call("core/pid", json!({}))?;
    response
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| RcError::InvalidResponse {
            method: "core/pid".into(),
            message: "missing or invalid pid".into(),
        })
}

pub fn quit(api: &impl RcApi) -> Result<(), RcError> {
    api.call("core/quit", json!({}))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshResult {
    pub pending_uploads: usize,
    pub relative_dir: String,
    pub entries: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    list: Vec<Value>,
}

pub fn refresh_remote_snapshot(
    api: &impl RcApi,
    remote: &str,
    relative_dir: &str,
    recursive: bool,
) -> Result<RefreshResult, RcError> {
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
    let listing: ListResponse = decode(
        "operations/list",
        api.call(
            "operations/list",
            json!({"fs": remote, "remote": relative_dir}),
        )?,
    )?;
    let queue: QueueResponse = decode("vfs/queue", api.call("vfs/queue", json!({}))?)?;
    Ok(RefreshResult {
        pending_uploads: queue.queue.len(),
        relative_dir: relative_dir.into(),
        entries: listing.list,
    })
}

pub fn refresh_remote_cache(api: &impl RcApi, relative_dir: &str) -> Result<(), RcError> {
    let params = if relative_dir.is_empty() {
        json!({})
    } else {
        json!({"dir": relative_dir})
    };
    api.call("vfs/forget", params.clone())?;
    api.call("vfs/refresh", params)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

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
    fn authenticated_rc_requests_send_basic_auth() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                headers.push_str(&line);
            }
            assert!(headers.lines().any(|line| {
                line.to_ascii_lowercase()
                    .starts_with("authorization: basic ")
                    && line.ends_with("bW91bnRtYXRlOnNlY3JldA==")
            }));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10\r\nConnection: close\r\n\r\n{\"pid\":42}",
                )
                .unwrap();
        });

        let client = HttpRcClient::with_credentials(
            &address.to_string(),
            "mountmate",
            "secret",
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(client.process_id().unwrap(), 42);
        server.join().unwrap();
    }

    #[test]
    fn refresh_forgets_refreshes_then_verifies_remote_listing() {
        let api = FakeRc::new([
            json!({}),
            json!({}),
            json!({"list": [{"Name": "remote.bin"}]}),
            json!({"queue": [{"name": "pending.bin"}]}),
        ]);
        let result = refresh_remote_snapshot(&api, "alpha:folder", "subdir", false).unwrap();
        assert_eq!(result.pending_uploads, 1);
        assert_eq!(result.relative_dir, "subdir");
        assert_eq!(result.entries.len(), 1);
        let calls = api.calls.borrow();
        assert_eq!(
            calls
                .iter()
                .map(|(method, _)| method.as_str())
                .collect::<Vec<_>>(),
            ["vfs/forget", "vfs/refresh", "operations/list", "vfs/queue"]
        );
        assert_eq!(
            calls[2].1,
            json!({"fs": "alpha:folder", "remote": "subdir"})
        );
    }

    #[test]
    fn cache_only_refresh_calls_exactly_forget_and_refresh() {
        let api = FakeRc::new([json!({}), json!({}), json!({})]);
        refresh_remote_cache(&api, "subdir").unwrap();
        let calls = api.calls.borrow();
        assert_eq!(
            calls.iter().map(|(method, _)| method.as_str()).collect::<Vec<_>>(),
            ["vfs/forget", "vfs/refresh"]
        );
        assert_eq!(calls[0].1, json!({"dir": "subdir"}));
        assert_eq!(calls[1].1, json!({"dir": "subdir"}));
    }

    #[test]
    fn root_refresh_never_sends_the_legacy_quote_remote() {
        let api = FakeRc::new([
            json!({}),
            json!({}),
            json!({"list": []}),
            json!({"queue": []}),
        ]);
        let result = refresh_remote_snapshot(&api, "alpha:", "", false).unwrap();
        assert_eq!(result.relative_dir, "");
        let calls = api.calls.borrow();
        assert_eq!(calls[2].1["remote"], "");
        assert_ne!(calls[2].1["remote"], "\"");
    }

    #[test]
    fn refresh_does_not_report_zero_entries_when_listing_is_missing() {
        let api = FakeRc::new([json!({}), json!({}), json!({}), json!({"queue": []})]);
        let error = refresh_remote_snapshot(&api, "alpha:", "subdir", false).unwrap_err();
        assert!(matches!(
            error,
            RcError::InvalidResponse { method, .. } if method == "operations/list"
        ));
    }

    #[test]
    fn process_identity_and_quit_use_rc_contract() {
        let api = FakeRc::new([json!({"pid": 42}), json!({})]);
        assert_eq!(process_id(&api).unwrap(), 42);
        quit(&api).unwrap();
        assert_eq!(
            api.calls
                .borrow()
                .iter()
                .map(|(method, _)| method.as_str())
                .collect::<Vec<_>>(),
            ["core/pid", "core/quit"]
        );
    }
}
