use super::*;
use crate::enrollment::EnrollmentState;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[path = "native_tests.rs"]
mod native;

#[derive(Clone, Debug)]
struct Request {
    method: String,
    path: String,
    headers: String,
    body: String,
}
struct Reply {
    status: u16,
    body: String,
    headers: String,
}
fn reply(status: u16, body: Value) -> Reply {
    Reply {
        status,
        body: body.to_string(),
        headers: String::new(),
    }
}
struct Fixture {
    base: Url,
    requests: Arc<Mutex<Vec<Request>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(mut handler: impl FnMut(&Request, &str) -> Option<Reply> + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let origin = base.as_str().trim_end_matches('/').to_owned();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let seen = requests.clone();
        let done = stopped.clone();
        let worker = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if done.load(Ordering::Relaxed) {
                    break;
                }
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() || first.is_empty() {
                    continue;
                }
                let parts: Vec<_> = first.split_whitespace().collect();
                let mut request = Request {
                    method: parts[0].into(),
                    path: parts[1].into(),
                    headers: String::new(),
                    body: String::new(),
                };
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                    request.headers.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                request.body = String::from_utf8(body).unwrap();
                seen.lock().unwrap().push(request.clone());
                let response =
                    handler(&request, &origin).unwrap_or_else(|| standard_reply(&request, &origin));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    response.status,
                    response.body.len(),
                    response.headers,
                    response.body
                );
            }
        });
        Self {
            base,
            requests,
            stopped,
            worker: Some(worker),
        }
    }
    async fn client(&self) -> Result<Client, Error> {
        Client::discover_at(self.base.clone(), self.base.join("realm").unwrap()).await
    }
    async fn session(&self) -> Session {
        let client = self.client().await.unwrap();
        let login = client.start_login().await.unwrap();
        client.finish_login(&login).await.unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.base.socket_addrs(|| None).unwrap()[0]);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn standard_reply(request: &Request, origin: &str) -> Reply {
    match request.path.as_str() {
        "/v1/auth/config?client=ssh-mountmate" => reply(
            200,
            json!({"schema_version":"ssh-mountmate.connection/v1", "identity_issuer":format!("{origin}/realm"), "oidc_client_id":CLIENT_ID, "oidc_resource":RESOURCE, "ssh_access":"public-key/v1"}),
        ),
        "/realm/.well-known/openid-configuration" => reply(
            200,
            json!({"issuer":format!("{origin}/realm"), "token_endpoint":format!("{origin}/token"), "device_authorization_endpoint":format!("{origin}/device")}),
        ),
        "/device" => reply(
            200,
            json!({"device_code":"secret-device-code", "user_code":"ABCD-EFGH", "verification_uri":format!("{origin}/verify"), "expires_in":600, "interval":1}),
        ),
        "/token" => reply(
            200,
            json!({"access_token":"secret-access-token", "token_type":"Bearer", "expires_in":300, "refresh_token":"never-persist-this", "scope":SCOPES.join(" ")}),
        ),
        "/v1/account/ssh/connections" => reply(
            200,
            json!({"schema_version":"ssh.connections/v1", "account_id":"opaque-account", "connections":[{"site_id":"cluster-1", "name":"Test cluster", "host":"ssh.example.org", "port":12022, "username":"unix_user"}]}),
        ),
        _ => reply(404, json!({})),
    }
}
fn enrollment(request: &Request, state: &str) -> Value {
    let body: Value = serde_json::from_str(&request.body).unwrap();
    let public = ssh_key::PublicKey::from_openssh(body["public_key"].as_str().unwrap()).unwrap();
    json!({"request_id":body["request_id"], "site_id":body["site_id"], "public_key_fingerprint":public.fingerprint(ssh_key::HashAlg::Sha256).to_string(), "state":state})
}

fn v2_configuration(request: &Request, origin: &str) -> Reply {
    let mut response: Value = serde_json::from_str(&standard_reply(request, origin).body).unwrap();
    response["ssh_access"] = json!("public-key/v2");
    reply(200, response)
}

fn verification(request: &Request, origin: &str) -> Value {
    let mut response = enrollment(request, "pending");
    response["verification_url"] = json!(format!(
        "{origin}/portal/#/portal/ssh-device?request_id={}",
        response["request_id"].as_str().unwrap()
    ));
    response["reason"] = json!("verification_required");
    response
}

#[tokio::test]
async fn browser_confirmation_is_reported_once_then_site_application_continues() {
    let mut registered = None;
    let mut polls = 0;
    let service = Fixture::new(move |request, origin| match request.path.as_str() {
        "/v1/auth/config?client=ssh-mountmate" => Some(v2_configuration(request, origin)),
        "/v1/account/ssh/keys" => {
            let response = verification(request, origin);
            registered = Some(response.clone());
            Some(reply(202, response))
        }
        path if path.starts_with("/v1/account/ssh/keys/") => {
            polls += 1;
            let mut response = registered.clone().unwrap();
            if polls >= 2 {
                response.as_object_mut().unwrap().remove("verification_url");
                response.as_object_mut().unwrap().remove("reason");
            }
            if polls >= 3 {
                response["state"] = json!("applied");
            }
            Some(reply(200, response))
        }
        _ => None,
    });
    let session = service.session().await;
    let root = tempfile::tempdir().unwrap();
    let mut progress = Vec::new();
    let configured = session
        .enroll_with_progress(&session.connections()[0], root.path(), |event| {
            progress.push(event)
        })
        .await
        .unwrap();
    assert!(configured.key_file.is_file());
    assert_eq!(progress.len(), 2);
    assert!(matches!(
        progress[0],
        EnrollmentProgress::VerificationRequired(_)
    ));
    assert_eq!(progress[1], EnrollmentProgress::Applying);
    let requests = service.requests.lock().unwrap();
    let registration = requests
        .iter()
        .find(|r| r.path == "/v1/account/ssh/keys")
        .unwrap();
    let request: Value = serde_json::from_str(&registration.body).unwrap();
    assert_eq!(
        progress[0],
        EnrollmentProgress::VerificationRequired(format!(
            "{}portal/#/portal/ssh-device?request_id={}",
            service.base,
            request["request_id"].as_str().unwrap()
        ))
    );
    // Approval uses a browser, not a bearer-authenticated request from the adapter.
    assert!(!requests.iter().any(|r| r.path.starts_with("/portal/")));
}

#[test]
fn verification_destinations_and_failure_reasons_are_bound_to_the_operation() {
    use crate::enrollment::{EnrollmentReason, Status};
    let portal = Url::parse("https://www.open-sai.com").unwrap();
    let request_id = "a".repeat(64);
    let approved = format!("{portal}portal/#/portal/ssh-device?request_id={request_id}");
    let response = |url: Option<String>| Enrollment {
        request_id: request_id.clone(),
        site_id: "site".into(),
        public_key_fingerprint: "SHA256:synthetic".into(),
        state: EnrollmentState::Pending,
        verification_url: url,
        reason: Some(EnrollmentReason::VerificationRequired),
    };
    for url in [
        approved.replace("www.open-sai.com", "untrusted.example"),
        approved.replace("https:", "http:"),
        approved.replace("www.open-sai.com", "www.open-sai.com:444"),
        approved.replace("www.open-sai.com", "user@www.open-sai.com"),
        approved.replace("/portal/#", "/other/#"),
        approved.replace("/portal/#", "/portal/?token=forbidden#"),
        approved.replace("ssh-device", "other-action"),
        approved.replace(&request_id, &"b".repeat(64)),
        format!("{approved}&request_id={request_id}"),
        format!("{approved}&next=https://untrusted.example"),
        format!("{approved}#extra"),
    ] {
        assert!(matches!(
            response(Some(url)).validate(&request_id, "site", "SHA256:synthetic", &portal, true),
            Err(Error::Contract)
        ));
    }
    assert!(matches!(
        response(None).validate(&request_id, "site", "SHA256:synthetic", &portal, true),
        Err(Error::Contract)
    ));
    assert!(matches!(
        response(Some(approved.clone())).validate(
            &request_id,
            "site",
            "SHA256:synthetic",
            &portal,
            false
        ),
        Err(Error::Contract)
    ));
    assert!(matches!(
        response(Some(approved)).validate(&request_id, "site", "SHA256:synthetic", &portal, true),
        Ok(Status::Pending(Some(_)))
    ));
    for (reason, error) in [
        (EnrollmentReason::ApprovalExpired, Error::ApprovalExpired),
        (EnrollmentReason::KeyRevoked, Error::KeyRevoked),
        (EnrollmentReason::KeyExpired, Error::KeyExpired),
        (EnrollmentReason::SyncFailed, Error::SyncFailed),
    ] {
        let mut failed = response(None);
        failed.state = EnrollmentState::Failed;
        failed.reason = Some(reason);
        assert!(
            matches!(failed.validate(&request_id, "site", "SHA256:synthetic", &portal, true), Err(actual) if actual == error)
        );
    }
    let unknown = reply(
        200,
        json!({"request_id":request_id, "site_id":"site", "public_key_fingerprint":"SHA256:synthetic", "state":"pending", "reason":"unknown"}),
    );
    assert!(serde_json::from_str::<Enrollment>(&unknown.body).is_err());
}

#[tokio::test]
async fn cancelled_browser_approval_keeps_the_key_and_original_retry_identity() {
    let service = Fixture::new(|request, origin| match request.path.as_str() {
        "/v1/auth/config?client=ssh-mountmate" => Some(v2_configuration(request, origin)),
        "/v1/account/ssh/keys" => Some(reply(202, verification(request, origin))),
        _ => None,
    });
    let session = service.session().await;
    let root = tempfile::tempdir().unwrap();
    let mut confirmations = 0;
    for _ in 0..2 {
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                session.enroll_with_progress(&session.connections()[0], root.path(), |progress| {
                    if matches!(progress, EnrollmentProgress::VerificationRequired(_)) {
                        confirmations += 1;
                    }
                })
            )
            .await
            .is_err()
        );
    }
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert_eq!(confirmations, 2);
    let requests = service.requests.lock().unwrap();
    let registrations: Vec<_> = requests
        .iter()
        .filter(|r| r.path == "/v1/account/ssh/keys")
        .collect();
    assert_eq!(registrations.len(), 2);
    assert_eq!(registrations[0].body, registrations[1].body);
    assert!(
        !requests
            .iter()
            .any(|r| r.path.starts_with("/v1/account/ssh/keys/"))
    );
}

#[tokio::test]
async fn browser_confirmation_timeout_is_distinct_from_site_sync_timeout() {
    let service = Fixture::new(|request, origin| {
        (request.path == "/v1/auth/config?client=ssh-mountmate")
            .then(|| v2_configuration(request, origin))
    });
    let session = service.session().await;
    let root = tempfile::tempdir().unwrap();
    let key = key::load_or_create(root.path(), b"pending-confirmation").unwrap();
    let response: Enrollment = serde_json::from_value(json!({
        "request_id":key.request_id, "site_id":session.connections()[0].site_id,
        "public_key_fingerprint":key.fingerprint, "state":"pending",
        "reason":"verification_required",
        "verification_url":format!("{}portal/#/portal/ssh-device?request_id={}", service.base, key.request_id),
    })).unwrap();
    assert_eq!(
        session
            .wait_for_key(
                response,
                &key,
                &session.connections()[0],
                Duration::from_millis(10),
                &mut |_| {}
            )
            .await,
        Err(Error::VerificationPending)
    );
    assert!(key.path.is_file());
}

#[tokio::test]
async fn retired_key_requires_explicit_reauthorization_and_restart_resumes_the_new_intent() {
    use sha2::{Digest, Sha256};
    for (reason, error) in [
        ("key_revoked", Error::KeyRevoked),
        ("key_expired", Error::KeyExpired),
    ] {
        let retired = Arc::new(AtomicBool::new(false));
        let approved = Arc::new(AtomicBool::new(false));
        let retirement = retired.clone();
        let approval = approved.clone();
        let mut original: Option<Request> = None;
        let mut replacement: Option<Request> = None;
        let service = Fixture::new(move |request, origin| match request.path.as_str() {
            "/v1/auth/config?client=ssh-mountmate" => Some(v2_configuration(request, origin)),
            "/v1/account/ssh/keys" => {
                let original = original.get_or_insert_with(|| request.clone());
                if request.body == original.body {
                    let mut response = enrollment(
                        request,
                        if retirement.load(Ordering::SeqCst) {
                            "failed"
                        } else {
                            "applied"
                        },
                    );
                    if retirement.load(Ordering::SeqCst) {
                        response["reason"] = json!(reason);
                    }
                    return Some(reply(200, response));
                }
                if replacement.is_none() {
                    replacement = Some(request.clone());
                    return Some(reply(503, json!({})));
                }
                assert_eq!(replacement.as_ref().unwrap().body, request.body);
                Some(reply(
                    200,
                    if approval.load(Ordering::SeqCst) {
                        enrollment(request, "applied")
                    } else {
                        verification(request, origin)
                    },
                ))
            }
            path if path.starts_with("/v1/account/ssh/keys/") => {
                let request = replacement.as_ref().unwrap();
                Some(reply(
                    200,
                    if approval.load(Ordering::SeqCst) {
                        enrollment(request, "applied")
                    } else {
                        verification(request, origin)
                    },
                ))
            }
            _ => None,
        });
        let root = tempfile::tempdir().unwrap();
        let session = service.session().await;
        let old = session
            .enroll(&session.connections()[0], root.path())
            .await
            .unwrap();
        let previous_bytes = Sha256::digest(std::fs::read(&old.key_file).unwrap());
        let mut saved_draft = ConnectionDraft::default();
        old.apply_to(&mut saved_draft);
        let saved = saved_draft.validate(&[]).unwrap().server;
        retired.store(true, Ordering::SeqCst);
        assert_eq!(
            session
                .enroll(&session.connections()[0], root.path())
                .await
                .unwrap_err(),
            error
        );
        // Only the explicit action can select a replacement. Ambiguous transport
        // failure after selection must not manufacture another key on restart.
        assert_eq!(
            session
                .reauthorize_with_progress(&session.connections()[0], root.path(), |_| {})
                .await
                .unwrap_err(),
            Error::Network
        );
        assert_eq!(
            Sha256::digest(std::fs::read(&old.key_file).unwrap()),
            previous_bytes
        );
        drop(session);
        let session = service.session().await;
        let next = session
            .enroll_with_progress(&session.connections()[0], root.path(), |event| {
                if matches!(event, EnrollmentProgress::VerificationRequired(_)) {
                    approved.store(true, Ordering::SeqCst);
                }
            })
            .await
            .unwrap();
        assert_ne!(next.key_file, old.key_file);
        assert_eq!(saved.key_file, old.key_file.to_string_lossy());
        assert_eq!(
            Sha256::digest(std::fs::read(&old.key_file).unwrap()),
            previous_bytes
        );
        // A stale UI recovery action cannot rotate an already current valid key.
        let resumed = session
            .reauthorize_with_progress(&session.connections()[0], root.path(), |_| {})
            .await
            .unwrap();
        assert_eq!(resumed.key_file, next.key_file);
        let requests = service.requests.lock().unwrap();
        let registrations: Vec<Value> = requests
            .iter()
            .filter(|r| r.path == "/v1/account/ssh/keys")
            .map(|r| serde_json::from_str(&r.body).unwrap())
            .collect();
        assert_eq!(registrations.len(), 6);
        assert_eq!(registrations[0], registrations[1]);
        assert_eq!(registrations[1], registrations[2]);
        assert_ne!(
            registrations[2]["request_id"],
            registrations[3]["request_id"]
        );
        assert_ne!(
            registrations[2]["public_key"],
            registrations[3]["public_key"]
        );
        assert_eq!(registrations[3], registrations[4]);
        assert_eq!(registrations[4], registrations[5]);
        assert_eq!(
            std::fs::read_to_string(old.key_file.parent().unwrap().join("current-key")).unwrap(),
            registrations[3]["request_id"].as_str().unwrap()
        );
    }
}

#[test]
fn replacement_selector_cannot_escape_or_silently_replace_a_missing_current_key() {
    let root = tempfile::tempdir().unwrap();
    let previous = key::load_or_create(root.path(), b"context").unwrap();
    let next = key::renew(root.path(), b"context", &previous.request_id).unwrap();
    let again = key::renew(root.path(), b"context", &previous.request_id).unwrap();
    assert_eq!(again.path, next.path);
    assert_eq!(again.request_id, next.request_id);
    let selector = previous.path.parent().unwrap().join("current-key");
    std::fs::write(&selector, "../outside").unwrap();
    assert!(matches!(
        key::load_or_create(root.path(), b"context"),
        Err(Error::KeyStorage)
    ));
    std::fs::write(&selector, &next.request_id).unwrap();
    std::fs::remove_file(&next.path).unwrap();
    assert!(matches!(
        key::load_or_create(root.path(), b"context"),
        Err(Error::KeyStorage)
    ));
    assert!(previous.path.is_file());
    assert!(!next.path.exists());
    #[cfg(unix)]
    {
        std::fs::remove_file(&selector).unwrap();
        std::os::unix::fs::symlink(&previous.path, &selector).unwrap();
        assert!(matches!(
            key::load_or_create(root.path(), b"context"),
            Err(Error::KeyStorage)
        ));
    }
}
#[tokio::test]
async fn setup_waits_for_application_and_reuses_key_across_sessions() {
    let mut registered = None;
    let service = Fixture::new(move |request, _| {
        if request.path == "/v1/account/ssh/keys" {
            let result = enrollment(request, "pending");
            registered = Some(result.clone());
            Some(reply(202, result))
        } else if request.path.starts_with("/v1/account/ssh/keys/") {
            let mut result = registered.clone().unwrap();
            result["state"] = json!("applied");
            Some(reply(200, result))
        } else {
            None
        }
    });
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data with spaces");
    let session = service.session().await;
    let configured = session
        .enroll(&session.connections()[0], &data)
        .await
        .unwrap();
    let mut draft = ConnectionDraft::default();
    draft.mountpoint = root.path().join("mount").to_string_lossy().into();
    draft.remote_path = "/work".into();
    configured.apply_to(&mut draft);
    let server = draft.validate(&[]).unwrap().server;
    assert_eq!(server.host, "ssh.example.org");
    assert_eq!(server.user, "unix_user");
    assert_eq!(server.remote_path, "/work");
    assert!(!server.ssh_config_managed);
    assert_eq!(
        ConnectionDraft::from_server(&server).key_file,
        draft.key_file
    );
    drop(session);
    let session = service.session().await;
    let retried = session
        .enroll(&session.connections()[0], &data)
        .await
        .unwrap();
    assert_eq!(configured.key_file, retried.key_file);
    let seen = service.requests.lock().unwrap();
    let registrations: Vec<_> = seen
        .iter()
        .filter(|r| r.path == "/v1/account/ssh/keys")
        .collect();
    assert_eq!(registrations.len(), 2);
    assert_eq!(registrations[0].body, registrations[1].body);
    for request in seen.iter() {
        assert!(!request.body.contains("PRIVATE KEY"));
        let bearer = request
            .headers
            .to_ascii_lowercase()
            .contains("authorization: bearer secret-access-token");
        assert_eq!(bearer, request.path.starts_with("/v1/account/ssh/"));
        if matches!(request.path.as_str(), "/device" | "/token") {
            assert_eq!(request.method, "POST");
            let fields: std::collections::HashMap<_, _> =
                url::form_urlencoded::parse(request.body.as_bytes()).collect();
            assert_eq!(fields.get("client_id").unwrap(), CLIENT_ID);
            assert_eq!(fields.get("resource").unwrap(), RESOURCE);
            assert!(!fields.contains_key("client_secret"));
            if request.path == "/device" {
                assert_eq!(fields.get("scope").unwrap(), &SCOPES.join(" "));
            }
        }
    }
    let serialized = serde_json::to_string(&server).unwrap();
    assert!(!serialized.contains("secret-access-token"));
    assert!(!serialized.contains("never-persist-this"));
    assert!(!format!("{session:?}").contains("secret-access-token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&configured.key_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(configured.key_file.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
#[tokio::test]
async fn unsupported_service_stops_before_login() {
    let service = Fixture::new(|_, _| Some(reply(400, json!({"code":"client_not_supported"}))));
    assert!(matches!(service.client().await, Err(Error::Unavailable)));
    assert_eq!(service.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn retry_after_ambiguous_registration_uses_the_original_key_and_request() {
    let mut posts = 0;
    let service = Fixture::new(move |request, _| {
        if request.path != "/v1/account/ssh/keys" {
            return None;
        }
        posts += 1;
        Some(if posts == 1 {
            reply(503, json!({}))
        } else {
            reply(200, enrollment(request, "applied"))
        })
    });
    let root = tempfile::tempdir().unwrap();
    let session = service.session().await;
    assert_eq!(
        session
            .enroll(&session.connections()[0], root.path())
            .await
            .unwrap_err(),
        Error::Network
    );
    drop(session);
    let session = service.session().await;
    session
        .enroll(&session.connections()[0], root.path())
        .await
        .unwrap();
    let seen = service.requests.lock().unwrap();
    let posts: Vec<_> = seen
        .iter()
        .filter(|r| r.path == "/v1/account/ssh/keys")
        .collect();
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0].body, posts[1].body);
}
#[tokio::test]
async fn issuer_and_endpoint_mismatches_stop_before_credentials() {
    for field in ["issuer", "token_endpoint", "device_authorization_endpoint"] {
        let service = Fixture::new(move |request, origin| {
            if !request.path.ends_with("openid-configuration") {
                return None;
            }
            let mut body: Value =
                serde_json::from_str(&standard_reply(request, origin).body).unwrap();
            body[field] = json!("https://untrusted.example/endpoint");
            Some(reply(200, body))
        });
        assert!(matches!(service.client().await, Err(Error::Contract)));
        assert_eq!(service.requests.lock().unwrap().len(), 2);
    }
}
#[tokio::test]
async fn device_polling_handles_pending_slow_down_and_denial() {
    let mut polls = 0;
    let service = Fixture::new(move |request, _| {
        if request.path != "/token" {
            return None;
        }
        polls += 1;
        Some(reply(
            400,
            json!({"error":match polls { 1 => "authorization_pending", 2 => "slow_down", _ => "access_denied" }, "error_description":"never-show-this-secret"}),
        ))
    });
    let client = service.client().await.unwrap();
    let login = client.start_login().await.unwrap();
    assert!(!format!("{login:?}").contains("secret-device-code"));
    let started = Instant::now();
    let error = client.finish_login(&login).await.unwrap_err();
    assert_eq!(error, Error::Denied);
    assert!(started.elapsed() >= Duration::from_secs(8));
    assert!(!error.to_string().contains("never-show-this-secret"));
}
#[tokio::test]
async fn expired_and_cancelled_logins_stop_polling() {
    let service = Fixture::new(|_, _| None);
    let client = service.client().await.unwrap();
    let mut login = client.start_login().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), client.finish_login(&login))
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        !service
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path == "/token")
    );
    login.deadline = Instant::now();
    assert_eq!(
        client.finish_login(&login).await.unwrap_err(),
        Error::Expired
    );
}
#[tokio::test]
async fn redirects_and_oversized_responses_are_not_accepted() {
    let service = Fixture::new(|request, origin| {
        (request.path == "/v1/auth/config?client=ssh-mountmate").then(|| Reply {
            status: 307,
            body: String::new(),
            headers: format!("Location: {origin}/must-not-follow\r\n"),
        })
    });
    assert!(service.client().await.is_err());
    assert_eq!(service.requests.lock().unwrap().len(), 1);
    let oversized = Fixture::new(|_, _| {
        Some(Reply {
            status: 200,
            body: "x".repeat(128 * 1024 + 1),
            headers: String::new(),
        })
    });
    assert!(matches!(oversized.client().await, Err(Error::Contract)));
}
#[tokio::test]
async fn failed_or_mismatched_enrollment_never_produces_a_connection() {
    for mismatch in [false, true] {
        let service = Fixture::new(move |request, _| {
            if request.path != "/v1/account/ssh/keys" {
                return None;
            }
            let mut result = enrollment(request, if mismatch { "applied" } else { "failed" });
            if mismatch {
                result["site_id"] = json!("other-site");
            }
            Some(reply(200, result))
        });
        let session = service.session().await;
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            session
                .enroll(&session.connections()[0], root.path())
                .await
                .unwrap_err(),
            if mismatch {
                Error::Contract
            } else {
                Error::SyncFailed
            }
        );
    }
}
#[tokio::test]
async fn timeout_retains_key_and_pending_is_not_success() {
    let service = Fixture::new(|_, _| None);
    let session = service.session().await;
    let root = tempfile::tempdir().unwrap();
    let key = key::load_or_create(root.path(), b"context").unwrap();
    let response = Enrollment {
        request_id: key.request_id.clone(),
        site_id: session.connections()[0].site_id.clone(),
        public_key_fingerprint: key.fingerprint.clone(),
        state: EnrollmentState::Pending,
        verification_url: None,
        reason: None,
    };
    assert_eq!(
        session
            .wait_for_key(
                response,
                &key,
                &session.connections()[0],
                Duration::from_millis(10),
                &mut |_| {},
            )
            .await,
        Err(Error::SyncPending)
    );
    assert!(key.path.is_file());
}
#[test]
fn key_isolation_and_storage_failure_do_not_replace_existing_keys() {
    let root = tempfile::tempdir().unwrap();
    let first = key::load_or_create(root.path(), b"account-a/site-a").unwrap();
    let other = key::load_or_create(root.path(), b"account-b/site-a").unwrap();
    assert_ne!(first.public_key, other.public_key);
    std::fs::write(&first.path, "broken key").unwrap();
    assert!(matches!(
        key::load_or_create(root.path(), b"account-a/site-a"),
        Err(Error::KeyStorage)
    ));
    assert_eq!(std::fs::read_to_string(&first.path).unwrap(), "broken key");
    #[cfg(unix)]
    {
        std::fs::remove_file(&first.path).unwrap();
        std::os::unix::fs::symlink(&other.path, &first.path).unwrap();
        assert!(matches!(
            key::load_or_create(root.path(), b"account-a/site-a"),
            Err(Error::KeyStorage)
        ));
    }
}
#[test]
fn directory_rejects_unsafe_targets_and_duplicate_ids() {
    let response = standard_reply(
        &Request {
            method: "GET".into(),
            path: "/v1/account/ssh/connections".into(),
            headers: String::new(),
            body: String::new(),
        },
        "unused",
    );
    for (field, value) in [
        ("host", "-oProxyCommand=bad"),
        ("username", "user\ncommand"),
        ("site_id", "../../escape"),
    ] {
        let mut payload: Value = serde_json::from_str(&response.body).unwrap();
        payload["connections"][0][field] = json!(value);
        assert_eq!(
            serde_json::from_value::<Directory>(payload)
                .unwrap()
                .validate(),
            Err(Error::Contract)
        );
    }
    let mut directory: Directory = serde_json::from_str(&response.body).unwrap();
    directory.connections.push(directory.connections[0].clone());
    assert_eq!(directory.validate(), Err(Error::Contract));
}
