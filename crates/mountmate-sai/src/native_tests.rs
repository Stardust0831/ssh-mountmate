//! Optional local integration: the real key and ordinary connection reach rclone SFTP.
//! Identity, Portal approval and site application are synthetic fixtures, not live SAI.
use super::*;
use mountmate_core::Settings;
use mountmate_core::paths::AppPaths;
use mountmate_core::rclone::{RcloneRemote, write_rclone_remote};
use mountmate_core::service::{MountService, ServiceError};
use std::fs;
use std::process::{Child, Command, Stdio};

struct SftpServer(Child);
impl Drop for SftpServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_sftp(binary: &Path, root: &Path, key: &Path, port: u16) -> SftpServer {
    let log_path = root.join("sftp.log");
    let log = fs::File::create(&log_path).unwrap();
    let mut server = SftpServer(
        Command::new(binary)
            .arg("--cache-dir")
            .arg(root.join("server-cache"))
            .args(["serve", "sftp"])
            .arg(root.join("remote"))
            .args(["--addr", &format!("127.0.0.1:{port}"), "--user", "fixture"])
            .arg("--authorized-keys")
            .arg(key)
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "SFTP fixture exited"
        );
        if fs::read_to_string(&log_path)
            .unwrap()
            .contains("SFTP server listening")
        {
            return server;
        }
        assert!(
            Instant::now() < deadline,
            "SFTP fixture did not become ready"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test]
#[ignore = "requires SSH_MOUNTMATE_TEST_RCLONE and ssh/ssh-keyscan; no production service or FUSE"]
async fn generated_key_reaches_native_sftp_after_browser_approval() {
    let binary = PathBuf::from(std::env::var_os("SSH_MOUNTMATE_TEST_RCLONE").unwrap());
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("remote")).unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let approved = Arc::new(AtomicBool::new(false));
    let approval = approved.clone();
    let fixture_root = root.path().to_owned();
    let sftp_binary = binary.clone();
    let mut registered = None;
    let mut server = None;
    let service = Fixture::new(move |request, origin| match request.path.as_str() {
        "/v1/auth/config?client=ssh-mountmate" => Some(v2_configuration(request, origin)),
        "/v1/account/ssh/connections" => Some(reply(
            200,
            json!({
                "schema_version":"ssh.connections/v1", "account_id":"native-fixture-account",
                "connections":[{"site_id":"fixture-site", "name":"Local SFTP fixture", "host":"127.0.0.1", "port":port, "username":"fixture"}]
            }),
        )),
        "/v1/account/ssh/keys" => {
            registered = Some(request.clone());
            Some(reply(202, verification(request, origin)))
        }
        path if path.starts_with("/v1/account/ssh/keys/") => {
            let registration = registered.as_ref().unwrap();
            if !approval.load(Ordering::SeqCst) {
                return Some(reply(200, verification(registration, origin)));
            }
            if server.is_none() {
                let request: Value = serde_json::from_str(&registration.body).unwrap();
                let public_key = fixture_root.join("authorized_keys");
                fs::write(&public_key, request["public_key"].as_str().unwrap()).unwrap();
                server = Some(start_sftp(&sftp_binary, &fixture_root, &public_key, port));
            }
            Some(reply(200, enrollment(registration, "applied")))
        }
        _ => None,
    });
    let session = service.session().await;
    let configured = session
        .enroll_with_progress(
            &session.connections()[0],
            &root.path().join("device"),
            |event| {
                if matches!(event, EnrollmentProgress::VerificationRequired(_)) {
                    approved.store(true, Ordering::SeqCst);
                }
            },
        )
        .await
        .unwrap();
    let mut draft = ConnectionDraft::default();
    draft.mountpoint = root.path().join("mount").to_string_lossy().into();
    configured.apply_to(&mut draft);
    let mut connection = draft.validate(&[]).unwrap().server;
    assert_eq!(connection.connection_method, ConnectionMethod::Native);

    let paths = AppPaths {
        config_dir: root.path().join("config"),
        cache_dir: root.path().join("cache"),
        state_dir: root.path().join("state"),
        data_dir: root.path().join("data"),
    };
    fs::create_dir_all(paths.managed_bin_dir()).unwrap();
    let managed_binary = paths.managed_bin_dir().join(if cfg!(windows) {
        "rclone.exe"
    } else {
        "rclone"
    });
    if fs::hard_link(&binary, &managed_binary).is_err() {
        fs::copy(&binary, &managed_binary).unwrap();
    }
    let mounts = MountService::new(paths.clone(), root.path().join("app"));
    // The real mount path must stop for explicit host trust before using the key.
    let error = mounts.mount(&connection, &Settings::default()).unwrap_err();
    let ServiceError::HostKeyConfirmation(review) = error else {
        panic!("expected host trust prompt, got {error}");
    };
    assert!(!paths.known_hosts().exists());
    mounts.confirm_host_key(&review).unwrap();
    assert!(mounts.host_key_is_confirmed(&review));
    let remote =
        RcloneRemote::for_server(&connection, None, Some(&paths.known_hosts()), cfg!(windows))
            .unwrap();
    write_rclone_remote(&paths, &remote).unwrap();
    let config = fs::read_to_string(paths.rclone_config()).unwrap();
    assert!(!config.contains("secret-access-token"));
    assert!(!config.contains("PRIVATE KEY"));

    let payload = b"Native SFTP roundtrip\0\x01\xff";
    let local = root.path().join("upload.bin");
    fs::write(&local, payload).unwrap();
    let remote_file = format!("{}:roundtrip.bin", connection.remote_name());
    let uploaded = Command::new(&binary)
        .arg("--config")
        .arg(paths.rclone_config())
        .arg("copyto")
        .arg(&local)
        .arg(&remote_file)
        .args(["--retries", "1", "--low-level-retries", "1"])
        .output()
        .unwrap();
    assert!(
        uploaded.status.success(),
        "{}",
        String::from_utf8_lossy(&uploaded.stderr)
    );
    let downloaded = Command::new(&binary)
        .arg("--config")
        .arg(paths.rclone_config())
        .arg("cat")
        .arg(&remote_file)
        .output()
        .unwrap();
    assert!(
        downloaded.status.success(),
        "{}",
        String::from_utf8_lossy(&downloaded.stderr)
    );
    assert_eq!(downloaded.stdout, payload);
    assert_eq!(
        fs::read(root.path().join("remote/roundtrip.bin")).unwrap(),
        payload
    );

    // A different local key cannot use the same connection's site enrollment.
    let other = key::load_or_create(&root.path().join("other-device"), b"other-account").unwrap();
    let trusted = fs::read(paths.known_hosts()).unwrap();
    fs::write(
        paths.known_hosts(),
        format!("[127.0.0.1]:{port} {}\n", other.public_key),
    )
    .unwrap();
    write_rclone_remote(
        &paths,
        &RcloneRemote::for_server(&connection, None, Some(&paths.known_hosts()), cfg!(windows))
            .unwrap(),
    )
    .unwrap();
    let wrong_host = Command::new(&binary)
        .arg("--config")
        .arg(paths.rclone_config())
        .arg("cat")
        .arg(&remote_file)
        .args(["--retries", "1", "--low-level-retries", "1"])
        .output()
        .unwrap();
    assert!(!wrong_host.status.success());
    assert!(String::from_utf8_lossy(&wrong_host.stderr).contains("knownhosts: key mismatch"));
    fs::write(paths.known_hosts(), trusted).unwrap();
    connection.key_file = other.path.to_string_lossy().into_owned();
    write_rclone_remote(
        &paths,
        &RcloneRemote::for_server(&connection, None, Some(&paths.known_hosts()), cfg!(windows))
            .unwrap(),
    )
    .unwrap();
    let denied = Command::new(&binary)
        .arg("--config")
        .arg(paths.rclone_config())
        .arg("cat")
        .arg(&remote_file)
        .args(["--retries", "1", "--low-level-retries", "1"])
        .output()
        .unwrap();
    assert!(!denied.status.success());
}
