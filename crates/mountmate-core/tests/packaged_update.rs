use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use mountmate_core::LEGACY_APP_ID;
use mountmate_core::model::Settings;
use mountmate_core::paths::AppPaths;
use mountmate_core::rclone_binary::file_sha256;
use mountmate_core::storage::save_settings;
use mountmate_core::update_helper::{
    ParentProcessIdentity, materialize_update_helper, write_update_plan,
};
use mountmate_core::update_install::{
    detect_install_layout, locate_directory_payload, plan_transaction_paths,
    prepare_directory_payload,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tempfile::TempDir;
use wait_timeout::ChildExt;

const PACKAGE_ROOT_ENV: &str = "SSH_MOUNTMATE_PACKAGE_ROOT";
const VERSION_MARKER: &str = "SSHMountMate.update-e2e-version";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(30);
const HELPER_TIMEOUT: Duration = Duration::from_secs(75);

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
#[ignore = "requires a packaged SSH MountMate bundle and a graphical session"]
fn packaged_update_commits_after_real_gui_health_confirmation() {
    run_scenario(Scenario::Commit).unwrap();
}

#[test]
#[ignore = "requires a packaged SSH MountMate bundle and a graphical session"]
fn packaged_update_rolls_back_when_new_gui_cannot_report_healthy() {
    run_scenario(Scenario::Rollback).unwrap();
}

#[derive(Clone, Copy)]
enum Scenario {
    Commit,
    Rollback,
}

impl Scenario {
    fn relaunch_arguments(self) -> Vec<String> {
        match self {
            Self::Commit => vec!["--show-main".into()],
            Self::Rollback => vec!["--version".into()],
        }
    }

    fn expected_marker(self) -> &'static str {
        match self {
            Self::Commit => "new",
            Self::Rollback => "old",
        }
    }
}

fn run_scenario(scenario: Scenario) -> TestResult {
    let package_root = env::var_os(PACKAGE_ROOT_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other(format!("{PACKAGE_ROOT_ENV} is not set")))?
        .canonicalize()?;
    let temporary = test_directory()?;
    let install_root = temporary.path().join(package_name());
    let payload_root = temporary.path().join(format!("payload-{}", package_name()));
    copy_tree(&package_root, &install_root)?;
    copy_tree(&package_root, &payload_root)?;
    fs::write(version_marker(&install_root), b"old\n")?;
    fs::write(version_marker(&payload_root), b"new\n")?;
    resign_macos_bundle(&install_root)?;
    resign_macos_bundle(&payload_root)?;

    let environment = TestEnvironment::new(temporary.path())?;
    let settings = Settings {
        auto_check_updates: false,
        auto_show_transfers: false,
        ..Settings::default()
    };
    save_settings(&environment.paths, &settings)?;

    let installed_executable = package_executable(&install_root);
    let layout = detect_install_layout(&installed_executable)?;
    let transaction = plan_transaction_paths(&layout)?;
    let payload = locate_directory_payload(&payload_root, env::consts::OS)?;
    let prepared = prepare_directory_payload(&layout, &payload, &transaction, env::consts::OS)?;
    let helper = materialize_update_helper(
        &temporary.path().join("detached-updater"),
        &installed_executable,
    )?;

    let parent_stdout = temporary.path().join("parent.stdout");
    let parent_stderr = temporary.path().join("parent.stderr");
    let mut parent = spawn_logged(
        &installed_executable,
        &["--show-main"],
        &environment,
        &parent_stdout,
        &parent_stderr,
    )?;

    let result = (|| {
        let parent_identity = wait_for_parent_identity(
            &mut parent,
            &layout.executable,
            &environment.paths.app_command_state(),
            &parent_stderr,
        )?;
        let authorization = write_update_plan(
            &environment.paths.update_state_dir(),
            parent_identity,
            layout,
            prepared,
            transaction.clone(),
            scenario.relaunch_arguments(),
        )?;

        let helper_stdout = temporary.path().join("helper.stdout");
        let helper_stderr = temporary.path().join("helper.stderr");
        let helper_arguments = vec![
            OsString::from("--run-update-helper"),
            authorization.plan_path.as_os_str().to_owned(),
            OsString::from("--update-helper-token"),
            OsString::from(&authorization.token),
        ];
        parent.kill()?;
        parent.wait()?;
        thread::sleep(Duration::from_secs(1));
        let mut updater = spawn_logged(
            &helper,
            &helper_arguments,
            &environment,
            &helper_stdout,
            &helper_stderr,
        )?;
        let status = match updater.wait_timeout(HELPER_TIMEOUT)? {
            Some(status) => status,
            None => {
                updater.kill()?;
                updater.wait()?;
                return Err(io::Error::other(format!(
                    "update helper timed out\n{}",
                    read_diagnostic(&helper_stderr)
                ))
                .into());
            }
        };
        let helper_error = read_diagnostic(&helper_stderr);
        match scenario {
            Scenario::Commit if !status.success() => {
                return Err(io::Error::other(format!(
                    "healthy packaged update failed with {status}\n{helper_error}"
                ))
                .into());
            }
            Scenario::Rollback if status.success() => {
                return Err(
                    io::Error::other("unhealthy packaged update unexpectedly committed").into(),
                );
            }
            Scenario::Rollback if !helper_error.contains("the previous version was restored") => {
                return Err(io::Error::other(format!(
                    "update failed without proving rollback\n{helper_error}"
                ))
                .into());
            }
            _ => {}
        }

        let installed_marker = fs::read_to_string(version_marker(&install_root))?;
        if installed_marker.trim() != scenario.expected_marker() {
            return Err(io::Error::other(format!(
                "installed bundle marker is {:?}, expected {:?}",
                installed_marker.trim(),
                scenario.expected_marker()
            ))
            .into());
        }
        if transaction.backup.exists() || transaction.prepared.exists() {
            return Err(io::Error::other(
                "update transaction left a prepared payload or backup behind",
            )
            .into());
        }

        if matches!(scenario, Scenario::Commit)
            && !terminate_processes_at(&installed_executable, PROCESS_TIMEOUT)?
        {
            return Err(io::Error::other(
                "updated GUI was not still running after health confirmation",
            )
            .into());
        }
        Ok(())
    })();

    let _ = parent.kill();
    let _ = parent.wait();
    let _ = terminate_processes_at(&installed_executable, Duration::from_secs(5));
    result
}

fn test_directory() -> TestResult<TempDir> {
    let target = env::current_dir()?.join("target");
    fs::create_dir_all(&target)?;
    Ok(tempfile::Builder::new()
        .prefix("packaged-update-e2e-")
        .tempdir_in(target)?)
}

fn package_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "SSH MountMate.app"
    } else {
        "SSHMountMate"
    }
}

fn package_executable(root: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        root.join("Contents/MacOS/SSHMountMate")
    } else if cfg!(windows) {
        root.join("SSHMountMate.exe")
    } else {
        root.join("SSHMountMate")
    }
}

fn version_marker(root: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        root.join("Contents/Resources").join(VERSION_MARKER)
    } else {
        root.join(VERSION_MARKER)
    }
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::other(format!(
            "packaged update test rejects symlinks: {}",
            source.display()
        )));
    }
    if metadata.is_file() {
        fs::copy(source, destination)?;
        fs::set_permissions(destination, metadata.permissions())?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(io::Error::other(format!(
            "packaged update test rejects special files: {}",
            source.display()
        )));
    }
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
    }
    fs::set_permissions(destination, metadata.permissions())
}

#[cfg(target_os = "macos")]
fn resign_macos_bundle(root: &Path) -> TestResult {
    let status = Command::new("codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(root)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("codesign failed with {status}")).into())
    }
}

#[cfg(not(target_os = "macos"))]
fn resign_macos_bundle(_root: &Path) -> TestResult {
    Ok(())
}

struct TestEnvironment {
    values: Vec<(OsString, OsString)>,
    paths: AppPaths,
}

impl TestEnvironment {
    fn new(root: &Path) -> io::Result<Self> {
        let home = root.join("home");
        fs::create_dir_all(&home)?;
        let mut values = vec![
            (OsString::from("HOME"), home.as_os_str().to_owned()),
            (
                OsString::from("SSH_MOUNTMATE_E2E_INHERIT_UPDATE_STDERR"),
                OsString::from("1"),
            ),
            (OsString::from("RUST_BACKTRACE"), OsString::from("1")),
        ];

        #[cfg(windows)]
        let paths = {
            let roaming = root.join("roaming");
            let local = root.join("local");
            values.extend([
                (OsString::from("USERPROFILE"), home.as_os_str().to_owned()),
                (OsString::from("APPDATA"), roaming.as_os_str().to_owned()),
                (OsString::from("LOCALAPPDATA"), local.as_os_str().to_owned()),
            ]);
            AppPaths {
                config_dir: roaming.join(LEGACY_APP_ID),
                cache_dir: local.join(LEGACY_APP_ID).join("Cache"),
                state_dir: local.join(LEGACY_APP_ID).join("State"),
                data_dir: local.join("ssh-mountmate"),
            }
        };

        #[cfg(target_os = "macos")]
        let paths = {
            let config = root.join("config");
            let cache = root.join("cache");
            let state = root.join("state");
            values.extend(xdg_values(root));
            AppPaths {
                config_dir: config.join(LEGACY_APP_ID),
                cache_dir: cache.join(LEGACY_APP_ID),
                state_dir: state.join(LEGACY_APP_ID),
                data_dir: home.join("Library/Application Support/ssh-mountmate"),
            }
        };

        #[cfg(all(unix, not(target_os = "macos")))]
        let paths = {
            let config = root.join("config");
            let cache = root.join("cache");
            let state = root.join("state");
            let data = root.join("data");
            values.extend(xdg_values(root));
            values.push((
                OsString::from("XDG_RUNTIME_DIR"),
                root.join("runtime").into_os_string(),
            ));
            fs::create_dir_all(root.join("runtime"))?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join("runtime"), fs::Permissions::from_mode(0o700))?;
            AppPaths {
                config_dir: config.join(LEGACY_APP_ID),
                cache_dir: cache.join(LEGACY_APP_ID),
                state_dir: state.join(LEGACY_APP_ID),
                data_dir: data.join("ssh-mountmate"),
            }
        };

        Ok(Self { values, paths })
    }

    fn apply(&self, command: &mut Command) {
        command.envs(self.values.iter().cloned());
        #[cfg(all(unix, not(target_os = "macos")))]
        command
            .env("WINIT_UNIX_BACKEND", "x11")
            .env("WGPU_BACKEND", "gl")
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .env("NO_AT_BRIDGE", "1")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET");
    }
}

#[cfg(unix)]
fn xdg_values(root: &Path) -> [(OsString, OsString); 4] {
    [
        (
            OsString::from("XDG_CONFIG_HOME"),
            root.join("config").into_os_string(),
        ),
        (
            OsString::from("XDG_CACHE_HOME"),
            root.join("cache").into_os_string(),
        ),
        (
            OsString::from("XDG_STATE_HOME"),
            root.join("state").into_os_string(),
        ),
        (
            OsString::from("XDG_DATA_HOME"),
            root.join("data").into_os_string(),
        ),
    ]
}

fn spawn_logged<A: AsRef<OsStr>>(
    executable: &Path,
    arguments: &[A],
    environment: &TestEnvironment,
    stdout_path: &Path,
    stderr_path: &Path,
) -> io::Result<Child> {
    let stdout = File::create(stdout_path)?;
    let stderr = File::create(stderr_path)?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    environment.apply(&mut command);
    command.spawn()
}

fn wait_for_parent_identity(
    child: &mut Child,
    expected_executable: &Path,
    command_state: &Path,
    stderr_path: &Path,
) -> TestResult<ParentProcessIdentity> {
    let expected_executable = expected_executable.canonicalize()?;
    let pid = Pid::from_u32(child.id());
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    let mut system = System::new();
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "packaged GUI exited before initialization with {status}\n{}",
                read_diagnostic(stderr_path)
            ))
            .into());
        }
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        if command_state.exists()
            && let Some(process) = system.process(pid)
            && let Some(executable) = process.exe().and_then(|path| path.canonicalize().ok())
            && executable == expected_executable
        {
            return Ok(ParentProcessIdentity {
                pid: child.id(),
                started_at: process.start_time(),
                executable: expected_executable.clone(),
                executable_sha256: file_sha256(&expected_executable)?,
            });
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "packaged GUI did not initialize before timeout\n{}",
                read_diagnostic(stderr_path)
            ))
            .into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn terminate_processes_at(executable: &Path, timeout: Duration) -> io::Result<bool> {
    let expected = executable.canonicalize()?;
    let deadline = Instant::now() + timeout;
    let mut found = false;
    loop {
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        let matching: Vec<_> = system
            .processes()
            .values()
            .filter(|process| {
                process
                    .exe()
                    .and_then(|path| path.canonicalize().ok())
                    .is_some_and(|path| path == expected)
            })
            .collect();
        if matching.is_empty() {
            return Ok(found);
        }
        found = true;
        for process in matching {
            process.kill();
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "could not terminate updated GUI at {}",
                expected.display()
            )));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn read_diagnostic(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| format!("could not read diagnostic: {error}"))
}
