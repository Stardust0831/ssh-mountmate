//! Real OpenSSH discovery followed by rclone authentication against a server
//! offering RSA, ECDSA and Ed25519 at once. No FUSE or user SSH files required.
use super::*;
use crate::rclone::{RcloneRemote, write_rclone_remote};
use crate::{AuthMethod, ServerConfig};
use std::net::TcpListener;
use std::process::Child;
use std::time::Instant;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn paths(root: &Path) -> AppPaths {
    AppPaths {
        config_dir: root.join("config"),
        cache_dir: root.join("cache"),
        state_dir: root.join("state"),
        data_dir: root.join("data"),
    }
}

fn public_key(path: &Path) -> String {
    fs::read_to_string(path.with_extension("pub"))
        .unwrap()
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

fn list(rclone: &Path, paths: &AppPaths, algorithms: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(rclone);
    cmd.arg("--config").arg(paths.rclone_config()).args([
        "lsf",
        "fixture:",
        "--max-depth",
        "1",
        "--log-level",
        "NOTICE",
    ]);
    if let Some(algorithms) = algorithms {
        cmd.args(["--sftp-host-key-algorithms", algorithms]);
    }
    cmd.output().unwrap()
}

#[test]
#[ignore = "requires SSH_MOUNTMATE_TEST_RCLONE and ssh/ssh-keygen"]
fn live_multi_algorithm_authentication() {
    let rclone = PathBuf::from(std::env::var_os("SSH_MOUNTMATE_TEST_RCLONE").unwrap());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let paths = paths(&root.join("profile with spaces"));
    let remote = root.join("remote");
    fs::create_dir(&remote).unwrap();
    fs::write(remote.join("verified.txt"), "verified").unwrap();
    let login_key = root.join("login-key");
    assert!(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ecdsa", "-b", "256", "-N", "", "-f"])
            .arg(&login_key)
            .status()
            .unwrap()
            .success()
    );
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
        .to_string();
    let cache = root.join("server-cache");
    let server_log = root.join("server.log");
    let log = fs::File::create(&server_log).unwrap();
    let mut server = Server(
        Command::new(&rclone)
            .arg("--cache-dir")
            .arg(&cache)
            .args(["serve", "sftp"])
            .arg(&remote)
            .args([
                "--addr",
                &format!("127.0.0.1:{port}"),
                "--user",
                "fixture",
                "--pass",
                "fixture-password",
            ])
            .arg("--authorized-keys")
            .arg(login_key.with_extension("pub"))
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "SFTP fixture exited"
        );
        if fs::read_to_string(&server_log)
            .unwrap()
            .contains("SFTP server listening")
        {
            break;
        }
        assert!(Instant::now() < deadline, "SFTP fixture never became ready");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Force the v0.6.16 fallback: OpenSSH returns just one of the three keys.
    let keys = discover_host_keys(
        &paths,
        &root.join("missing-keyscan"),
        Path::new("ssh"),
        "127.0.0.1",
        &port,
    )
    .unwrap();
    assert_eq!(keys.len(), 1);
    let review = HostKeyReview::new(&paths, "127.0.0.1", &port, keys, None).unwrap();
    assert!(!paths.known_hosts().exists());
    review.confirm(&paths).unwrap();
    let password = Command::new(&rclone)
        .args(["obscure", "fixture-password"])
        .output()
        .unwrap();
    assert!(password.status.success());
    let mut connection = ServerConfig {
        id: "fixture".into(),
        host: "127.0.0.1".into(),
        port: port.clone(),
        user: "fixture".into(),
        auth: AuthMethod::Password,
        password_obscured: String::from_utf8(password.stdout).unwrap().trim().into(),
        ..ServerConfig::default()
    };
    let remote_config =
        RcloneRemote::for_server(&connection, None, Some(&paths.known_hosts()), cfg!(windows))
            .unwrap();
    write_rclone_remote(&paths, &remote_config).unwrap();
    let trusted = fs::read_to_string(paths.known_hosts()).unwrap();
    let other_algorithm = if trusted.contains("ssh-ed25519 ") {
        "rsa-sha2-512"
    } else {
        "ssh-ed25519"
    };
    let started = Instant::now();
    let rejected = list(&rclone, &paths, Some(other_algorithm));
    assert!(!rejected.status.success());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("knownhosts: key mismatch"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "identity error was retried: {:?}",
        started.elapsed()
    );
    let started = Instant::now();
    let listing = list(&rclone, &paths, None);
    assert!(
        listing.status.success(),
        "{}",
        String::from_utf8_lossy(&listing.stderr)
    );
    assert!(String::from_utf8_lossy(&listing.stdout).contains("verified.txt"));
    eprintln!(
        "confirmed single-key password SFTP: {:?}",
        started.elapsed()
    );

    // Check every supported public-key family with both login methods. RSA
    // trust must allow rsa-sha2-* signatures, not require SHA-1 signatures.
    for (filename, kind) in [
        ("id_ed25519", "ssh-ed25519"),
        ("id_ecdsa", "ecdsa-sha2-nistp256"),
        ("id_rsa", "ssh-rsa"),
    ] {
        let public = public_key(&cache.join("serve-sftp").join(filename));
        assert!(public.starts_with(kind));
        let replacement = HostKeyReview::new(
            &paths,
            "127.0.0.1",
            &port,
            vec![format!("[127.0.0.1]:{port} {public}")],
            Some(&paths.known_hosts()),
        )
        .unwrap();
        replacement.confirm(&paths).unwrap();
        for auth in [AuthMethod::Password, AuthMethod::Key] {
            connection.auth = auth;
            connection.key_file = login_key.display().to_string();
            let cfg = RcloneRemote::for_server(
                &connection,
                None,
                Some(&paths.known_hosts()),
                cfg!(windows),
            )
            .unwrap();
            write_rclone_remote(&paths, &cfg).unwrap();
            let started = Instant::now();
            let result = list(&rclone, &paths, None);
            assert!(
                result.status.success(),
                "{kind} {auth:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(String::from_utf8_lossy(&result.stdout).contains("verified.txt"));
            eprintln!("{kind} {auth:?} SFTP: {:?}", started.elapsed());
        }
    }

    // A changed key of the same type must still fail before authentication.
    let wrong_key = public_key(&login_key);
    fs::write(
        paths.known_hosts(),
        format!("[127.0.0.1]:{port} {wrong_key}\n"),
    )
    .unwrap();
    let cfg =
        RcloneRemote::for_server(&connection, None, Some(&paths.known_hosts()), cfg!(windows))
            .unwrap();
    write_rclone_remote(&paths, &cfg).unwrap();
    let failed = list(&rclone, &paths, None);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("knownhosts: key mismatch"));
    let before = fs::read(paths.known_hosts()).unwrap();
    let received = discover_host_keys(
        &paths,
        &root.join("missing-keyscan"),
        Path::new("ssh"),
        "127.0.0.1",
        &port,
    )
    .unwrap();
    let changed = HostKeyReview::new(
        &paths,
        "127.0.0.1",
        &port,
        received,
        Some(&paths.known_hosts()),
    )
    .unwrap();
    assert!(changed.changed());
    assert_ne!(changed.fingerprints(), changed.previous_fingerprints());
    assert_eq!(fs::read(paths.known_hosts()).unwrap(), before); // Preview/cancel.
    changed.confirm(&paths).unwrap();
    let cfg =
        RcloneRemote::for_server(&connection, None, Some(&paths.known_hosts()), cfg!(windows))
            .unwrap();
    write_rclone_remote(&paths, &cfg).unwrap();
    assert!(list(&rclone, &paths, None).status.success());

    // Imported terminal profiles must not inject text/PTY processing into the
    // binary SFTP stream. Keep the alias, identity and strict trust policy.
    let ssh_config = root.join("ssh-config");
    let ssh_path = |path: &Path| quote_ssh_value(&path.to_string_lossy().replace('\\', "/"));
    fs::write(&ssh_config, format!(
        "Host fixture\n HostName 127.0.0.1\n Port {port}\n User fixture\n IdentityFile {}\n IdentitiesOnly yes\n UserKnownHostsFile {}\n StrictHostKeyChecking yes\n RequestTTY force\n RemoteCommand echo UNEXPECTED_REMOTE_COMMAND\n PermitLocalCommand yes\n LocalCommand echo UNEXPECTED_LOCAL_COMMAND\n",
        ssh_path(&login_key), ssh_path(&paths.known_hosts()),
    )).unwrap();
    connection.connection_method = crate::ConnectionMethod::Openssh;
    connection.mode = "ssh_config".into();
    connection.source = "ssh_config".into();
    connection.host_alias = "fixture".into();
    connection.ssh_config_path = ssh_config.display().to_string();
    let mut cfg = RcloneRemote::for_server(&connection, None, None, cfg!(windows)).unwrap();
    // Windows shipping tests exercise the app's actual connector proxy too.
    if let Some(proxy) = std::env::var_os("SSH_MOUNTMATE_TEST_APP") {
        cfg.wrap_external_ssh(Path::new(&proxy), cfg!(windows))
            .unwrap();
    }
    write_rclone_remote(&paths, &cfg).unwrap();
    let listing = list(&rclone, &paths, None);
    assert!(
        listing.status.success(),
        "{}",
        String::from_utf8_lossy(&listing.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&listing.stdout).trim(),
        "verified.txt"
    );
    assert!(!String::from_utf8_lossy(&listing.stderr).contains("No host key validation"));
    // Transport overrides must not suppress a real host-key rejection.
    fs::write(
        paths.known_hosts(),
        format!("[127.0.0.1]:{port} {wrong_key}\n"),
    )
    .unwrap();
    let rejected = list(&rclone, &paths, None);
    assert!(!rejected.status.success());
    let detail = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        detail.contains("REMOTE HOST IDENTIFICATION HAS CHANGED"),
        "{detail}"
    );
}
