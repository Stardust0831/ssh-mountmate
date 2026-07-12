use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};
use thiserror::Error;

use crate::paths::AppPaths;
use crate::process::{MountStatus, argv_matches_state};
use crate::rc::HttpRcClient;
use crate::rclone::MountCommand;
use crate::storage::{FileLock, StorageError, read_json, write_private_json};
use crate::{MountPhase, MountState, ServerConfig, Settings};

#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("mount state does not exist for {0}")]
    NotRecorded(String),
    #[error("{0} is already mounted or starting")]
    AlreadyMounted(String),
    #[error("invalid mountpoint {path}: {message}")]
    InvalidMountpoint { path: PathBuf, message: String },
    #[error("process operation failed: {0}")]
    Process(String),
    #[error("mount did not become ready; log: {log}\n{tail}")]
    NotReady { log: PathBuf, tail: String },
    #[error("recorded PID was reused; stale state was removed without stopping the process")]
    PidReused,
    #[error(
        "cannot verify that the recorded PID still belongs to this mount; no process was stopped"
    )]
    IdentityUnverified,
    #[error("mount process stopped but {0} is still mounted")]
    MountpointStillReady(PathBuf),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub arguments: Option<Vec<String>>,
    pub started_at: u64,
}

pub trait ProcessControl {
    fn spawn(&self, program: &Path, arguments: &[String], log: &Path) -> Result<u32, String>;
    fn snapshot(&self, pid: u32) -> Option<ProcessSnapshot>;
    fn signal_verified(
        &self,
        state: &MountState,
        windows: bool,
        force: bool,
    ) -> Result<bool, String>;
}

pub struct SystemProcessControl;

impl ProcessControl for SystemProcessControl {
    fn spawn(&self, program: &Path, arguments: &[String], log: &Path) -> Result<u32, String> {
        let parent = log.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .map_err(|error| error.to_string())?;
        let stderr = stdout.try_clone().map_err(|error| error.to_string())?;
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        #[cfg(windows)]
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().map_err(|error| error.to_string())?;
        Ok(child.id())
    }

    fn snapshot(&self, pid: u32) -> Option<ProcessSnapshot> {
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::Always)
                .with_exe(UpdateKind::Always),
        );
        let process = system.process(pid)?;
        Some(snapshot_from_process(process))
    }

    fn signal_verified(
        &self,
        state: &MountState,
        windows: bool,
        force: bool,
    ) -> Result<bool, String> {
        signal_verified_process(state, windows, force)
    }
}

fn snapshot_from_process(process: &sysinfo::Process) -> ProcessSnapshot {
    let arguments = (!process.cmd().is_empty()).then(|| {
        process
            .cmd()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    });
    ProcessSnapshot {
        arguments,
        started_at: process.start_time(),
    }
}

fn signal_verified_process(state: &MountState, windows: bool, force: bool) -> Result<bool, String> {
    let pid = Pid::from_u32(state.pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always),
    );
    let process = system
        .process(pid)
        .ok_or_else(|| format!("PID {pid} is not running"))?;
    let snapshot = snapshot_from_process(process);
    if !start_time_matches(state, &snapshot)
        || !snapshot
            .arguments
            .as_ref()
            .is_some_and(|arguments| argv_matches_state(arguments, state, windows))
    {
        return Ok(false);
    }
    let signal = if force { Signal::Kill } else { Signal::Term };
    match process.kill_with(signal) {
        Some(true) => Ok(true),
        Some(false) => Err(format!("PID {pid} rejected signal {signal:?}")),
        None => Err(format!("signal {signal:?} is unsupported on this platform")),
    }
}

pub trait RcControl {
    fn process_id(&self, address: &str) -> Result<u32, String>;
    fn quit(&self, address: &str) -> Result<(), String>;
}

pub struct HttpRcControl {
    timeout: Duration,
}

impl HttpRcControl {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl RcControl for HttpRcControl {
    fn process_id(&self, address: &str) -> Result<u32, String> {
        HttpRcClient::new(address, self.timeout)
            .and_then(|client| client.process_id())
            .map_err(|error| error.to_string())
    }

    fn quit(&self, address: &str) -> Result<(), String> {
        HttpRcClient::new(address, self.timeout)
            .and_then(|client| client.quit())
            .map_err(|error| error.to_string())
    }
}

pub trait MountpointControl {
    fn prepare(&self, path: &Path) -> Result<(), String>;
    fn is_ready(&self, path: &Path) -> bool;
}

pub struct SystemMountpointControl;

impl MountpointControl for SystemMountpointControl {
    fn prepare(&self, path: &Path) -> Result<(), String> {
        if path.as_os_str() == "*" {
            return Err("automatic '*' mountpoints must be resolved before spawning rclone".into());
        }
        #[cfg(windows)]
        {
            let value = path.as_os_str().to_string_lossy();
            if value.len() == 2 && value.as_bytes()[0].is_ascii_alphabetic() && value.ends_with(':')
            {
                return Ok(());
            }
            if path.exists() {
                return Err("Windows folder mount targets must not already exist".into());
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            fs::create_dir_all(path).map_err(|error| error.to_string())
        }
    }

    fn is_ready(&self, path: &Path) -> bool {
        system_mountpoint_ready(path)
    }
}

#[cfg(windows)]
fn system_mountpoint_ready(path: &Path) -> bool {
    path.exists()
}

#[cfg(target_os = "linux")]
fn system_mountpoint_ready(path: &Path) -> bool {
    let expected = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let Ok(mountinfo) = fs::read_to_string("/proc/self/mountinfo") else {
        return unix_device_changed(path);
    };
    mountinfo.lines().any(|line| {
        line.split_whitespace()
            .nth(4)
            .map(decode_mountinfo_path)
            .is_some_and(|candidate| candidate == expected || candidate == path)
    })
}

#[cfg(target_os = "linux")]
fn decode_mountinfo_path(value: &str) -> PathBuf {
    PathBuf::from(
        value
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\"),
    )
}

#[cfg(all(unix, not(target_os = "linux")))]
fn system_mountpoint_ready(path: &Path) -> bool {
    unix_device_changed(path)
}

#[cfg(unix)]
fn unix_device_changed(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Some(parent) = path.parent() else {
        return false;
    };
    match (path.metadata(), parent.metadata()) {
        (Ok(path_metadata), Ok(parent_metadata)) => path_metadata.dev() != parent_metadata.dev(),
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub ready_timeout: Duration,
    pub ready_stable_for: Duration,
    pub poll_interval: Duration,
    pub stop_timeout: Duration,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            ready_timeout: Duration::from_secs(20),
            ready_stable_for: Duration::from_millis(750),
            poll_interval: Duration::from_millis(250),
            stop_timeout: Duration::from_secs(3),
        }
    }
}

pub struct MountRequest<'a> {
    pub server: &'a ServerConfig,
    pub settings: &'a Settings,
    pub rclone: &'a Path,
    pub mountpoint: &'a Path,
    pub cache_dir: &'a Path,
}

pub struct MountRuntime<'a> {
    paths: &'a AppPaths,
    processes: &'a dyn ProcessControl,
    rc: &'a dyn RcControl,
    mountpoints: &'a dyn MountpointControl,
    options: RuntimeOptions,
    windows: bool,
}

impl<'a> MountRuntime<'a> {
    pub fn new(
        paths: &'a AppPaths,
        processes: &'a dyn ProcessControl,
        rc: &'a dyn RcControl,
        mountpoints: &'a dyn MountpointControl,
    ) -> Self {
        Self {
            paths,
            processes,
            rc,
            mountpoints,
            options: RuntimeOptions::default(),
            windows: cfg!(windows),
        }
    }

    pub fn with_options(mut self, options: RuntimeOptions) -> Self {
        self.options = options;
        self
    }

    pub fn mount(&self, request: MountRequest<'_>) -> Result<MountState, RuntimeError> {
        let server = request.server;
        let _lock =
            FileLock::acquire(&self.paths.mount_lock(&server.id), Duration::from_secs(180))?;
        if let Some(state) = self.load_state(&server.id)? {
            if matches!(
                self.status_for(&state),
                MountStatus::Mounted | MountStatus::Starting
            ) {
                return Err(RuntimeError::AlreadyMounted(server.display_name().into()));
            }
            self.remove_state(&server.id)?;
        }
        self.mountpoints
            .prepare(request.mountpoint)
            .map_err(|message| RuntimeError::InvalidMountpoint {
                path: request.mountpoint.to_owned(),
                message,
            })?;
        fs::create_dir_all(request.cache_dir).map_err(|source| StorageError::Io {
            path: request.cache_dir.to_owned(),
            source,
        })?;
        fs::create_dir_all(&self.paths.state_dir).map_err(|source| StorageError::Io {
            path: self.paths.state_dir.clone(),
            source,
        })?;

        let remote = server.remote_spec();
        let log = self.paths.mount_log(server.remote_name());
        let rc_addr = allocate_loopback_address()?;
        let command = MountCommand {
            rclone: request.rclone,
            config: &self.paths.rclone_config(),
            server,
            settings: request.settings,
            remote: &remote,
            mountpoint: request.mountpoint,
            cache_dir: request.cache_dir,
            log_path: &log,
            rc_addr: &rc_addr,
            windows: self.windows,
        }
        .build();
        let arguments = &command[1..];
        let pid = self
            .processes
            .spawn(request.rclone, arguments, &log)
            .map_err(RuntimeError::Process)?;
        let spawned_snapshot = self.processes.snapshot(pid);
        let process_started_at = spawned_snapshot
            .as_ref()
            .map(|snapshot| snapshot.started_at);
        let rclone = spawned_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.arguments.as_ref())
            .and_then(|arguments| arguments.first())
            .map(PathBuf::from)
            .unwrap_or_else(|| request.rclone.to_owned());
        let mut state = MountState {
            pid,
            server_id: server.id.clone(),
            remote,
            mountpoint: request.mountpoint.to_owned(),
            log,
            rc_addr,
            phase: MountPhase::Starting,
            process_started_at,
            rclone,
        };
        if let Err(error) = self.save_state(&state) {
            let _ = self.stop_if_owned(&state);
            return Err(error);
        }
        if !self.wait_until_ready(&state) {
            if self.stop_if_owned(&state) {
                self.remove_state(&server.id)?;
            }
            return Err(RuntimeError::NotReady {
                log: state.log.clone(),
                tail: log_tail(&state.log, 24),
            });
        }
        state.phase = MountPhase::Mounted;
        if let Err(error) = self.save_state(&state) {
            if self.stop_if_owned(&state) {
                let _ = self.remove_state(&server.id);
            }
            return Err(error);
        }
        Ok(state)
    }

    pub fn status(&self, server_id: &str) -> Result<MountStatus, RuntimeError> {
        Ok(self
            .load_state(server_id)?
            .as_ref()
            .map_or(MountStatus::Unmounted, |state| self.status_for(state)))
    }

    pub fn unmount(&self, server_id: &str) -> Result<(), RuntimeError> {
        let _lock = FileLock::acquire(&self.paths.mount_lock(server_id), Duration::from_secs(180))?;
        let state = self
            .load_state(server_id)?
            .ok_or_else(|| RuntimeError::NotRecorded(server_id.into()))?;
        let Some(snapshot) = self.processes.snapshot(state.pid) else {
            self.remove_state(server_id)?;
            return Ok(());
        };
        if !start_time_matches(&state, &snapshot) {
            self.remove_state(server_id)?;
            return Err(RuntimeError::PidReused);
        }
        let command_matches = snapshot
            .arguments
            .as_ref()
            .is_some_and(|arguments| argv_matches_state(arguments, &state, self.windows));
        let rc_matches = self.rc.process_id(&state.rc_addr) == Ok(state.pid);
        if !command_matches && !rc_matches {
            if snapshot.arguments.is_some() {
                self.remove_state(server_id)?;
                return Err(RuntimeError::PidReused);
            }
            return Err(RuntimeError::IdentityUnverified);
        }

        if rc_matches {
            let _ = self.rc.quit(&state.rc_addr);
        }
        self.wait_for_exit(&state, self.options.stop_timeout);
        if self.processes.snapshot(state.pid).is_some() {
            let _ = self.stop_if_owned(&state);
            self.wait_for_exit(&state, self.options.stop_timeout);
        }
        if self.processes.snapshot(state.pid).is_some() {
            return Err(RuntimeError::Process(format!(
                "PID {} did not exit",
                state.pid
            )));
        }
        let deadline = Instant::now() + self.options.stop_timeout;
        while self.mountpoints.is_ready(&state.mountpoint) && Instant::now() < deadline {
            std::thread::sleep(self.options.poll_interval);
        }
        if self.mountpoints.is_ready(&state.mountpoint) {
            return Err(RuntimeError::MountpointStillReady(state.mountpoint));
        }
        self.remove_state(server_id)?;
        Ok(())
    }

    pub fn cleanup_stale_state(&self, server_id: &str) -> Result<bool, RuntimeError> {
        let _lock = FileLock::acquire(&self.paths.mount_lock(server_id), Duration::from_secs(180))?;
        let Some(state) = self.load_state(server_id)? else {
            return Ok(false);
        };
        if self.status_for(&state) == MountStatus::Stale {
            self.remove_state(server_id)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn status_for(&self, state: &MountState) -> MountStatus {
        let Some(snapshot) = self.processes.snapshot(state.pid) else {
            return MountStatus::Stale;
        };
        if !start_time_matches(state, &snapshot) {
            return MountStatus::Stale;
        }
        let command = snapshot
            .arguments
            .as_ref()
            .map(|arguments| argv_matches_state(arguments, state, self.windows));
        if command == Some(false) {
            return MountStatus::Stale;
        }
        let rc_verified = self.rc.process_id(&state.rc_addr) == Ok(state.pid);
        let mountpoint_ready = self.mountpoints.is_ready(&state.mountpoint);
        if mountpoint_ready && (rc_verified || command == Some(true)) {
            MountStatus::Mounted
        } else if state.phase == MountPhase::Starting || command != Some(false) {
            MountStatus::Starting
        } else {
            MountStatus::Stale
        }
    }

    fn wait_until_ready(&self, state: &MountState) -> bool {
        let deadline = Instant::now() + self.options.ready_timeout;
        let mut ready_since = None;
        loop {
            let Some(snapshot) = self.processes.snapshot(state.pid) else {
                return false;
            };
            if !start_time_matches(state, &snapshot)
                || snapshot
                    .arguments
                    .as_ref()
                    .is_some_and(|arguments| !argv_matches_state(arguments, state, self.windows))
            {
                return false;
            }
            let ready = self.rc.process_id(&state.rc_addr) == Ok(state.pid)
                && self.mountpoints.is_ready(&state.mountpoint);
            if ready {
                let since = ready_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= self.options.ready_stable_for {
                    return true;
                }
            } else {
                ready_since = None;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(self.options.poll_interval);
        }
    }

    fn stop_if_owned(&self, state: &MountState) -> bool {
        let Some(snapshot) = self.processes.snapshot(state.pid) else {
            return true;
        };
        if !start_time_matches(state, &snapshot) {
            return false;
        }
        let command_matches = snapshot
            .arguments
            .as_ref()
            .is_some_and(|arguments| argv_matches_state(arguments, state, self.windows));
        let rc_matches = self.rc.process_id(&state.rc_addr) == Ok(state.pid);
        if !command_matches && !rc_matches {
            return false;
        }
        if rc_matches {
            let _ = self.rc.quit(&state.rc_addr);
        }
        let _ = self.processes.signal_verified(state, self.windows, false);
        self.wait_for_exit(state, Duration::from_millis(500));
        if let Some(snapshot) = self.processes.snapshot(state.pid)
            && start_time_matches(state, &snapshot)
            && snapshot
                .arguments
                .as_ref()
                .is_some_and(|arguments| argv_matches_state(arguments, state, self.windows))
        {
            let _ = self.processes.signal_verified(state, self.windows, true);
            self.wait_for_exit(state, Duration::from_millis(500));
        }
        self.processes.snapshot(state.pid).is_none()
    }

    fn wait_for_exit(&self, state: &MountState, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.processes.snapshot(state.pid).is_some() && Instant::now() < deadline {
            std::thread::sleep(self.options.poll_interval);
        }
    }

    fn load_state(&self, server_id: &str) -> Result<Option<MountState>, RuntimeError> {
        let path = self.paths.state_file(server_id);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(read_json(&path)?))
    }

    fn save_state(&self, state: &MountState) -> Result<(), RuntimeError> {
        write_private_json(&self.paths.state_file(&state.server_id), state)?;
        Ok(())
    }

    fn remove_state(&self, server_id: &str) -> Result<(), RuntimeError> {
        let path = self.paths.state_file(server_id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StorageError::Io { path, source }.into()),
        }
    }
}

fn start_time_matches(state: &MountState, snapshot: &ProcessSnapshot) -> bool {
    state
        .process_started_at
        .is_none_or(|expected| expected == snapshot.started_at)
}

fn allocate_loopback_address() -> Result<String, RuntimeError> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| RuntimeError::Process(error.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|error| RuntimeError::Process(error.to_string()))?
        .port();
    drop(listener);
    Ok(format!("127.0.0.1:{port}"))
}

fn log_tail(path: &Path, line_count: usize) -> String {
    if line_count == 0 {
        return String::new();
    }
    let Ok(file) = File::open(path) else {
        return String::new();
    };
    let mut reader = BufReader::new(file);
    let mut lines = VecDeque::with_capacity(line_count);
    let mut bytes = Vec::new();
    while reader.read_until(b'\n', &mut bytes).unwrap_or(0) > 0 {
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        if lines.len() == line_count {
            lines.pop_front();
        }
        lines.push_back(String::from_utf8_lossy(&bytes).into_owned());
        bytes.clear();
    }
    lines.into_iter().collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use tempfile::tempdir;

    use super::*;

    struct FakeProcesses {
        snapshots: RefCell<VecDeque<Option<ProcessSnapshot>>>,
        last: RefCell<Option<ProcessSnapshot>>,
        signals: RefCell<Vec<&'static str>>,
    }

    impl FakeProcesses {
        fn new(snapshots: impl IntoIterator<Item = Option<ProcessSnapshot>>) -> Self {
            Self {
                snapshots: RefCell::new(snapshots.into_iter().collect()),
                last: RefCell::new(None),
                signals: RefCell::new(Vec::new()),
            }
        }
    }

    impl ProcessControl for FakeProcesses {
        fn spawn(
            &self,
            _program: &Path,
            _arguments: &[String],
            _log: &Path,
        ) -> Result<u32, String> {
            Ok(42)
        }

        fn snapshot(&self, _pid: u32) -> Option<ProcessSnapshot> {
            if let Some(snapshot) = self.snapshots.borrow_mut().pop_front() {
                *self.last.borrow_mut() = snapshot.clone();
                snapshot
            } else {
                self.last.borrow().clone()
            }
        }

        fn signal_verified(
            &self,
            state: &MountState,
            windows: bool,
            force: bool,
        ) -> Result<bool, String> {
            let verified = self.last.borrow().as_ref().is_some_and(|snapshot| {
                start_time_matches(state, snapshot)
                    && snapshot
                        .arguments
                        .as_ref()
                        .is_some_and(|arguments| argv_matches_state(arguments, state, windows))
            });
            if !verified {
                return Ok(false);
            }
            self.signals
                .borrow_mut()
                .push(if force { "kill" } else { "term" });
            *self.last.borrow_mut() = None;
            self.snapshots.borrow_mut().clear();
            Ok(true)
        }
    }

    struct FakeRc {
        pid: Result<u32, String>,
        quit_calls: RefCell<usize>,
    }

    impl RcControl for FakeRc {
        fn process_id(&self, _address: &str) -> Result<u32, String> {
            self.pid.clone()
        }

        fn quit(&self, _address: &str) -> Result<(), String> {
            *self.quit_calls.borrow_mut() += 1;
            Ok(())
        }
    }

    struct FakeMountpoint {
        ready: RefCell<bool>,
    }

    impl MountpointControl for FakeMountpoint {
        fn prepare(&self, _path: &Path) -> Result<(), String> {
            Ok(())
        }

        fn is_ready(&self, _path: &Path) -> bool {
            *self.ready.borrow()
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

    fn server() -> ServerConfig {
        ServerConfig {
            id: "alpha".into(),
            name: "Alpha".into(),
            ..ServerConfig::default()
        }
    }

    fn state(root: &Path, arguments: Option<Vec<String>>) -> (MountState, ProcessSnapshot) {
        let state = MountState {
            pid: 42,
            server_id: "alpha".into(),
            remote: "alpha:".into(),
            mountpoint: root.join("mnt"),
            log: root.join("state/alpha.log"),
            rc_addr: "127.0.0.1:1234".into(),
            phase: MountPhase::Mounted,
            process_started_at: Some(100),
            rclone: PathBuf::from("rclone"),
        };
        (
            state,
            ProcessSnapshot {
                arguments,
                started_at: 100,
            },
        )
    }

    fn matching_arguments(root: &Path) -> Vec<String> {
        vec![
            "rclone".into(),
            "--rc".into(),
            "mount".into(),
            "alpha:".into(),
            root.join("mnt").display().to_string(),
            "--log-file".into(),
            root.join("state/alpha.log").display().to_string(),
        ]
    }

    fn options() -> RuntimeOptions {
        RuntimeOptions {
            ready_timeout: Duration::from_millis(1),
            ready_stable_for: Duration::ZERO,
            poll_interval: Duration::from_millis(1),
            stop_timeout: Duration::from_millis(1),
        }
    }

    #[test]
    fn mount_persists_starting_then_mounted_state() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let args = matching_arguments(temp.path());
        let (_, snapshot) = state(temp.path(), Some(args));
        let processes = FakeProcesses::new([Some(snapshot.clone()), Some(snapshot)]);
        let rc = FakeRc {
            pid: Ok(42),
            quit_calls: RefCell::new(0),
        };
        let mountpoint = FakeMountpoint {
            ready: RefCell::new(true),
        };
        let runtime =
            MountRuntime::new(&paths, &processes, &rc, &mountpoint).with_options(options());

        let mounted = runtime
            .mount(MountRequest {
                server: &server(),
                settings: &Settings::default(),
                rclone: Path::new("rclone"),
                mountpoint: &temp.path().join("mnt"),
                cache_dir: &temp.path().join("cache"),
            })
            .unwrap();

        assert_eq!(mounted.phase, MountPhase::Mounted);
        let saved: MountState = read_json(&paths.state_file("alpha")).unwrap();
        assert_eq!(saved.phase, MountPhase::Mounted);
        assert!(processes.signals.borrow().is_empty());
    }

    #[test]
    fn pid_reuse_is_never_terminated_during_unmount() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let (state, snapshot) = state(
            temp.path(),
            Some(vec!["unrelated".into(), "--serve".into()]),
        );
        write_private_json(&paths.state_file("alpha"), &state).unwrap();
        let processes = FakeProcesses::new([Some(snapshot)]);
        let rc = FakeRc {
            pid: Err("offline".into()),
            quit_calls: RefCell::new(0),
        };
        let mountpoint = FakeMountpoint {
            ready: RefCell::new(false),
        };
        let runtime =
            MountRuntime::new(&paths, &processes, &rc, &mountpoint).with_options(options());

        assert!(matches!(
            runtime.unmount("alpha"),
            Err(RuntimeError::PidReused)
        ));
        assert!(processes.signals.borrow().is_empty());
        assert!(!paths.state_file("alpha").exists());
    }

    #[test]
    fn unverifiable_identity_keeps_state_and_process() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let (state, snapshot) = state(temp.path(), None);
        write_private_json(&paths.state_file("alpha"), &state).unwrap();
        let processes = FakeProcesses::new([Some(snapshot)]);
        let rc = FakeRc {
            pid: Err("offline".into()),
            quit_calls: RefCell::new(0),
        };
        let mountpoint = FakeMountpoint {
            ready: RefCell::new(true),
        };
        let runtime =
            MountRuntime::new(&paths, &processes, &rc, &mountpoint).with_options(options());

        assert!(matches!(
            runtime.unmount("alpha"),
            Err(RuntimeError::IdentityUnverified)
        ));
        assert!(paths.state_file("alpha").exists());
        assert!(processes.signals.borrow().is_empty());
    }

    #[test]
    fn verified_rc_quits_without_force_killing() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let (state, snapshot) = state(temp.path(), Some(matching_arguments(temp.path())));
        write_private_json(&paths.state_file("alpha"), &state).unwrap();
        let processes = FakeProcesses::new([Some(snapshot), None]);
        let rc = FakeRc {
            pid: Ok(42),
            quit_calls: RefCell::new(0),
        };
        let mountpoint = FakeMountpoint {
            ready: RefCell::new(false),
        };
        let runtime =
            MountRuntime::new(&paths, &processes, &rc, &mountpoint).with_options(options());

        runtime.unmount("alpha").unwrap();

        assert_eq!(*rc.quit_calls.borrow(), 1);
        assert!(processes.signals.borrow().is_empty());
        assert!(!paths.state_file("alpha").exists());
    }

    #[test]
    fn readiness_timeout_stops_only_the_owned_process_and_removes_state() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let (_, snapshot) = state(temp.path(), Some(matching_arguments(temp.path())));
        let processes = FakeProcesses::new([Some(snapshot.clone()), Some(snapshot)]);
        let rc = FakeRc {
            pid: Err("offline".into()),
            quit_calls: RefCell::new(0),
        };
        let mountpoint = FakeMountpoint {
            ready: RefCell::new(false),
        };
        let runtime =
            MountRuntime::new(&paths, &processes, &rc, &mountpoint).with_options(options());

        assert!(matches!(
            runtime.mount(MountRequest {
                server: &server(),
                settings: &Settings::default(),
                rclone: Path::new("rclone"),
                mountpoint: &temp.path().join("mnt"),
                cache_dir: &temp.path().join("cache"),
            }),
            Err(RuntimeError::NotReady { .. })
        ));
        assert_eq!(&*processes.signals.borrow(), &["term"]);
        assert!(!paths.state_file("alpha").exists());
    }

    #[test]
    fn readiness_failure_preserves_state_when_pid_identity_changed() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let (_, spawned) = state(temp.path(), Some(matching_arguments(temp.path())));
        let reused = ProcessSnapshot {
            arguments: Some(vec!["unrelated".into(), "--serve".into()]),
            started_at: spawned.started_at,
        };
        let processes = FakeProcesses::new([Some(spawned), Some(reused)]);
        let rc = FakeRc {
            pid: Err("offline".into()),
            quit_calls: RefCell::new(0),
        };
        let mountpoint = FakeMountpoint {
            ready: RefCell::new(false),
        };
        let runtime =
            MountRuntime::new(&paths, &processes, &rc, &mountpoint).with_options(options());

        assert!(matches!(
            runtime.mount(MountRequest {
                server: &server(),
                settings: &Settings::default(),
                rclone: Path::new("rclone"),
                mountpoint: &temp.path().join("mnt"),
                cache_dir: &temp.path().join("cache"),
            }),
            Err(RuntimeError::NotReady { .. })
        ));
        assert!(paths.state_file("alpha").exists());
        assert!(processes.signals.borrow().is_empty());
    }

    #[test]
    fn start_time_mismatch_never_signals_the_reused_pid() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let (state, mut snapshot) = state(temp.path(), Some(matching_arguments(temp.path())));
        snapshot.started_at = 101;
        write_private_json(&paths.state_file("alpha"), &state).unwrap();
        let processes = FakeProcesses::new([Some(snapshot)]);
        let rc = FakeRc {
            pid: Ok(42),
            quit_calls: RefCell::new(0),
        };
        let mountpoint = FakeMountpoint {
            ready: RefCell::new(true),
        };
        let runtime =
            MountRuntime::new(&paths, &processes, &rc, &mountpoint).with_options(options());

        assert!(matches!(
            runtime.unmount("alpha"),
            Err(RuntimeError::PidReused)
        ));
        assert!(processes.signals.borrow().is_empty());
        assert_eq!(*rc.quit_calls.borrow(), 0);
    }

    #[test]
    fn live_unverifiable_state_is_not_cleaned_as_stale() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let (state, snapshot) = state(temp.path(), None);
        write_private_json(&paths.state_file("alpha"), &state).unwrap();
        let processes = FakeProcesses::new([Some(snapshot)]);
        let rc = FakeRc {
            pid: Err("offline".into()),
            quit_calls: RefCell::new(0),
        };
        let mountpoint = FakeMountpoint {
            ready: RefCell::new(false),
        };
        let runtime =
            MountRuntime::new(&paths, &processes, &rc, &mountpoint).with_options(options());

        assert!(!runtime.cleanup_stale_state("alpha").unwrap());
        assert!(paths.state_file("alpha").exists());
        assert_eq!(runtime.status("alpha").unwrap(), MountStatus::Starting);
    }

    #[test]
    fn log_tail_returns_only_the_requested_final_lines() {
        let temp = tempdir().unwrap();
        let log = temp.path().join("mount.log");
        fs::write(&log, "one\ntwo\nthree\nfour\n").unwrap();

        assert_eq!(log_tail(&log, 2), "three\nfour");
        assert_eq!(log_tail(&log, 0), "");
    }
}
