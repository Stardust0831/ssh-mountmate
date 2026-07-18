#[cfg(not(windows))]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(not(windows))]
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use thiserror::Error;
use uuid::Uuid;

#[cfg(windows)]
use sha2::{Digest, Sha256};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::CreateMutexW;

use crate::ssh::SshPermissionControl;
use crate::storage::read_json;

const MAX_MESSAGE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AppCommand {
    ShowMain,
    ShowTransfers,
    Mount { id: String },
    Unmount { id: String },
    Open { id: String },
    RefreshPath { path: String },
    Refresh { id: String, relative_dir: String },
    MountAll,
    MountStartup,
    UnmountAll,
    ExitForReplacement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningInstance {
    pub pid: u32,
    pub executable: PathBuf,
    pub version: String,
}

#[derive(Debug, Error)]
pub enum AppCommandError {
    #[error("another SSH MountMate instance is already running")]
    AlreadyRunning,
    #[error("SSH MountMate is not running")]
    NotRunning,
    #[error("the running SSH MountMate instance could not be verified")]
    IdentityMismatch,
    #[error("invalid app command state: {0}")]
    InvalidState(String),
    #[error("app command was rejected: {0}")]
    Rejected(String),
    #[error("could not connect to the running SSH MountMate instance: {0}")]
    Connect(#[source] std::io::Error),
    #[error("app command I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("app command JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("app command permissions failed: {0}")]
    Permissions(String),
}

pub struct InstanceLock {
    #[cfg(not(windows))]
    _file: File,
    #[cfg(windows)]
    mutex: HANDLE,
}

#[cfg(windows)]
// The handle is immutable and is only closed when the owning InstanceLock is dropped.
unsafe impl Send for InstanceLock {}
#[cfg(windows)]
unsafe impl Sync for InstanceLock {}

impl InstanceLock {
    pub fn try_acquire(path: &Path) -> Result<Self, AppCommandError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(windows)]
        {
            let name = windows_mutex_name(path)?;
            let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
            if mutex.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe {
                    CloseHandle(mutex);
                }
                return Err(AppCommandError::AlreadyRunning);
            }
            return Ok(Self { mutex });
        }
        #[cfg(not(windows))]
        {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)?;
            match file.try_lock_exclusive() {
                Ok(()) => Ok(Self { _file: file }),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    Err(AppCommandError::AlreadyRunning)
                }
                Err(error) => Err(error.into()),
            }
        }
    }
}

#[cfg(windows)]
impl Drop for InstanceLock {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.mutex);
        }
    }
}

#[cfg(windows)]
fn windows_mutex_name(path: &Path) -> Result<Vec<u16>, AppCommandError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = fs::canonicalize(parent)?;
    let identity = canonical_parent
        .join(path.file_name().unwrap_or_default())
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase();
    let digest = Sha256::digest(identity.as_bytes());
    let name = format!("Local\\SSHMountMate.Instance.{digest:x}");
    Ok(name.encode_utf16().chain(std::iter::once(0)).collect())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CommandState {
    pid: u32,
    started_at: u64,
    executable: PathBuf,
    #[serde(default)]
    version: String,
    port: u16,
    token: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CommandRequest {
    token: String,
    command: AppCommand,
}

#[derive(Debug, Serialize, Deserialize)]
struct CommandResponse {
    ok: bool,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentity {
    started_at: u64,
    executable: PathBuf,
}

trait ProcessProbe {
    fn identity(&self, pid: u32) -> Option<ProcessIdentity>;
}

struct SystemProcessProbe;

impl ProcessProbe for SystemProcessProbe {
    fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        let process = system.process(pid)?;
        Some(ProcessIdentity {
            started_at: process.start_time(),
            executable: process.exe()?.to_owned(),
        })
    }
}

pub struct AppCommandServer {
    state_path: PathBuf,
    token: String,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AppCommandServer {
    pub fn start(
        state_path: PathBuf,
        permissions: &dyn SshPermissionControl,
        callback: impl Fn(AppCommand) + Send + Sync + 'static,
    ) -> Result<Self, AppCommandError> {
        Self::start_with_version(state_path, permissions, env!("CARGO_PKG_VERSION"), callback)
    }

    pub fn start_with_version(
        state_path: PathBuf,
        permissions: &dyn SshPermissionControl,
        version: &str,
        callback: impl Fn(AppCommand) + Send + Sync + 'static,
    ) -> Result<Self, AppCommandError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let identity = SystemProcessProbe
            .identity(std::process::id())
            .ok_or(AppCommandError::IdentityMismatch)?;
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let state = CommandState {
            pid: std::process::id(),
            started_at: identity.started_at,
            executable: identity.executable,
            version: version.to_owned(),
            port,
            token: token.clone(),
        };
        write_private_state(&state_path, &state, permissions)?;

        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread_token = token.clone();
        let callback = Arc::new(callback);
        let thread = thread::Builder::new()
            .name("ssh-mountmate-command".into())
            .spawn(move || {
                while !thread_stopping.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            handle_connection(stream, peer, &thread_token, callback.as_ref());
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            state_path,
            token,
            stopping,
            thread: Some(thread),
        })
    }
}

pub fn running_instance(state_path: &Path) -> Result<RunningInstance, AppCommandError> {
    running_instance_with_probe(state_path, &SystemProcessProbe)
}

fn running_instance_with_probe(
    state_path: &Path,
    probe: &dyn ProcessProbe,
) -> Result<RunningInstance, AppCommandError> {
    let state: CommandState = match read_json(state_path) {
        Ok(state) => state,
        Err(_) if !state_path.exists() => return Err(AppCommandError::NotRunning),
        Err(error) => return Err(AppCommandError::InvalidState(error.to_string())),
    };
    validate_state(&state, probe)?;
    Ok(RunningInstance {
        pid: state.pid,
        executable: state.executable,
        version: state.version,
    })
}

pub fn same_instance_build(
    running: &RunningInstance,
    current_executable: &Path,
    current_version: &str,
) -> bool {
    same_executable(&running.executable, current_executable) && running.version == current_version
}

impl Drop for AppCommandServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let owns_state = read_json::<CommandState>(&self.state_path)
            .is_ok_and(|state| constant_time_eq(state.token.as_bytes(), self.token.as_bytes()));
        if owns_state {
            let _ = fs::remove_file(&self.state_path);
        }
    }
}

pub fn send_command(
    state_path: &Path,
    command: &AppCommand,
    timeout: Duration,
) -> Result<(), AppCommandError> {
    send_command_with_probe(state_path, command, timeout, &SystemProcessProbe)
}

pub fn send_command_retry(
    state_path: &Path,
    command: &AppCommand,
    timeout: Duration,
) -> Result<(), AppCommandError> {
    let started = Instant::now();
    loop {
        match send_command(state_path, command, Duration::from_millis(500)) {
            Ok(()) => return Ok(()),
            Err(error) if retryable_before_delivery(&error) && started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

fn retryable_before_delivery(error: &AppCommandError) -> bool {
    matches!(
        error,
        AppCommandError::NotRunning | AppCommandError::Connect(_)
    )
}

fn send_command_with_probe(
    state_path: &Path,
    command: &AppCommand,
    timeout: Duration,
    probe: &dyn ProcessProbe,
) -> Result<(), AppCommandError> {
    let state: CommandState = match read_json(state_path) {
        Ok(state) => state,
        Err(_) if !state_path.exists() => return Err(AppCommandError::NotRunning),
        Err(error) => return Err(AppCommandError::InvalidState(error.to_string())),
    };
    validate_state(&state, probe)?;
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, state.port)),
        timeout,
    )
    .map_err(AppCommandError::Connect)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let request = CommandRequest {
        token: state.token,
        command: command.clone(),
    };
    serde_json::to_writer(&mut stream, &request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let response: CommandResponse = read_message(&mut stream)?;
    if response.ok {
        Ok(())
    } else {
        Err(AppCommandError::Rejected(response.error))
    }
}

fn validate_state(state: &CommandState, probe: &dyn ProcessProbe) -> Result<(), AppCommandError> {
    if state.port == 0
        || state.token.len() != 64
        || !state.token.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppCommandError::InvalidState(
            "invalid port or token".into(),
        ));
    }
    let identity = probe
        .identity(state.pid)
        .ok_or(AppCommandError::NotRunning)?;
    if identity.started_at != state.started_at
        || !same_executable(&identity.executable, &state.executable)
    {
        return Err(AppCommandError::IdentityMismatch);
    }
    Ok(())
}

fn same_executable(actual: &Path, expected: &Path) -> bool {
    let normalize = |path: &Path| {
        let value = path.to_string_lossy().replace('\\', "/");
        if cfg!(windows) {
            value.to_lowercase()
        } else {
            value
        }
    };
    normalize(actual) == normalize(expected)
}

fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    token: &str,
    callback: &dyn Fn(AppCommand),
) {
    let response = (|| {
        if !peer.ip().is_loopback() {
            return Err("non-loopback client".into());
        }
        stream
            .set_nonblocking(false)
            .map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        let request: CommandRequest =
            read_message(&mut stream).map_err(|error| error.to_string())?;
        if !constant_time_eq(request.token.as_bytes(), token.as_bytes()) {
            return Err("invalid command token".into());
        }
        callback(request.command);
        Ok(())
    })();
    let response = match response {
        Ok(()) => CommandResponse {
            ok: true,
            error: String::new(),
        },
        Err(error) => CommandResponse { ok: false, error },
    };
    let _ = serde_json::to_writer(&mut stream, &response);
    let _ = stream.write_all(b"\n");
    let _ = stream.flush();
}

fn read_message<T: for<'de> Deserialize<'de>>(
    stream: &mut TcpStream,
) -> Result<T, AppCommandError> {
    let mut reader = BufReader::new(stream).take(MAX_MESSAGE_BYTES + 1);
    let mut message = Vec::new();
    reader.read_until(b'\n', &mut message)?;
    if message.is_empty() || message.len() as u64 > MAX_MESSAGE_BYTES || !message.ends_with(b"\n") {
        return Err(AppCommandError::InvalidState("invalid command size".into()));
    }
    message.pop();
    Ok(serde_json::from_slice(&message)?)
}

fn write_private_state(
    path: &Path,
    state: &CommandState,
    permissions: &dyn SshPermissionControl,
) -> Result<(), AppCommandError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    permissions
        .restrict_private_path(parent, true)
        .map_err(AppCommandError::Permissions)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        permissions
            .restrict_private_path(&temporary, false)
            .map_err(AppCommandError::Permissions)?;
        serde_json::to_writer(&mut file, state)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&temporary, path)?;
        Ok::<_, AppCommandError>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::*;

    struct TestPermissions;

    impl SshPermissionControl for TestPermissions {
        fn restrict_private_path(&self, _path: &Path, _directory: bool) -> Result<(), String> {
            Ok(())
        }
    }

    struct FakeProbe(Option<ProcessIdentity>);

    impl ProcessProbe for FakeProbe {
        fn identity(&self, _pid: u32) -> Option<ProcessIdentity> {
            self.0.clone()
        }
    }

    #[test]
    fn authenticated_command_is_forwarded() {
        let temp = tempdir().unwrap();
        let state = temp.path().join("command.json");
        let received = Arc::new(Mutex::new(Vec::new()));
        let callback_received = Arc::clone(&received);
        let server = AppCommandServer::start(state.clone(), &TestPermissions, move |command| {
            callback_received.lock().unwrap().push(command);
        })
        .unwrap();

        send_command(&state, &AppCommand::ShowTransfers, Duration::from_secs(1)).unwrap();
        assert_eq!(
            received.lock().unwrap().as_slice(),
            &[AppCommand::ShowTransfers]
        );
        drop(server);
        assert!(!state.exists());
    }

    #[test]
    fn second_instance_lock_is_rejected_until_release() {
        const CHILD_PATH: &str = "SSH_MOUNTMATE_LOCK_TEST_PATH";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            assert!(matches!(
                InstanceLock::try_acquire(Path::new(&path)),
                Err(AppCommandError::AlreadyRunning)
            ));
            return;
        }

        let temp = tempdir().unwrap();
        let path = temp.path().join("instance.lock");
        let first = InstanceLock::try_acquire(&path).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app_command::tests::second_instance_lock_is_rejected_until_release",
            ])
            .env(CHILD_PATH, &path)
            .status()
            .unwrap();
        assert!(status.success());
        drop(first);
        InstanceLock::try_acquire(&path).unwrap();
    }

    #[test]
    fn pid_reuse_and_executable_mismatch_are_rejected() {
        let state = CommandState {
            pid: 42,
            started_at: 100,
            executable: PathBuf::from("/app/SSHMountMate"),
            version: "0.4.0-alpha.7".into(),
            port: 1234,
            token: "a".repeat(64),
        };
        assert!(matches!(
            validate_state(
                &state,
                &FakeProbe(Some(ProcessIdentity {
                    started_at: 101,
                    executable: state.executable.clone(),
                }))
            ),
            Err(AppCommandError::IdentityMismatch)
        ));
        assert!(matches!(
            validate_state(
                &state,
                &FakeProbe(Some(ProcessIdentity {
                    started_at: 100,
                    executable: PathBuf::from("/other/app"),
                }))
            ),
            Err(AppCommandError::IdentityMismatch)
        ));
    }

    #[test]
    fn build_identity_requires_both_version_and_executable() {
        let running = RunningInstance {
            pid: 42,
            executable: PathBuf::from("/app/SSHMountMate"),
            version: "0.4.0-alpha.7".into(),
        };
        assert!(same_instance_build(
            &running,
            Path::new("/app/SSHMountMate"),
            "0.4.0-alpha.7"
        ));
        assert!(!same_instance_build(
            &running,
            Path::new("/downloads/SSHMountMate"),
            "0.4.0-alpha.7"
        ));
        assert!(!same_instance_build(
            &running,
            Path::new("/app/SSHMountMate"),
            "0.4.0-alpha.8"
        ));
    }

    #[test]
    fn legacy_command_state_without_version_remains_readable() {
        let state: CommandState = serde_json::from_str(
            r#"{"pid":42,"started_at":100,"executable":"/app/SSHMountMate","port":1234,"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        )
        .unwrap();
        assert!(state.version.is_empty());
    }

    #[test]
    fn invalid_tokens_and_oversized_messages_are_rejected() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"public"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn retries_stop_once_command_delivery_may_have_started() {
        assert!(retryable_before_delivery(&AppCommandError::NotRunning));
        assert!(retryable_before_delivery(&AppCommandError::Connect(
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "not listening")
        )));
        assert!(!retryable_before_delivery(&AppCommandError::Io(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "response timed out")
        )));
    }
}
