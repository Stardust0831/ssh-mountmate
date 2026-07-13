# Rust rewrite completion audit

This document maps every requirement in `docs/rust-rewrite.md` to current authoritative evidence. A green build alone does not close an item whose user-visible behavior is broader than the exercised path.

## Authoritative workflow evidence

- [29232443979](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29232443979): Windows/Linux real SFTP mount, refresh, queued write-back, remote digest, unmount, packaged update, and rollback on x64/ARM64.
- [29236712312](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29236712312): Windows x64/ARM64 one-instance IPC, tray initialization, taskbar COM, close-to-tray, and main-window restoration.
- [29238672993](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29238672993): Windows x64/ARM64 native Toast submission and active taskbar progress.
- [29242454625](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29242454625): Linux X11 real notification/tray host and Wayland notification-protocol/tray-capability integration; six-platform build and packaging.
- [29244592603](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29244592603): macOS x64/ARM64 application-bundle startup, notification submission, menu-bar initialization, Dock progress, IPC, update, and rollback.
- [29246310691](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29246310691): macOS x64/ARM64 real SFTP mount, verified refresh, queued 8 MiB write-back, remote completion, digest, unmount, and state cleanup through rclone's supported FUSE-T mount layer.
- [29245426903](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29245426903): non-publishing release workflow dry run; release quality gate plus six complete platform packages and exactly six artifacts.
- [29247208972](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29247208972): pending non-publishing release workflow dry run including macOS real-mount gates.

## Product requirements

| Requirement | Status | Evidence and remaining work |
| --- | --- | --- |
| Existing server/settings/rclone/state/cache/log compatibility without losing secrets or custom settings | Verified | The fixture-driven `legacy_migration` integration test loads three legacy Python server shapes, obscured password and key-passphrase secrets, settings, mount state, cache/log data, and a mixed rclone config together; it then rewrites and reloads the data while proving custom settings, unrelated remotes/secrets, and legacy cache/log files remain intact. |
| Manual, SSH config, batch SSH config, SAI, password/key/passphrase, native SFTP, and OpenSSH creation paths | Partially verified | Parsing, import planning, duplicate protection, secret preservation, proxy/OpenSSH selection, and generated rclone remotes have unit coverage. Native SFTP password paths are exercised by real mounts. GUI-level creation flows and a real OpenSSH transport mount still need integration evidence. |
| Drive/folder allocation, duplicate detection, capacity, dependency checks, login mounts, and concurrent batches | Partially verified | Allocation, duplicates, local/Lustre capacity parsing, dependency discovery, startup registration, and batch implementation have unit or platform registration coverage. Login-triggered mounting and concurrent multi-mount behavior still need end-to-end evidence. |
| Truthful rclone RC transfer state | Verified | Unit tests cover queued, active, errored, unknown, exhausted-cache, and remote-byte states. Windows/Linux/macOS real mounts prove queued writes are not presented as complete and later become remotely complete. |
| Refresh order and Windows root quote repair | Verified | RC contract tests prove queue/forget/refresh/list order and reject the legacy quote remote; real mount tests prove remotely created content appears after refresh. |
| Safe process ownership and PID-reuse behavior | Verified | Process/runtime tests cover exact argv identity, start-time mismatch, unverifiable ownership, safe RC quit, stale state, and never terminating a reused PID. |
| Verified update, extraction, staged replacement, health, and rollback | Verified | Unit tests cover URL/digest/size trust, redirect restrictions, archive safety, transaction containment, authenticated health, and rollback. Packaged update/rollback runs on all six targets. |
| Responsive bilingual GUI, per-connection popup, transfer center, tray/menu bar, notifications, global progress, and file-manager integration | Partially verified | Native tray/menu bar, notifications, Windows taskbar, macOS Dock, X11/Wayland capability reporting, authenticated IPC, and file-manager registration are exercised. Transfer-state calculations and popup layout have unit coverage. A user-level interaction test for simultaneous per-connection popups and transfer-center behavior remains. |
| Native x64/ARM64 packages without Python | Verified for portable packages; installer review open | Six native ZIP packages contain the Rust executable, verified rclone, and notices with no Python runtime. Native installer formats, icons, signing/notarization inputs, and installed-update behavior remain under final distribution review. |

## Historical regressions

| Regression | Status | Evidence |
| --- | --- | --- |
| Unreadable `known_hosts`; first-seen pinning; mismatch visibility | Partially verified | SSH and rclone unit tests cover unreadable-file exclusion, managed first-seen keys, idempotence, and no silent replacement. A real changed-host-key path still needs to prove that the user receives an explicit mismatch instead of only a lower-level rclone failure. |
| Windows ACLs for SSH config and copied keys | Verified | Windows ACL implementation and native Windows tests cover protected current-user/SYSTEM access. |
| Explicit host-key mismatch and `--links` | Partially verified | Rclone command tests prove `--links`. The changed-host-key user-facing path remains part of the explicit mismatch integration blocker above. |
| Readiness polling instead of fixed sleep | Verified | Runtime tests and all real mount workflows require RC/process/mountpoint readiness. |
| Parent log handles close after spawn | Verified | Process-spawn implementation plus persistent Windows mount lifecycle evidence. |
| Windows status avoids routine PowerShell CIM | Verified | Native process APIs and RC are used; Windows integration tests exercise status and ownership. |
| Platform-correct folder mount targets | Verified | Mountpoint allocator tests cover Windows missing-target semantics and Unix folder behavior; real mounts cover all three operating systems. |
| Cache defaults and migration preservation | Verified | Model migration and rclone command tests cover `full`, `30m`, `0s`, `5m`, and custom values. |
| Upload warnings distinguish truthful states | Verified | Transfer and UI-state unit tests plus real queued-write workflows. |
| GUI restart/update leaves mounts and uploads running | Partially verified | Packaged update tests use real GUI health handoff and rclone process ownership is independent. A packaged update performed while a real mount has an active queued upload is still required. |

## Verification gates

| Gate | Status |
| --- | --- |
| `cargo fmt --all --check` | Verified in rewrite and release quality jobs. |
| Zero-warning workspace Clippy | Verified in rewrite and release quality jobs. |
| Complete workspace tests | Verified in rewrite and release quality jobs and on all six build targets. |
| Six-platform packages and smoke tests | Verified; latest release-grade rerun pending at 29247208972. |
| Windows Explorer/ACL/IPC/Toast/tray/taskbar/mount/refresh/upload/update/rollback | Verified. |
| macOS bundle/notifications/menu bar/Finder/mount/update/rollback | Verified on x64 and ARM64; signing-ready layout is verified with ad-hoc signatures, while production signing/notarization configuration remains a distribution task. |
| Linux X11/Wayland/notifications/tray/file manager/mount/update/rollback | Verified on x64 and ARM64 where architecture-specific; X11 and Wayland desktop protocol checks run on Ubuntu x64. |

## Remaining release blockers

1. Add real OpenSSH transport and login/concurrent multi-mount integration coverage.
2. Exercise simultaneous per-connection transfer popups and the transfer center at the window level.
3. Complete native installer, icon, signing/notarization, and installed-update review for Windows, macOS, and Linux.
4. Add changed-host-key mismatch integration evidence with an explicit user-facing result.
5. Perform a packaged update while a real mount has an active queued upload and prove the mount/upload survive GUI replacement.
6. Run the final non-publishing release workflow after all blockers are closed, update PR evidence, and only then consider removing Draft status.
