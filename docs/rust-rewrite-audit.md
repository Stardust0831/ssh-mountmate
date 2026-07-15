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
- [29251143444](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29251143444): six-platform quality/build run with complete legacy migration, real changed-host-key rejection and cleanup, real OpenSSH transport, four concurrent login mounts, two simultaneous bottom-right transfer popups, transfer-center activation, and popup completion on Linux x64/ARM64.
- [29378614305](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29378614305): current six-platform quality/build run for configurable VFS upload concurrency. All Windows, Linux, and macOS x64/ARM64 jobs passed package smoke tests, update/rollback, active queued-upload package replacement, and real SFTP mount/refresh/upload/unmount lifecycles.
- [29382800350](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29382800350): stable-version branch gate. Quality and all six authoritative package, update/rollback, and real SFTP lifecycle jobs passed on `19d096d`.
- [29382809180](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29382809180): non-publishing stable release gate. All six platform jobs were blocking and passed, including macOS ARM64 active-upload package replacement; release aggregation verified exactly six ZIPs and `SHA256SUMS.txt`.
- [29393569520](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29393569520): final stable branch gate on `b54be59`. Quality and all six native package, update/rollback, active-upload, real SFTP, and platform integration jobs passed.
- [29393569262](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29393569262): final non-publishing release gate. Tests exercised the exact final standalone Windows/Linux ZIPs and macOS application ZIPs; all six jobs and exact six-archive checksum aggregation passed.
- [29395079259](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29395079259): formal
  v0.4.0 tag run. Quality, all six native lifecycle jobs, final six-ZIP aggregation, checksum
  validation, and non-prerelease GitHub Release publication passed from merge commit `e2b222f`.
- [29398901355](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29398901355): post-release
  extension run. All six jobs passed; macOS x64/ARM64 each exercised real FUSE and loopback-only
  Experimental NFS mount lifecycles without changing the default backend.
- [29404531615](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29404531615): opt-in system
  credential storage run. Quality and all six jobs passed; Windows exercised Credential Manager and
  macOS exercised Keychain round trips. Linux Secret Service remains simulated in tests.
- [29414771418](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29414771418): final
  interactive shared-SSH run on `037dd88`. Quality and all six jobs passed. Linux x64/ARM64 opened
  the terminal login path, verified normal or shortened ControlMaster sockets, mounted through the
  non-interactive shared connector, and preserved separate interactive/OpenSSH rclone remotes.
  Windows x64/ARM64 verified fixed-key Plink sharing and completed the real mount lifecycle. Both
  macOS architectures also retained passing FUSE and loopback-only NFS lifecycle evidence.

## Product requirements

| Requirement | Status | Evidence and remaining work |
| --- | --- | --- |
| Existing server/settings/rclone/state/cache/log compatibility without losing secrets or custom settings | Verified | The fixture-driven `legacy_migration` integration test loads three legacy Python server shapes, obscured password and key-passphrase secrets, settings, mount state, cache/log data, and a mixed rclone config together; it then rewrites and reloads the data while proving custom settings, unrelated remotes/secrets, and legacy cache/log files remain intact. |
| Manual, SSH config, batch SSH config, SAI, password/key/passphrase, native SFTP, and OpenSSH creation paths | Verified | Connection-draft validation, import planning, duplicate protection, secret preservation, proxy/OpenSSH selection, and generated rclone remotes cover every creation path. Real mounts exercise native password/key authentication and external OpenSSH transport; the OpenSSH remotes contain no native-auth fallback. |
| Drive/folder allocation, duplicate detection, capacity, dependency checks, login mounts, and concurrent batches | Verified | Unit coverage verifies allocation, duplicates, local/Lustre capacity parsing, dependency discovery, and registration. Run 29251143444 executes the exact registered `--mount-startup-all` command, mounts four connections concurrently with a 0-second process-start spread on x64/ARM64, and unmounts all without stale state. |
| Truthful rclone RC transfer state | Verified | Unit tests cover queued, active, errored, unknown, exhausted-cache, and remote-byte states. Windows/Linux/macOS real mounts prove queued writes are not presented as complete and later become remotely complete. |
| Refresh order and Windows root quote repair | Verified | RC contract tests prove forget/refresh/list followed by a post-refresh queue snapshot and reject the legacy quote remote; real mount tests prove remotely created content appears after refresh. |
| Safe process ownership and PID-reuse behavior | Verified | Process/runtime tests cover exact argv identity, start-time mismatch, unverifiable ownership, safe RC quit, stale state, and never terminating a reused PID. |
| Verified update, extraction, staged replacement, health, and rollback | Ed25519 implementation pending new CI evidence | Existing packaged update/rollback runs on all six targets. The new client additionally requires a signed six-asset manifest plus matching GitHub REST digests, sizes, tag/channel and URLs before installation; tamper, missing-signature, wrong-key, rotation, and signing-tool tests run locally. Production key confirmation and a fresh six-platform run remain required. |
| Responsive bilingual GUI, shared transfer window, transfer center, tray/menu bar, notifications, global progress, and file-manager integration | Verified | Native tray/menu bar, notifications, Windows taskbar, macOS Dock, X11/Wayland capability reporting, authenticated IPC, and file-manager registration are exercised. The application now reuses one movable transfer window across mounts and expands it to per-connection details; unit tests verify aggregation, dismissal, unknown-size handling, and truthful completion. |
| Native x64/ARM64 packages without Python | Verified for portable packages | Six native ZIP packages contain the Rust executable, verified rclone, and notices with no Python runtime. The approved v0.4.0 scope is portable and unsigned; native installers and production signing/notarization remain separate distribution work. |
| Experimental macOS built-in NFS | Verified as opt-in, not default | Both macOS architectures completed real loopback NFS lifecycles in run 29398901355. Legacy settings and new defaults remain FUSE; Windows/Linux commands are unchanged. Evidence is not yet sufficient to promote NFS to the default. |
| System credential storage | Verified on Windows/macOS; Linux native service pending | The setting is manually enabled and migrates only passwords/private-key passphrases after write/read verification. Credential Manager and Keychain have real CI evidence; Linux currently has compile and fake-store coverage only. |
| Interactive shared SSH | Verified on Linux and Windows x64/ARM64 | OpenSSH ControlMaster and verified Plink sharing connectors contain no one-time response. Linux uses owner-only normal or stable shortened sockets and isolates interactive rclone remotes by server ID; both Linux architectures completed real terminal-login, second-request mount, read, and transfer-window lifecycles. Windows remains limited to direct manual connections, but both architectures completed fixed-host-key Plink sharing and real mounts. macOS uses the same OpenSSH implementation and has unit/build coverage; a dedicated macOS interactive-login lifecycle is still a useful follow-up, not a blocker for the verified Linux/Windows scope. |

## Historical regressions

| Regression | Status | Evidence |
| --- | --- | --- |
| Unreadable `known_hosts`; first-seen pinning; mismatch visibility | Verified | Unit tests cover unreadable-file exclusion, managed first-seen keys, idempotence, and no silent replacement. Run 29251143444 replaces the host key on the same live endpoint, requires a user-facing `knownhosts: key mismatch`, proves the managed file digest is unchanged, and leaves no mount or stale state. |
| Windows ACLs for SSH config and copied keys | Verified | Windows ACL implementation and native Windows tests cover protected current-user/SYSTEM access. |
| Explicit host-key mismatch and `--links` | Verified | Rclone command tests prove `--links`; run 29251143444 proves the explicit changed-key failure reaches the user and cannot silently repin the host. |
| Readiness polling instead of fixed sleep | Verified | Runtime tests and all real mount workflows require RC/process/mountpoint readiness. |
| Parent log handles close after spawn | Verified | Process-spawn implementation plus persistent Windows mount lifecycle evidence. |
| Windows status avoids routine PowerShell CIM | Verified | Native process APIs and RC are used; Windows integration tests exercise status and ownership. |
| Platform-correct folder mount targets | Verified | Mountpoint allocator tests cover Windows missing-target semantics and Unix folder behavior; real mounts cover all three operating systems. |
| Cache defaults and migration preservation | Verified | Model migration and rclone command tests cover `full`, `30m`, `5s`, `5m`, and custom values. |
| Upload warnings distinguish truthful states | Verified | Transfer and UI-state unit tests plus real queued-write workflows. |
| GUI restart/update leaves mounts and uploads running | Verified | Run 29378614305 executes packaged replacement while a real mount has a queued upload on all six targets, verifies unchanged mount state and rclone identity, reads through the live mount, and confirms the queued or uploading state remains after GUI health handoff. |

## Verification gates

| Gate | Status |
| --- | --- |
| `cargo fmt --all --check` | Verified in rewrite and release quality jobs. |
| Zero-warning workspace Clippy | Verified in rewrite and release quality jobs. |
| Complete workspace tests | Verified in rewrite and release quality jobs and on all six build targets. |
| Six-platform packages and smoke tests | Verified at final branch run 29393569520 and complete non-publishing release workflow 29393569262. |
| Windows Explorer/ACL/IPC/Toast/tray/taskbar/mount/refresh/upload/update/rollback | Verified. |
| macOS bundle/notifications/menu bar/Finder/mount/update/rollback | Verified on x64 and ARM64; signing-ready layout is verified with ad-hoc signatures, while production signing/notarization configuration remains a distribution task. |
| Linux X11/Wayland/notifications/tray/file manager/mount/update/rollback | Verified on x64 and ARM64 where architecture-specific; X11 and Wayland desktop protocol checks run on Ubuntu x64. |

## Current signed-prerelease gates

1. Commit and push the Ed25519 implementation without merging the extension branch.
2. Require quality and all six Windows, Linux, and macOS x64/ARM64 jobs to pass on the exact commit.
3. Configure the protected production-signing Environment, generate the only production key, and
   commit its public record without persisting the private key outside the GitHub secret.
4. Show the production `key_id` and full public-key SHA-256 to the owner and obtain explicit
   confirmation before creating the `v0.4.1-alpha.1` tag or publishing any Release.
