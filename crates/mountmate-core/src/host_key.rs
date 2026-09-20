//! Host-key discovery is separate from trust: only an explicit confirmation
//! may write discovered keys into the application's known_hosts file.
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use wait_timeout::ChildExt;

use crate::paths::AppPaths;
use crate::ssh::{
    SshError, concise_ssh_diagnostics, hashed_host_matches, known_hosts_marker,
    normalize_host_key_output, quote_ssh_value, scan_host_keys, validate_host_alias, validate_port,
};
use crate::storage::{FileLock, atomic_write, restrict_private_path};

#[derive(Debug, Clone)]
pub struct HostKeyReview {
    host: String,
    port: String,
    keys: Vec<String>,
    previous: Vec<String>,
    source: Option<PathBuf>,
    managed_before: Vec<String>,
}

impl HostKeyReview {
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> &str {
        &self.port
    }
    pub fn changed(&self) -> bool {
        !self.previous.is_empty()
    }
    pub fn fingerprints(&self) -> String {
        fingerprints(&self.keys)
    }
    pub fn previous_fingerprints(&self) -> String {
        fingerprints(&self.previous)
    }

    pub(crate) fn new(
        paths: &AppPaths,
        host: &str,
        port: &str,
        keys: Vec<String>,
        source: Option<&Path>,
    ) -> Result<Self, SshError> {
        validate_host_alias(host)?;
        let port = validate_port(port)?;
        let keys = normalize_host_key_output(host, &port, &keys.join("\n"))
            .into_iter()
            .map(|line| {
                line.split_whitespace()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Err(SshError::Command(
                "no valid server host key to confirm".into(),
            ));
        }
        let marker = known_hosts_marker(host, &port);
        let previous = source
            .map(|path| read_bindings(path, &marker))
            .transpose()?
            .unwrap_or_default();
        let managed_before = read_bindings(&paths.known_hosts(), &marker)?;
        Ok(Self {
            host: host.into(),
            port,
            keys,
            previous,
            source: source.map(Path::to_owned),
            managed_before,
        })
    }

    pub(crate) fn matches_previous(&self) -> bool {
        same_keys(&self.keys, &self.previous)
    }

    pub(crate) fn includes_trusted_key(&self) -> bool {
        self.keys.iter().any(|key| self.previous.contains(key))
    }

    pub fn is_confirmed(&self, paths: &AppPaths) -> bool {
        read_bindings(
            &paths.known_hosts(),
            &known_hosts_marker(&self.host, &self.port),
        )
        .is_ok_and(|keys| same_keys(&keys, &self.keys))
    }

    /// Save exactly the reviewed keys. Never replace another confirmation
    /// that arrived while this dialog was open, or edit the user's SSH files.
    pub fn confirm(&self, paths: &AppPaths) -> Result<(), SshError> {
        let _lock = FileLock::acquire(&paths.known_hosts_lock(), Duration::from_secs(30))?;
        let marker = known_hosts_marker(&self.host, &self.port);
        let managed = paths.known_hosts();
        let current = read_bindings(&managed, &marker)?;
        if same_keys(&current, &self.keys) {
            return Ok(());
        }
        if !same_keys(&current, &self.managed_before)
            || self.source.as_ref().is_some_and(|source| {
                read_bindings(source, &marker)
                    .map_or(true, |keys| !same_keys(&keys, &self.previous))
            })
        {
            return Err(SshError::Command(
                "saved host keys changed while confirmation was open; retry mounting to review them again".into(),
            ));
        }
        let content = read_optional(&managed)?;
        let mut updated = Vec::new();
        for line in content.lines() {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            if parts.len() < 3 || parts[0].starts_with(['#', '@']) {
                updated.push(line.to_owned());
                continue;
            }
            let remaining = parts[0]
                .split(',')
                .filter(|pattern| *pattern != marker && !hashed_host_matches(pattern, &marker))
                .collect::<Vec<_>>();
            if remaining.len() == parts[0].split(',').count() {
                updated.push(line.to_owned());
            } else if !remaining.is_empty() {
                updated.push(format!("{} {}", remaining.join(","), parts[1..].join(" ")));
            }
        }
        updated.extend(self.keys.iter().cloned());
        atomic_write(&managed, format!("{}\n", updated.join("\n")).as_bytes())?;
        Ok(())
    }
}

fn same_keys(left: &[String], right: &[String]) -> bool {
    let set = |keys: &[String]| {
        keys.iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    };
    set(left) == set(right)
}

fn fingerprints(keys: &[String]) -> String {
    keys.iter()
        .filter_map(|line| {
            let (_, key) = line.split_once(' ')?;
            let key = ssh_key::PublicKey::from_openssh(key).ok()?;
            Some(format!(
                "{}  {}",
                key.algorithm(),
                key.fingerprint(ssh_key::HashAlg::Sha256)
            ))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_optional(path: &Path) -> Result<String, SshError> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(SshError::Io {
            path: path.into(),
            source,
        }),
    }
}

fn read_bindings(path: &Path, marker: &str) -> Result<Vec<String>, SshError> {
    Ok(read_optional(path)?
        .lines()
        .filter_map(|line| {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            if parts.len() < 3
                || parts[0].starts_with(['#', '@'])
                || !parts[0]
                    .split(',')
                    .any(|pattern| pattern == marker || hashed_host_matches(pattern, marker))
            {
                return None;
            }
            let key =
                ssh_key::PublicKey::from_openssh(&format!("{} {}", parts[1], parts[2])).ok()?;
            if matches!(key.algorithm(), ssh_key::Algorithm::Other(_)) {
                return None;
            }
            Some(format!("{marker} {} {}", parts[1], parts[2]))
        })
        .collect())
}

pub(crate) fn discover_host_keys(
    paths: &AppPaths,
    keyscan: &Path,
    ssh: &Path,
    host: &str,
    port: &str,
) -> Result<Vec<String>, SshError> {
    validate_host_alias(host)?;
    validate_port(port)?;
    match scan_host_keys(keyscan, host, port, Duration::from_secs(12)) {
        Ok(keys) => Ok(keys),
        Err(scan_error) => probe_host_keys(paths, ssh, host, port, Duration::from_secs(12))
            .map_err(|probe_error| {
                SshError::Command(format!(
                    "{scan_error}\nSSH handshake fallback: {probe_error}"
                ))
            }),
    }
}

struct ProbeDirectory(PathBuf);
impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Work around Windows ssh-keyscan's unsupported-KEX bug using ssh's normal
/// handshake. No credentials, agent, config, remote command, or real trust file
/// is used. Authentication failure is expected; only the temporary key matters.
pub(crate) fn probe_host_keys(
    paths: &AppPaths,
    ssh: &Path,
    host: &str,
    port: &str,
    timeout: Duration,
) -> Result<Vec<String>, SshError> {
    validate_host_alias(host)?;
    let port = validate_port(port)?;
    fs::create_dir_all(&paths.state_dir).map_err(|source| SshError::Io {
        path: paths.state_dir.clone(),
        source,
    })?;
    let directory = paths
        .state_dir
        .join(format!("host-key-probe-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&directory).map_err(|source| SshError::Io {
        path: directory.clone(),
        source,
    })?;
    let directory = ProbeDirectory(directory);
    restrict_private_path(&directory.0, true).map_err(|source| SshError::Io {
        path: directory.0.clone(),
        source,
    })?;
    let temporary = directory.0.join("known_hosts");
    let mut command = Command::new(ssh);
    command
        .args([
            "-F",
            "none",
            "-T",
            "-N",
            "-p",
            &port,
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "GlobalKnownHostsFile=none",
            "-o",
            "HashKnownHosts=no",
            "-o",
            "CheckHostIP=no",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "PreferredAuthentications=none",
            "-o",
            "PubkeyAuthentication=no",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=no",
            "-o",
            "IdentityAgent=none",
            "-o",
            "IdentityFile=none",
            "-l",
            "ssh-mountmate-host-key-probe",
            "-o",
        ])
        .arg(format!(
            "UserKnownHostsFile={}",
            quote_ssh_value(&temporary.to_string_lossy().replace('\\', "/"))
        ))
        .arg(host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|source| SshError::Io {
        path: ssh.into(),
        source,
    })?;
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let result = child.wait_timeout(timeout);
    if !matches!(result, Ok(Some(_))) {
        let _ = child.kill();
        let _ = child.wait();
    }
    let diagnostics = reader.join().unwrap_or_default();
    if let Err(source) = result {
        return Err(SshError::Io {
            path: ssh.into(),
            source,
        });
    }
    let keys = normalize_host_key_output(host, &port, &read_optional(&temporary)?);
    if keys.is_empty() {
        return Err(SshError::Command(format!(
            "could not obtain the server host key for {host}:{port}: {}",
            concise_ssh_diagnostics(&diagnostics)
        )));
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XFSqti";

    fn paths(root: &Path) -> AppPaths {
        AppPaths {
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            data_dir: root.join("data"),
        }
    }

    fn different_key() -> String {
        use base64::Engine;
        let (kind, encoded) = KEY.split_once(' ').unwrap();
        let mut data = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        *data.last_mut().unwrap() ^= 1;
        format!(
            "{kind} {}",
            base64::engine::general_purpose::STANDARD.encode(data)
        )
    }

    #[test]
    fn first_use_requires_confirmation_and_saves_only_the_reviewed_key() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let review = HostKeyReview::new(
            &paths,
            "example.com",
            "2222",
            vec![format!("[example.com]:2222 {KEY}")],
            None,
        )
        .unwrap();
        assert!(!review.changed());
        assert!(review.fingerprints().contains("ssh-ed25519  SHA256:"));
        assert!(!review.is_confirmed(&paths));
        assert!(!paths.known_hosts().exists()); // Preview/cancel is read-only.
        review.confirm(&paths).unwrap();
        assert!(review.is_confirmed(&paths));
        assert_eq!(
            fs::read_to_string(paths.known_hosts()).unwrap(),
            format!("[example.com]:2222 {KEY}\n")
        );
        review.confirm(&paths).unwrap(); // Same-server parallel mapping is idempotent.
        assert_eq!(
            fs::read_to_string(paths.known_hosts())
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn changed_key_review_preserves_other_hosts_and_external_trust_files() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        let old = different_key();
        let source = temp.path().join("user-known-hosts");
        fs::write(&source, format!("example.com {old}\n")).unwrap();
        fs::write(
            paths.known_hosts(),
            format!("# keep this\nother.example {old}\n"),
        )
        .unwrap();
        let review = HostKeyReview::new(
            &paths,
            "example.com",
            "22",
            vec![format!("example.com {KEY}")],
            Some(&source),
        )
        .unwrap();
        assert!(review.changed());
        assert_ne!(review.fingerprints(), review.previous_fingerprints());
        let before = fs::read(paths.known_hosts()).unwrap();
        assert_eq!(fs::read(paths.known_hosts()).unwrap(), before);
        review.confirm(&paths).unwrap();
        assert_eq!(
            fs::read_to_string(source).unwrap(),
            format!("example.com {old}\n")
        );
        assert_eq!(
            fs::read_to_string(paths.known_hosts()).unwrap(),
            format!("# keep this\nother.example {old}\nexample.com {KEY}\n")
        );
    }

    #[test]
    fn replacing_managed_key_preserves_other_names_on_a_shared_line() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        let old = different_key();
        fs::write(
            paths.known_hosts(),
            format!("example.com,other.example {old}\n[example.com]:2222 {old}\n"),
        )
        .unwrap();
        let review = HostKeyReview::new(
            &paths,
            "example.com",
            "22",
            vec![format!("example.com {KEY}")],
            Some(&paths.known_hosts()),
        )
        .unwrap();
        review.confirm(&paths).unwrap();
        assert_eq!(
            fs::read_to_string(paths.known_hosts()).unwrap(),
            format!("other.example {old}\n[example.com]:2222 {old}\nexample.com {KEY}\n")
        );
    }

    #[test]
    fn stale_dialog_cannot_overwrite_a_concurrent_confirmation() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let first = HostKeyReview::new(
            &paths,
            "example.com",
            "22",
            vec![format!("example.com {KEY}")],
            None,
        )
        .unwrap();
        let second = HostKeyReview::new(
            &paths,
            "example.com",
            "22",
            vec![format!("example.com {}", different_key())],
            None,
        )
        .unwrap();
        first.confirm(&paths).unwrap();
        let before = fs::read(paths.known_hosts()).unwrap();
        assert!(second.confirm(&paths).is_err());
        assert_eq!(fs::read(paths.known_hosts()).unwrap(), before);
    }

    #[test]
    fn hashed_host_keys_can_be_reviewed_and_replaced_without_duplicates() {
        use base64::Engine;
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        let marker = "[example.com]:2222";
        let salt = b"host key review salt";
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, salt);
        let tag = ring::hmac::sign(&key, marker.as_bytes());
        let engine = base64::engine::general_purpose::STANDARD;
        let hashed = format!("|1|{}|{}", engine.encode(salt), engine.encode(tag.as_ref()));
        fs::write(
            paths.known_hosts(),
            format!("{hashed} {}\n", different_key()),
        )
        .unwrap();
        let review = HostKeyReview::new(
            &paths,
            "example.com",
            "2222",
            vec![format!("{marker} {KEY}")],
            Some(&paths.known_hosts()),
        )
        .unwrap();
        assert!(review.changed());
        review.confirm(&paths).unwrap();
        assert_eq!(
            fs::read_to_string(paths.known_hosts()).unwrap(),
            format!("{marker} {KEY}\n")
        );
    }

    #[test]
    fn failed_probe_leaves_no_temporary_or_trusted_keys() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        assert!(
            probe_host_keys(
                &paths,
                &temp.path().join("missing-ssh"),
                "example.com",
                "2222",
                Duration::from_millis(50)
            )
            .is_err()
        );
        assert!(!paths.known_hosts().exists());
        assert_eq!(fs::read_dir(paths.state_dir).unwrap().count(), 0);
    }

    #[test]
    fn invalid_keys_and_changes_to_external_trust_are_rejected() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        assert!(
            HostKeyReview::new(
                &paths,
                "example.com",
                "22",
                vec!["example.com ssh-ed25519 invalid".into()],
                None
            )
            .is_err()
        );
        let source = temp.path().join("known_hosts");
        fs::write(&source, format!("example.com {KEY}\n")).unwrap();
        let review = HostKeyReview::new(
            &paths,
            "example.com",
            "22",
            vec![format!("example.com {}", different_key())],
            Some(&source),
        )
        .unwrap();
        fs::write(&source, "# changed externally\n").unwrap();
        assert!(review.confirm(&paths).is_err());
        assert!(!paths.known_hosts().exists());
    }

    #[cfg(unix)]
    #[test]
    fn failed_keyscan_reports_stderr_and_probe_uses_no_login_credentials() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempdir().unwrap();
        let paths = paths(&temp.path().join("profile with spaces"));
        let keyscan = temp.path().join("ssh-keyscan");
        fs::write(&keyscan, "#!/bin/sh\necho 'choose_kex: unsupported KEX method sntrup761x25519-sha512@openssh.com' >&2\nexit 1\n").unwrap();
        fs::set_permissions(&keyscan, fs::Permissions::from_mode(0o700)).unwrap();
        let error =
            scan_host_keys(&keyscan, "example.com", "2222", Duration::from_secs(2)).unwrap_err();
        assert!(error.to_string().contains("unsupported KEX method"));
        let ssh = temp.path().join("ssh");
        let fixture = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
for arg do
  case "$arg" in
    UserKnownHostsFile=*) file=${{arg#UserKnownHostsFile=}}; file=${{file#\"}}; file=${{file%\"}};;
  esac
done
printf '%s\n' '[example.com]:2222 {KEY}' > "$file"
echo 'Permission denied (publickey).' >&2
exit 255
"#,
            temp.path().join("arguments").display()
        );
        fs::write(&ssh, fixture).unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let keys = discover_host_keys(&paths, &keyscan, &ssh, "example.com", "2222").unwrap();
        assert_eq!(keys, [format!("[example.com]:2222 {KEY}")]);
        let arguments = fs::read_to_string(temp.path().join("arguments")).unwrap();
        for required in [
            "-F\nnone",
            "-p\n2222",
            "PreferredAuthentications=none",
            "IdentityAgent=none",
            "IdentityFile=none",
            "PubkeyAuthentication=no",
            "PasswordAuthentication=no",
            "KbdInteractiveAuthentication=no",
            "GlobalKnownHostsFile=none",
        ] {
            assert!(arguments.contains(required), "missing {required}");
        }
        assert!(!paths.known_hosts().exists());
        assert_eq!(fs::read_dir(paths.state_dir).unwrap().count(), 0);
    }

    #[test]
    #[ignore = "requires the CI local SFTP fixture and SSH_MOUNTMATE_HOST_KEY_TEST_PORT/PUBLIC"]
    fn live_host_key_probe() {
        let port = std::env::var("SSH_MOUNTMATE_HOST_KEY_TEST_PORT").unwrap();
        let public = std::env::var("SSH_MOUNTMATE_HOST_KEY_TEST_PUBLIC").unwrap();
        let public = public
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        let temp = tempdir().unwrap();
        let paths = paths(&temp.path().join("profile with spaces"));
        let ssh = Path::new(if cfg!(windows) { "ssh.exe" } else { "ssh" });
        let keys = discover_host_keys(
            &paths,
            &temp.path().join("missing-keyscan"),
            ssh,
            "127.0.0.1",
            &port,
        )
        .unwrap();
        assert_eq!(keys, [format!("[127.0.0.1]:{port} {public}")]);
        assert!(!paths.known_hosts().exists());
        assert_eq!(fs::read_dir(&paths.state_dir).unwrap().count(), 0);
        let review = HostKeyReview::new(&paths, "127.0.0.1", &port, keys, None).unwrap();
        review.confirm(&paths).unwrap();
        assert!(review.is_confirmed(&paths));
    }
}
