#[cfg(not(windows))]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Arc, Mutex};
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
const AUTH_WORKER_COUNT: usize = 4;
const AUTH_QUEUE_CAPACITY: usize = 16;
const UNAUTHENTICATED_DEADLINE: Duration = Duration::from_millis(500);

struct AcceptedConnection {
    stream: TcpStream,
    peer: SocketAddr,
    deadline: Instant,
}

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

struct PendingCommandState {
    path: PathBuf,
    token: String,
    armed: bool,
}

impl PendingCommandState {
    fn new(path: PathBuf, token: String) -> Self {
        Self {
            path,
            token,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingCommandState {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let owns_state = read_json::<CommandState>(&self.path)
            .is_ok_and(|state| constant_time_eq(state.token.as_bytes(), self.token.as_bytes()));
        if owns_state {
            let _ = fs::remove_file(&self.path);
        }
    }
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
        let mut pending_state = PendingCommandState::new(state_path.clone(), token.clone());

        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread_token = token.clone();
        let callback = Arc::new(Mutex::new(callback));
        let (connection_sender, connection_receiver) =
            mpsc::sync_channel::<AcceptedConnection>(AUTH_QUEUE_CAPACITY);
        let connection_receiver = Arc::new(Mutex::new(connection_receiver));
        for worker_index in 0..AUTH_WORKER_COUNT {
            let worker_receiver = Arc::clone(&connection_receiver);
            let worker_token = thread_token.clone();
            let worker_callback = Arc::clone(&callback);
            let worker_stopping = Arc::clone(&thread_stopping);
            thread::Builder::new()
                .name(format!("ssh-mountmate-command-auth-{worker_index}"))
                .spawn(move || {
                    loop {
                        let connection = {
                            let Ok(receiver) = worker_receiver.lock() else {
                                return;
                            };
                            receiver.recv()
                        };
                        let Ok(connection) = connection else {
                            return;
                        };
                        handle_connection(
                            connection.stream,
                            connection.peer,
                            connection.deadline,
                            &worker_token,
                            worker_callback.as_ref(),
                            worker_stopping.as_ref(),
                        );
                    }
                })?;
        }
        let thread = thread::Builder::new()
            .name("ssh-mountmate-command".into())
            .spawn(move || {
                while !thread_stopping.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            let connection = AcceptedConnection {
                                stream,
                                peer,
                                deadline: Instant::now() + UNAUTHENTICATED_DEADLINE,
                            };
                            match connection_sender.try_send(connection) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_connection)) => {}
                                Err(TrySendError::Disconnected(_connection)) => break,
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        let server = Self {
            state_path,
            token,
            stopping,
            thread: Some(thread),
        };
        pending_state.disarm();
        Ok(server)
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

fn handle_connection<F>(
    mut stream: TcpStream,
    peer: SocketAddr,
    deadline: Instant,
    token: &str,
    callback: &Mutex<F>,
    stopping: &AtomicBool,
) where
    F: Fn(AppCommand),
{
    let response = (|| {
        if !peer.ip().is_loopback() {
            return Err("non-loopback client".into());
        }
        stream
            .set_nonblocking(false)
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        let request: CommandRequest =
            read_message_until(&mut stream, deadline).map_err(|error| error.to_string())?;
        if !constant_time_eq(request.token.as_bytes(), token.as_bytes()) {
            return Err("invalid command token".into());
        }
        let callback = callback
            .lock()
            .map_err(|_| "command callback is unavailable".to_owned())?;
        if stopping.load(Ordering::Acquire) {
            return Err("command server is stopping".into());
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
    read_message_with_deadline(stream, None)
}

fn read_message_until<T: for<'de> Deserialize<'de>>(
    stream: &mut TcpStream,
    deadline: Instant,
) -> Result<T, AppCommandError> {
    read_message_with_deadline(stream, Some(deadline))
}

fn read_message_with_deadline<T: for<'de> Deserialize<'de>>(
    stream: &mut TcpStream,
    deadline: Option<Instant>,
) -> Result<T, AppCommandError> {
    let mut message = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        if let Some(deadline) = deadline {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "request deadline elapsed")
                })?;
            stream.set_read_timeout(Some(remaining))?;
        }
        let capacity = (MAX_MESSAGE_BYTES + 1).saturating_sub(message.len() as u64) as usize;
        if capacity == 0 {
            return Err(AppCommandError::InvalidState("invalid command size".into()));
        }
        let read_capacity = buffer.len().min(capacity);
        let read = stream.read(&mut buffer[..read_capacity])?;
        if read == 0 {
            return Err(AppCommandError::InvalidState("incomplete command".into()));
        }
        if let Some(newline) = buffer[..read].iter().position(|byte| *byte == b'\n') {
            message.extend_from_slice(&buffer[..=newline]);
            break;
        }
        message.extend_from_slice(&buffer[..read]);
    }
    if message.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(AppCommandError::InvalidState("invalid command size".into()));
    }
    message.pop();
    let parsed = serde_json::from_slice(&message)?;
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "request deadline elapsed").into(),
        );
    }
    Ok(parsed)
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
    fn unpublished_server_state_is_removed_by_startup_guard() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("command.json");
        let state = CommandState {
            pid: 1,
            started_at: 1,
            executable: PathBuf::from("app"),
            version: "test".into(),
            port: 1234,
            token: "a".repeat(64),
        };
        write_private_state(&path, &state, &TestPermissions).unwrap();

        drop(PendingCommandState::new(path.clone(), state.token));

        assert!(!path.exists());
    }

    #[test]
    fn slow_unauthenticated_client_does_not_block_valid_command() {
        let temp = tempdir().unwrap();
        let state_path = temp.path().join("command.json");
        let received = Arc::new(Mutex::new(Vec::new()));
        let callback_received = Arc::clone(&received);
        let server =
            AppCommandServer::start(state_path.clone(), &TestPermissions, move |command| {
                callback_received.lock().unwrap().push(command);
            })
            .unwrap();
        let state: CommandState = read_json(&state_path).unwrap();
        let mut slow = TcpStream::connect((Ipv4Addr::LOCALHOST, state.port)).unwrap();
        slow.write_all(b"{").unwrap();
        thread::sleep(Duration::from_millis(100));

        let started = Instant::now();
        let result = send_command(
            &state_path,
            &AppCommand::ShowTransfers,
            Duration::from_secs(1),
        );
        let elapsed = started.elapsed();
        drop(slow);

        assert!(
            result.is_ok(),
            "valid command failed after {elapsed:?}: {result:?}"
        );
        assert!(elapsed < Duration::from_secs(1));
        assert_eq!(
            received.lock().unwrap().as_slice(),
            &[AppCommand::ShowTransfers]
        );
        drop(server);
    }

    #[test]
    fn drip_fed_bytes_cannot_extend_unauthenticated_deadline() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let writer = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            for _ in 0..20 {
                if stream.write_all(b"{").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(40));
            }
        });
        let (mut stream, _) = listener.accept().unwrap();
        let started = Instant::now();
        let result =
            read_message_until::<CommandRequest>(&mut stream, started + Duration::from_millis(150));
        drop(stream);
        writer.join().unwrap();

        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(400));
    }

    #[test]
    fn json_parsing_cannot_cross_unauthenticated_deadline() {
        struct SlowRequest;

        impl<'de> Deserialize<'de> for SlowRequest {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let _ = serde_json::Value::deserialize(deserializer)?;
                thread::sleep(Duration::from_millis(25));
                Ok(Self)
            }
        }

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.write_all(b"{}\n").unwrap();
        let (mut stream, _) = listener.accept().unwrap();

        let result = read_message_until::<SlowRequest>(
            &mut stream,
            Instant::now() + Duration::from_millis(5),
        );

        assert!(result.is_err());
    }

    #[test]
    fn unauthenticated_deadline_starts_when_connection_is_accepted() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (server_stream, peer) = listener.accept().unwrap();
        let accepted_deadline = Instant::now() + Duration::from_millis(50);
        let token = "a".repeat(64);
        serde_json::to_writer(
            &mut client,
            &CommandRequest {
                token: token.clone(),
                command: AppCommand::ShowTransfers,
            },
        )
        .unwrap();
        client.write_all(b"\n").unwrap();
        thread::sleep(Duration::from_millis(75));
        let received = Mutex::new(Vec::new());
        handle_connection(
            server_stream,
            peer,
            accepted_deadline,
            &token,
            &Mutex::new(|command| received.lock().unwrap().push(command)),
            &AtomicBool::new(false),
        );

        assert!(received.lock().unwrap().is_empty());
    }

    #[test]
    fn callback_waiter_rechecks_stopping_after_serialization_lock() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (server_stream, peer) = listener.accept().unwrap();
        let token = "a".repeat(64);
        serde_json::to_writer(
            &mut client,
            &CommandRequest {
                token: token.clone(),
                command: AppCommand::ShowTransfers,
            },
        )
        .unwrap();
        client.write_all(b"\n").unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let callback_received = Arc::clone(&received);
        let callback = Arc::new(Mutex::new(move |command| {
            callback_received.lock().unwrap().push(command);
        }));
        let callback_guard = callback.lock().unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_callback = Arc::clone(&callback);
        let worker_stopping = Arc::clone(&stopping);
        let worker = thread::spawn(move || {
            handle_connection(
                server_stream,
                peer,
                Instant::now() + Duration::from_secs(1),
                &token,
                worker_callback.as_ref(),
                worker_stopping.as_ref(),
            );
        });
        thread::sleep(Duration::from_millis(50));
        stopping.store(true, Ordering::Release);
        drop(callback_guard);
        worker.join().unwrap();
        let response: CommandResponse = read_message(&mut client).unwrap();

        assert!(!response.ok);
        assert!(received.lock().unwrap().is_empty());
    }

    #[test]
    fn invalid_token_never_invokes_callback() {
        let temp = tempdir().unwrap();
        let state_path = temp.path().join("command.json");
        let received = Arc::new(Mutex::new(Vec::new()));
        let callback_received = Arc::clone(&received);
        let server =
            AppCommandServer::start(state_path.clone(), &TestPermissions, move |command| {
                callback_received.lock().unwrap().push(command);
            })
            .unwrap();
        let state: CommandState = read_json(&state_path).unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, state.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        serde_json::to_writer(
            &mut stream,
            &CommandRequest {
                token: "0".repeat(64),
                command: AppCommand::ShowTransfers,
            },
        )
        .unwrap();
        stream.write_all(b"\n").unwrap();
        let response: CommandResponse = read_message(&mut stream).unwrap();

        assert!(!response.ok);
        assert!(received.lock().unwrap().is_empty());
        drop(server);
    }

    #[test]
    fn server_drop_is_not_delayed_by_unauthenticated_client() {
        let temp = tempdir().unwrap();
        let state_path = temp.path().join("command.json");
        let server = AppCommandServer::start(state_path.clone(), &TestPermissions, |_| {}).unwrap();
        let state: CommandState = read_json(&state_path).unwrap();
        let mut slow = TcpStream::connect((Ipv4Addr::LOCALHOST, state.port)).unwrap();
        slow.write_all(b"{").unwrap();
        thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        drop(server);

        assert!(started.elapsed() < Duration::from_millis(250));
        assert!(!state_path.exists());
        drop(slow);
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
    fn token_comparison_accepts_only_equal_values() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"public"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn oversized_messages_are_rejected() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let writer = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .write_all(&vec![b'a'; MAX_MESSAGE_BYTES as usize + 1])
                .unwrap();
        });
        let (mut stream, _) = listener.accept().unwrap();

        let result = read_message::<serde_json::Value>(&mut stream);
        writer.join().unwrap();

        assert!(result.is_err());
    }

    #[test]
    fn message_at_exact_size_limit_is_accepted() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let payload_length = MAX_MESSAGE_BYTES as usize - 3;
        let writer = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(b"\"").unwrap();
            stream.write_all(&vec![b'a'; payload_length]).unwrap();
            stream.write_all(b"\"\n").unwrap();
        });
        let (mut stream, _) = listener.accept().unwrap();

        let value = read_message::<String>(&mut stream).unwrap();
        writer.join().unwrap();

        assert_eq!(value.len(), payload_length);
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
