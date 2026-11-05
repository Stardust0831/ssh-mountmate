use super::*;
use mountmate_core::model::MountMapping;

fn discovery(request: &Request, origin: &str) -> Reply {
    let mut response: Value =
        serde_json::from_str(&v2_configuration(request, origin).body).unwrap();
    response["mount_roots"] = json!("ssh.mount-roots/v1");
    reply(200, response)
}

fn page() -> Value {
    json!({"schema_version":"ssh.mount-roots/v1", "site_id":"cluster-1", "items":[
        {"id":"shared-results", "display_name":"Shared results", "kind":"org_share", "path":"/data/计算 results ", "role":"read"},
        {"id":"project-work", "display_name":"Project work", "kind":"project_root", "path":"/data/project work\u{3000}", "role":"write"}
    ], "next_cursor":null})
}

fn optional_token() -> Reply {
    reply(
        200,
        json!({"access_token":"secret-access-token", "token_type":"Bearer", "expires_in":300,
        "scope": format!("{} {MOUNT_SCOPE}", SCOPES.join(" "))}),
    )
}

#[tokio::test]
async fn directories_use_optional_scope_and_continue_after_empty_pages() {
    let service = Fixture::new(|request, origin| match request.path.as_str() {
        "/v1/auth/config?client=ssh-mountmate" => Some(discovery(request, origin)),
        "/token" => Some(optional_token()),
        "/v1/account/ssh/keys" => Some(reply(200, enrollment(request, "applied"))),
        path if path.starts_with("/v1/account/ssh/mount-roots?") => Some(reply(
            200,
            if path.contains("cursor=") {
                page()
            } else {
                json!({"schema_version":"ssh.mount-roots/v1", "site_id":"cluster-1", "items":[], "next_cursor":"next-page"})
            },
        )),
        _ => None,
    });
    let session = service.session().await;
    let first = session
        .mount_roots(&session.connections()[0], None)
        .await
        .unwrap();
    assert!(first.items.is_empty());
    assert_eq!(first.next_cursor.as_deref(), Some("next-page"));
    let second = session
        .mount_roots(&session.connections()[0], first.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(second.items[0].path, "/data/计算 results ");
    assert_eq!(second.items[0].role, MountRole::Read);
    assert_eq!(second.items[1].role, MountRole::Write);
    assert!(second.next_cursor.is_none());
    let data = tempfile::tempdir().unwrap();
    let configured = session
        .enroll(&session.connections()[0], data.path())
        .await
        .unwrap()
        .with_mount_roots(second.items.clone());
    let mut draft = ConnectionDraft::default();
    configured.apply_to(&mut draft);
    draft.mountpoint = data
        .path()
        .join("local-mount")
        .to_string_lossy()
        .into_owned();
    let saved = draft.validate(&[]).unwrap().server;
    for (target, root) in saved.mount_targets().iter().zip(&second.items) {
        assert_eq!(target.remote_path, root.path);
        assert_eq!(
            target.remote_spec(),
            format!("{}:{}", target.remote_name(), root.path)
        );
    }
    let requests = service.requests.lock().unwrap();
    let fields: std::collections::HashMap<_, _> = url::form_urlencoded::parse(
        requests
            .iter()
            .find(|r| r.path == "/device")
            .unwrap()
            .body
            .as_bytes(),
    )
    .collect();
    assert_eq!(
        fields["scope"],
        format!("{} {MOUNT_SCOPE}", SCOPES.join(" "))
    );
    let pages: Vec<_> = requests
        .iter()
        .filter(|r| r.path.starts_with("/v1/account/ssh/mount-roots?"))
        .collect();
    assert_eq!(pages.len(), 2);
    assert!(pages.iter().all(|r| {
        r.headers
            .to_ascii_lowercase()
            .contains("authorization: bearer secret-access-token")
    }));
    assert!(
        pages[1].path.contains("site_id=cluster-1") && pages[1].path.contains("cursor=next-page")
    );
}

#[tokio::test]
async fn old_service_and_ungranted_directory_scope_keep_connection_setup() {
    for capability in [false, true] {
        let service = Fixture::new(move |request, origin| match request.path.as_str() {
            "/v1/auth/config?client=ssh-mountmate" if capability => {
                Some(discovery(request, origin))
            }
            "/v1/account/ssh/keys" => Some(reply(200, enrollment(request, "applied"))),
            _ => None,
        });
        let session = service.session().await;
        assert_eq!(session.has_mount_directory_discovery(), capability);
        assert_eq!(
            session
                .mount_roots(&session.connections()[0], None)
                .await
                .unwrap_err(),
            if capability {
                Error::Forbidden
            } else {
                Error::Unavailable
            }
        );
        let data = tempfile::tempdir().unwrap();
        session
            .enroll(&session.connections()[0], data.path())
            .await
            .unwrap();
        assert!(
            !service
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.path.contains("/mount-roots?"))
        );
    }
}

#[tokio::test]
async fn directory_pages_bind_site_paths_and_report_denial_or_unavailability() {
    let response = Arc::new(Mutex::new((200, page())));
    let current = response.clone();
    let service = Fixture::new(move |request, origin| match request.path.as_str() {
        "/v1/auth/config?client=ssh-mountmate" => Some(discovery(request, origin)),
        "/token" => Some(optional_token()),
        path if path.starts_with("/v1/account/ssh/mount-roots?") => {
            let (status, value) = current.lock().unwrap().clone();
            Some(reply(status, value))
        }
        _ => None,
    });
    let session = service.session().await;
    let mut large = page();
    large["items"] = Value::Array((0..20).map(|index| json!({
        "id":format!("directory-{index}"), "display_name":"界".repeat(2048), "kind":"project_root",
        "path":format!("/data/{}", vec!["nested".repeat(30); 10].join("/")), "role":"read"
    })).collect());
    assert!(large.to_string().len() > 128 * 1024);
    *response.lock().unwrap() = (200, large);
    assert_eq!(
        session
            .mount_roots(&session.connections()[0], None)
            .await
            .unwrap()
            .items
            .len(),
        20
    );
    for invalid in [
        {
            let mut value = page();
            value["site_id"] = json!("other-site");
            value
        },
        {
            let mut value = page();
            value["items"][0]["path"] = json!("/data/../secret");
            value
        },
        {
            let mut value = page();
            value["items"][0]["role"] = json!("owner");
            value
        },
        {
            let mut value = page();
            value["items"][1]["id"] = value["items"][0]["id"].clone();
            value
        },
        {
            let mut value = page();
            value["next_cursor"] = json!("already-seen");
            value
        },
    ] {
        *response.lock().unwrap() = (200, invalid);
        assert_eq!(
            session
                .mount_roots(&session.connections()[0], Some("already-seen"))
                .await
                .unwrap_err(),
            Error::Contract
        );
    }
    for (status, error) in [
        (401, Error::Expired),
        (403, Error::Forbidden),
        (503, Error::Unavailable),
    ] {
        *response.lock().unwrap() = (
            status,
            json!({"detail":{"code":"ssh_mount_roots_unavailable"}}),
        );
        assert_eq!(
            session
                .mount_roots(&session.connections()[0], None)
                .await
                .unwrap_err(),
            error
        );
    }
}

#[test]
fn selected_directories_preserve_existing_mappings_and_local_paths() {
    let roots: MountRootPage = serde_json::from_value(page()).unwrap();
    let configured = ConfiguredConnection {
        connection: Connection {
            site_id: "cluster-1".into(),
            name: "Test cluster".into(),
            host: "ssh.example.org".into(),
            port: 22,
            username: "example_user".into(),
        },
        key_file: PathBuf::from("synthetic-key"),
        mount_roots: roots.items,
    };
    let mut fresh = ConnectionDraft::default();
    configured.apply_to(&mut fresh);
    assert_eq!(fresh.remote_path, "/data/计算 results ");
    assert_eq!(fresh.mounts.len(), 1);
    assert_eq!(fresh.mounts[0].remote_path, "/data/project work\u{3000}");
    assert!(fresh.mounts[0].mountpoint.is_empty());
    let original = MountMapping {
        remote_path: "/data/project work\u{3000}".into(),
        mountpoint: "chosen-local-folder".into(),
        ..MountMapping::default()
    };
    let mut existing = ConnectionDraft::default();
    existing.editing_id = Some("saved-connection".into());
    existing.remote_path = "/existing".into();
    existing.mountpoint = "existing-local-folder".into();
    existing.mounts = vec![original.clone()];
    configured.apply_to(&mut existing);
    configured.apply_to(&mut existing);
    assert_eq!(existing.remote_path, "/existing");
    assert_eq!(existing.mountpoint, "existing-local-folder");
    assert_eq!(existing.mounts[0], original);
    assert_eq!(existing.mounts.len(), 2);
    assert_eq!(existing.mounts[1].remote_path, "/data/计算 results ");
}
