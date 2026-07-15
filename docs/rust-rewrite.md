# Rust rewrite acceptance contract

The `rust-rewrite` branch replaces the Python/PyInstaller application with one Rust application. Python is not a runtime, packaging, or fallback dependency in the completed rewrite. Rclone remains the mount engine because its VFS and SFTP behavior are the product's proven filesystem boundary.

## Product requirements

- Read and migrate the existing `rsshmount/servers.json`, `settings.json`, rclone config, mount-state files, cache, and logs without losing secrets or custom settings.
- Preserve manual, SSH config, batch SSH config, and SAI connection creation; key, password, passphrase, native SFTP, and OpenSSH transports.
- Preserve automatic drive letters, user mount folders, custom mountpoints, duplicate detection, local capacity, Lustre project capacity, dependency checks, login mounting, and concurrent batch operations.
- Preserve truthful rclone RC transfer state from `vfs/queue`, `vfs/stats`, and `core/stats`; never present a queued local write as remotely complete.
- Preserve verified refresh order: `vfs/forget`, `vfs/refresh`, `operations/list`, then a post-refresh `vfs/queue` snapshot, including the Windows drive-root quote repair.
- Preserve safe process ownership checks before status reporting or termination. PID reuse must never terminate an unrelated process.
- Preserve verified GitHub updates with platform/architecture selection, SHA-256 and size verification, safe archive extraction, staged replacement, startup health checks, and rollback.
- Provide a responsive bilingual GUI, one shared movable transfer window that expands across active connections, a complete transfer center, tray controls, native notifications, taskbar or dock progress where supported, and file-manager integration.
- Build native packages for Windows, macOS, and Linux on x64 and arm64. No PyInstaller onefile extraction or Python interpreter is shipped.

## Historical regressions that must remain fixed

- Unreadable user `known_hosts` files are never passed to rclone; first-seen host keys are pinned and not silently replaced.
- Windows ACLs on managed SSH config and copied private keys allow the current user and SYSTEM while removing unsafe inherited access.
- Host-key mismatch fallback is explicit and visible; symlinks always use rclone `--links`.
- Mount readiness is polled through RC/process/mountpoint evidence instead of a fixed sleep.
- Parent log handles close immediately after spawning rclone.
- Windows status checks use RC and native process APIs, not routine PowerShell CIM scans.
- Folder mount targets follow platform rules and do not accidentally pre-create forbidden Windows targets.
- Cache defaults remain `full`, `30m`, `5s`, and `5m`; custom values survive migrations.
- Upload warnings distinguish queued, active, errored, unknown, and remotely confirmed empty states.
- GUI restart or self-update leaves rclone mounts and uploads running.

## Verification gates

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
3. `cargo test --workspace --all-features`
4. Six-platform GitHub Actions builds with packaged smoke tests.
5. Windows Explorer, ACL, one-instance activation, Toast, tray, taskbar progress, mount, refresh, upload, update, and rollback integration tests.
6. macOS app bundle, signing-ready layout, notifications, menu bar, Finder integration, mount, update, and rollback integration tests.
7. Linux X11/Wayland startup, notifications, tray capability reporting, file-manager integration, mount, update, and rollback integration tests.
