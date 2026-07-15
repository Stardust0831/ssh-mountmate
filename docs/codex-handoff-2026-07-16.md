# SSH MountMate Codex handoff - 2026-07-16

This document is the handoff for the next Codex agent. It records the exact repository baseline,
recent release work, newly reported product problems, implementation boundaries, likely code entry
points, and the evidence required before the next prerelease.

## Repository baseline

- Repository: `Stardust0831/ssh-mountmate`
- Workspace: `/mnt/g/work/agent/rsshmount`
- Active branch: `feature/macos-nfs-credentials-ssh`
- Branch head at handoff creation: `d48aa4b` (`Harden signed draft publication recovery`)
- Published prerelease: `v0.4.1-alpha.1`
- Immutable release tag commit: `be1b917dc5d527db12964d7e163433116b2d973d`
- Release URL:
  <https://github.com/Stardust0831/ssh-mountmate/releases/tag/v0.4.1-alpha.1>
- The extension branch is currently unmerged. The user authorizes merging it after the requested
  implementation, review, and required verification are complete; do not merge incomplete or
  failing work merely because merge permission exists.
- Do not move, delete, or reuse `v0.4.1-alpha.1`. The next prerelease must use a new version and
  tag, normally `v0.4.1-alpha.2` unless repository state requires another version.

The following existing user files are untracked and must not be edited, deleted, committed, or
used as generated output unless the user explicitly asks:

- `issue-1-reply.md`
- `屏幕截图 2026-06-27 013756.png`
- `屏幕截图 2026-06-27 031441.png`
- `屏幕截图 2026-07-07 020503.png`
- `屏幕截图 2026-07-07 020603.png`
- `屏幕截图 2026-07-07 020631.png`

## Release and signing state

`v0.4.1-alpha.1` is the first Ed25519-signed prerelease. Automatic installation requires agreement
among the signed manifest, `key_id`, version, stable/prerelease channel, canonical platform asset,
GitHub REST digest, size, SHA-256, and canonical GitHub URL.

- Production `key_id`: `ed25519-563e14d2c6b880f9`
- Raw public-key SHA-256:
  `563e14d2c6b880f9326f71c809a49474ec74cf74ca2347cc5ac3bf6efad27a2a`
- Production private key exists only in the protected GitHub Environment secret. It has no offline
  backup by explicit owner choice. Never print, download, copy, or regenerate it casually.
- Environment: `production-update-signing`
- Required reviewer: `Stardust0831`
- Deployment policy was restored to the single `v*` tag rule after controlled release recovery.
- Windows/Linux onedir self-update remains unsupported because the Release contains only the six
  canonical onefile/macOS ZIP assets.
- The v0.4.0-to-v0.4.1 update is still a first-trust bootstrap because v0.4.0 does not contain the
  Ed25519 public key.

Authoritative evidence:

- Branch gate with embedded production public key:
  <https://github.com/Stardust0831/ssh-mountmate/actions/runs/29435256461>
- Exact-tag production build gate:
  <https://github.com/Stardust0831/ssh-mountmate/actions/runs/29442216176>
- Quality, all six platform jobs, real mount/update tests, and the ephemeral signed release-set job
  passed in the production run. Its automated publish job stopped on GitHub draft lookup semantics;
  the draft was then published under an automatic rollback trap and verified with the same
  `update-signing verify-published` implementation against actual public metadata.
- Release ID `354659949` contains six platform ZIPs, `SHA256SUMS.txt`, the production manifest, and
  the signature. All nine GitHub assets have REST SHA-256 digests.
- Permanent workflow recovery and rollback logic is in `d48aa4b`. It is not part of the old tag and
  must be included in the next release commit.

See also:

- `docs/update-signing.md`
- `docs/development-roadmap.md`
- `docs/rust-rewrite-audit.md`
- `release-notes/v0.4.1-alpha.1.md`
- Chinese handoff: `docs/codex-handoff-2026-07-16.zh-CN.md`

## User authorization for the next work

The user explicitly authorized implementing all issues in this handoff and publishing a new
prerelease after implementation, self-review, and required verification. Do not ask for a second
release authorization. This does not authorize:

- publishing a stable Release;
- weakening or bypassing Ed25519 checks;
- moving an existing tag;
- changing cloud/server code;
- storing private keys, passwords, passphrases, OAuth tokens, or one-time codes in logs or docs.

Use the protected production-signing Environment normally for the new prerelease. A new production
public-key confirmation is not required unless the signing key changes. Environment approval and
all workflow security gates still apply.

The user also explicitly authorizes a branch merge after implementation, self-review, and required
CI evidence. A merge does not itself authorize a stable Release or bypass prerelease signing gates.

## Product and platform boundaries

- Keep the product pure Rust and continue using the packaged official rclone.
- Do not modify remote/cloud/server code.
- Windows remains WinFsp, Linux remains FUSE3, and macOS remains FUSE by default.
- macOS built-in `rclone nfsmount` remains explicit Experimental opt-in and loopback-only. Do not
  promote it to default as part of this work.
- Do not add silent backend, transport, credential-store, or authentication fallbacks.
- User-selected settings affect the next mount unless a requirement explicitly says otherwise.
- Preserve old settings, server files, mount state, logs, rclone config, and credential references.
- Never expose secret plaintext in status text, terminal command previews, debug output, panic
  messages, tests, screenshots, or release artifacts.

## New reports and required behavior

### P0 - System credential store can lose the private-key passphrase

User report:

- After enabling the system credential store, the private-key passphrase field became empty.
- Mounting then reported that the private-key passphrase was missing.
- After switching back to `rclone obscure`, entering and saving the passphrase still did not persist
  reliably.
- The user cannot see what migration occurred or whether it succeeded.

Treat this as a potential secret-loss/data-loss defect and a release blocker. Do not work around it
by silently falling back to empty secrets or `rclone obscure`.

Relevant implementation:

- `crates/mountmate-core/src/credential.rs`
  - `migrate_server_to_system`
  - `migrate_server_to_obscure`
  - `hydrate_server_from_system`
  - `replace_verified`, rollback, and deletion behavior
- `crates/mountmate-core/src/connection.rs`
  - `ConnectionDraft::from_server`
  - `ConnectionDraft::validate`
  - `SecretAction::{Clear, Keep, Obscure}`
  - `ValidatedConnection::apply_secrets`
- `crates/mountmate-app/src/main.rs`
  - `save_connection`
  - `save_settings`
  - `prepare_secret_action`
  - `migrate_servers_for_storage`
  - `cleanup_new_system_credentials`
  - `cleanup_retired_system_credentials`
- `crates/mountmate-core/src/service.rs`
  - `hydrate_server_credentials`
  - `prepare_server_credentials`

Important current behavior:

- Editing a server intentionally initializes password/passphrase text inputs as blank. Blank must
  mean "unchanged" when a valid obscured value or system credential reference exists.
- A system-stored secret is represented by `password_credential` or `key_pass_credential`; the
  corresponding obscured field is cleared after verified migration.
- Only native SFTP hydrates stored credentials into a temporary obscured rclone value. OpenSSH and
  interactive transports delegate authentication to SSH and should not claim to consume a stored
  key passphrase.

Required design and acceptance:

1. Reproduce on the real Windows Credential Manager path first; also inspect macOS Keychain and
   Linux Secret Service behavior without assuming the cause is cross-platform.
2. Capture sanitized before/after server records: only whether obscured/reference fields are empty,
   never their values.
3. Migration to the system store must be transactional:
   - reveal old obscured secret locally;
   - write OS credential;
   - read it back and compare;
   - persist the credential reference;
   - reload the persisted server record;
   - only then retire the old obscured value/rclone secret.
4. Migration back to obscure must reverse the order:
   - read OS credential;
   - obscure it;
   - save and reload the server record;
   - only then delete the OS credential.
5. If any step fails, preserve the last usable representation and show a durable error. Never leave
   both representations empty.
6. The UI must show a non-secret state such as "Stored in system credential store" with explicit
   replace/clear actions. A blank password box must not imply that the stored value vanished.
7. Saving an unrelated field while the secret input is blank must preserve the existing reference.
8. Switching storage back and then entering a replacement passphrase must persist and survive app
   restart.
9. Add end-to-end migration/edit/mount regression tests. The existing low-level native credential
   round-trip test is not sufficient.

### P0 - Mountpoint must be on a supported local Windows volume

Observed rclone/WinFsp failure when attempting `Z:\test\mount`:

```text
2026/07/16 04:00:08 ERROR : sftp://xujiacheng@c0.sai.ai-4s.com:12022/: Mount failed
2026/07/16 04:00:08 NOTICE: Z:\test\mount: Unmounted rclone mount
2026/07/16 04:00:08 CRITICAL: Fatal error: failed to umount FUSE fs: mount failed
2026/07/16 04:00:20 NOTICE: sftp://xujiacheng@c0.sai.ai-4s.com:12022/: Symlinks support enabled
Cannot set WinFsp-FUSE file system mount point.
The service rclone-492648a3867d has failed to start (Status=c0000277).
```

The current allocator verifies that a custom Windows parent exists and the target does not exist,
but it does not verify that the backing volume is a supported local volume. A mapped network drive
or another unsupported mounted filesystem can therefore reach rclone and fail late.

Relevant implementation:

- `crates/mountmate-core/src/mountpoint.rs`
  - `MountpointProbe`
  - `SystemMountpointProbe`
  - `MountpointAllocator::validate_custom`
- `crates/mountmate-core/src/runtime.rs`
- `crates/mountmate-core/src/service.rs`
- Windows platform bindings in `crates/mountmate-platform`

Required behavior:

1. Resolve the volume/root that backs a custom folder mountpoint before spawning rclone.
2. Use native Windows APIs such as `GetVolumePathNameW` and `GetDriveTypeW`; determine whether
   WinFsp directory mountpoints require a fixed local volume and whether any additional filesystem
   constraints are necessary.
3. Reject UNC paths, mapped network drives (`DRIVE_REMOTE`), unknown/no-root volumes, and other
   unsupported volume types with a clear localized preflight error.
4. Do not create the child mountpoint or start rclone after validation fails.
5. Keep drive-letter mountpoints and automatic local folder allocation working.
6. Add native Windows tests plus fake-probe unit coverage for local, remote, missing, and unsupported
   volume types.

### P0 - Errors disappear too quickly

The unsupported interactive SSH warning and mount failures currently appear in the shared bottom
status line and can be overwritten almost immediately by periodic status/transfer updates. The user
sees a flash but cannot read or act on it.

Required behavior:

- Add durable per-connection operation errors rather than relying only on the global status string.
- Keep an error visible until the user dismisses it, retries successfully, opens the related
  settings/log, or starts a clearly superseding operation.
- Show an actionable error surface with at least connection name, concise cause, details/log action,
  and dismiss/retry where appropriate.
- Do not repeatedly display the same polling failure as a modal dialog.
- Unsupported option combinations should be disabled in the editor with an adjacent explanation,
  so they do not fail only after Mount is clicked.
- Persist the last mount failure in memory across routine polling. Persisting a sanitized diagnostic
  record to app state is acceptable, but never persist secret values.

Likely entry points:

- `App::start_mount_operation` and `Message::MountFinished` in
  `crates/mountmate-app/src/main.rs`
- `localize_service_error`
- connection-card state and `server_card_view`
- periodic mount/transfer polling that currently rewrites `self.status`

### P1 - SSH config and OpenSSH semantics must be explicit

User concern:

- Selecting interactive shared SSH for an SSH-config connection on Windows flashes a message that
  only manual connections are supported.
- Some SSH config entries are simple direct hosts without ProxyJump/ProxyCommand. The UI should not
  imply that imported visible fields replace or rewrite all SSH config semantics.
- It is unclear whether the OpenSSH transport uses the SSH Host alias or the visible IP/host fields.

Current authoritative behavior:

- For an imported SSH-config source with a Host alias, OpenSSH and Unix/macOS interactive sharing
  build the equivalent of:

  ```text
  ssh -F <selected-config-path> <Host-alias>
  ```

- The Host alias and selected SSH config file are authoritative in this mode. The visible host,
  user, port, and identity fields are a resolved snapshot for display/import bookkeeping; they are
  not a complete replacement for `Include`, `Match`, `ProxyJump`, `ProxyCommand`, canonicalization,
  token expansion, agent, certificate, or other OpenSSH behavior.
- For a manual connection using OpenSSH, the visible fields do take effect. The connector uses the
  equivalent of `ssh -l <user> -p <port> [-i <key>] <host>`.
- For a program-managed SSH profile, the managed Host alias is authoritative.
- Windows interactive sharing currently rejects SSH-config and batch SSH-config sources before it
  starts bundled Plink. This restriction applies even to a simple direct entry because the code does
  not translate or certify the whole OpenSSH config contract for Plink.

Relevant implementation:

- `crates/mountmate-core/src/interactive_ssh.rs`
  - `windows_direct_connection_supported`
  - `openssh_target_arguments`
- `crates/mountmate-core/src/rclone.rs`
- `crates/mountmate-core/src/ssh.rs`
- `crates/mountmate-core/src/connection.rs`
- connection editor in `crates/mountmate-app/src/main.rs`

Required UI behavior:

1. On Windows, disable "Interactive shared SSH" for SSH-config and batch SSH-config sources and show
   a persistent explanation next to the disabled choice. Do not allow selection and fail later.
2. For an SSH-config source using OpenSSH or interactive OpenSSH, freeze fields that are not
   authoritative and label the source of truth:
   - selected SSH config path;
   - Host alias;
   - command-equivalent preview (`ssh -F ... alias`), safely quoted and with no secrets;
   - a read-only relevant config/resolution preview where feasible.
3. Explain that the preview is not a complete semantic expansion and that the actual OpenSSH command
   remains authoritative. Do not claim that only the displayed lines affect behavior.
4. For manual OpenSSH connections, keep host/user/port/key editable and state that these fields form
   the command.
5. Add tests for source/platform-specific transport choices and command previews.

### P1 - Interactive SSH should be app-managed, not a separate black console

Current implementation deliberately starts visible external UI:

- Windows bundled Plink uses `CREATE_NEW_CONSOLE`.
- macOS writes a `.command` script and opens it.
- Linux writes a shell script and launches an external terminal emulator.

The user does not want a separate console/terminal window stealing focus or interrupting other
work. The SSH process itself cannot disappear because OAuth/2FA prompts still need interactive I/O;
the requirement is no separate visible process window. Build an app-managed terminal/session UI.

Recommended direction:

- Windows: use ConPTY or another established Rust PTY integration and start Plink/OpenSSH without a
  new visible console.
- macOS/Linux: use a PTY-backed child process and render its terminal in an SSH login window/panel.
- Keep distribution to one executable by adding a hidden subcommand to the same binary, for example
  `SSHMountMate.exe --ssh-session-broker <server-id>`. The broker owns the ConPTY/PTY and SSH child;
  the main GUI connects through authenticated current-user-only local IPC. This is still one shipped
  EXE even though an app-owned helper process exists at runtime.
- Prefer the broker model when a shared SSH session must survive hiding/recreating the GUI. A strict
  single-process implementation is possible, but closing or replacing the GUI would also close its
  PTY unless substantially more lifecycle state is kept in the main process.
- `CREATE_NO_WINDOW` plus ordinary stdin/stdout pipes is not a sufficient replacement. SSH, Plink,
  password prompts, host-key confirmation, OAuth/device login, 2FA, and terminal control sequences
  may require a real TTY/PTY. Do not ship a pipe-based prompt parser that guesses prompt text.
- Render the PTY with a mature terminal parser/widget or a deliberately bounded terminal surface.
  Handle ANSI/VT sequences, cursor movement, backspace, resize, Unicode, IME input, URLs, EOF, and
  child exit without implementing a terminal emulator protocol from scratch.
- Keep one app-managed login session per connection and display lifecycle state, output, retry, and
  close semantics without logging secrets.
- Support prompts, device/OAuth URLs, 2FA codes, host-key questions, resize, EOF, and process exit.
- Closing the UI must have an explicit choice when a shared session is still required. Do not kill a
  live shared session merely because the view is hidden unless the user explicitly requests it.
- Do not make the process headless before an in-app interactive channel exists; that would strand
  password/OAuth/2FA prompts.
- Authenticate IPC, restrict endpoints to the current user, bound any in-memory scrollback, and do
  not persist terminal contents by default. Clipboard/export actions must be explicit because the
  terminal may contain tokens, hostnames, usernames, or other sensitive material.

### P1 - Harden macOS/Linux per-connection OpenSSH sockets

Keep the existing per-server socket design, including the short-path fallback, but validate it
before creation, readiness checks, connector use, cleanup, and reuse.

Current implementation creates the control directory and sets mode `0700`, but it does not fully
verify pre-existing path ownership/type/symlink status or the socket object before use.

Required checks:

- control directory is a real directory, not a symlink;
- owner is the current user;
- permissions are owner-only (`0700`, or a documented equivalently strict mode);
- control socket is a Unix socket, not a regular file/symlink/device;
- socket owner is the current user;
- socket permissions are not group/world accessible;
- stale cleanup only removes objects whose ownership and expected path identity are verified;
- the temp-directory fallback receives the same checks;
- path-length behavior remains covered.

Use native metadata (`symlink_metadata`, Unix file type extensions, uid/mode checks through `rustix`
or libc bindings already accepted by the project). Add malicious symlink, wrong-owner abstraction,
wrong-type, permissive-mode, stale-owned-socket, and short-path tests.

### P1 - Log viewer should guide selection and prioritize the failed connection

Current log window already has a selector, but when no log is selected it displays the generic
"No log content" message. The user interprets this as no logs existing.

Required behavior:

- Opening Mount logs without a selected connection must prominently instruct the user to choose a
  connection from the selector.
- Prefer the most recently failed or active connection when opened from an error/card context.
- A mount error action should open the corresponding log directly.
- If the selected log file does not exist, show the expected path and explain whether the connection
  has never been mounted, logging has not started, or the file is unavailable.
- Keep partial text selection/copy and initial scroll to the newest line.
- Do not replace the selector with a single global log dump.

Entry points are `open_log_window`, `open_log`, `log_viewer_view`, `MountLogView`, and related i18n
in `crates/mountmate-app/src/main.rs` and `crates/mountmate-app/src/i18n.rs`.

### P1 - First-run empty state should guide connection creation

When no connections exist, the first screen must present a clear empty-state action that opens the
new-connection editor. It may automatically open the editor on first run, but must not repeatedly
force it after the user intentionally closes it. Persist a lightweight onboarding-dismissed state
only if necessary; do not add a marketing/landing page.

### P1 - Required fields need visible red asterisks

Mark every actually required editor field with a red `*` in its label. The visual marker and
validation rules must come from the same field semantics so they cannot drift. At minimum review:

- display name;
- IP/host for manual sources;
- SSH Host alias and config path for SSH-config sources;
- user;
- port;
- password when password auth has no preserved secret;
- private key when native key auth requires one;
- any custom mountpoint/path value after the custom mode is selected.

Do not mark conditionally irrelevant or read-only fields as required. Keep localized accessible
labels; color alone is not sufficient.

### P2 - Add common theme choices

The app currently returns `Theme::Dark` unconditionally in `App::theme`. Add persisted, migrated
settings and a compact choice in Settings. Recommended minimum:

- follow system;
- light;
- dark;
- a small set of restrained accent presets such as blue, green, amber, and purple.

Reuse Iced theme APIs and existing layout patterns. Ensure contrast, focus, disabled, error,
progress, and selection states remain legible. Do not create separate incompatible component styles
for every accent. Add settings migration/serialization, bilingual labels/help, and UI-state tests.

### P1 - Rename card action "Edit" to "Settings" and allow read-only inspection while mounted

Each connection card currently disables Edit while the connection cannot be modified. Change the
button label to "Settings" / "设置" and allow it to open while mounted.

- When unmounted and not busy, fields remain editable and Save is available.
- When mounted, starting, unmounting, or otherwise locked, show the same configuration screen in
  read-only mode: controls are visibly disabled/grey, no mutation messages are emitted, and Save is
  hidden or disabled.
- Display a concise explanation that changes require unmounting and will affect the next mount.
- Secrets remain masked/state-only even in read-only mode.
- Remove/delete behavior remains separately protected and must not become available merely because
  Settings can be opened.

## Suggested implementation order

1. Reproduce and fix system credential migration/persistence. Add end-to-end regression tests.
2. Add durable per-connection errors and log navigation, because these make every later failure
   diagnosable.
3. Add Windows local-volume mountpoint preflight.
4. Make transport options source/platform aware and clarify SSH-config/OpenSSH authority.
5. Replace external interactive terminal windows with app-managed PTY UI; harden Unix sockets in
   the same transport-focused phase.
6. Add read-only mounted Settings and rename the card action.
7. Add first-run empty-state guidance and required-field markers.
8. Add theme settings.
9. Run focused review, full local gates, exact six-platform CI, then publish the next prerelease.

The user expects all listed items, including themes, before the next prerelease. P0/P1 labels indicate
risk and implementation order, not permission to omit lower-priority items.

## Required tests

At minimum add or extend coverage for:

- system store migration to/from obscure with passwords and key passphrases;
- unchanged connection edits preserving a system credential reference;
- replacing a system-stored secret and surviving restart;
- rollback at every persistence/deletion boundary;
- native mount hydration using the system credential;
- Windows local fixed-volume acceptance and network/UNC/unsupported-volume rejection;
- mount failure and unsupported-transport errors remaining visible until dismissed;
- error-to-log navigation and no-selection/missing-log guidance;
- Windows SSH-config interactive choice disabled before mount;
- SSH-config alias/config-path command construction versus manual host/user/port/key construction;
- in-app PTY session lifecycle and no visible external console creation;
- Unix control directory/socket owner, type, mode, symlink, stale cleanup, and short-path behavior;
- mounted connection Settings opening read-only with all mutation controls disabled;
- required markers following conditional validation;
- empty-state onboarding behavior;
- theme migration, serialization, bilingual labels, contrast-sensitive component states;
- stable/prerelease update channel selection and Ed25519 update tests remaining unchanged.

Required local commands when a Rust toolchain is available:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Also run focused tests during development. Before release, require quality and all six authoritative
Windows x64/ARM64, Linux x64/ARM64, and macOS x64/ARM64 jobs. Existing macOS FUSE and Experimental
NFS real lifecycle tests must continue to pass even though this task does not change their design.

## Engineering and security rules

- Inspect the dirty worktree before every edit and preserve unrelated/user files.
- Use `rg`/`rg --files` for search and `apply_patch` for manual edits.
- Do not use destructive git commands or force-move tags.
- Keep changes scoped; do not bundle unrelated refactors.
- Use strong types and existing project patterns rather than scattered string switches.
- Use native OS APIs or established crates for PTY, credentials, and volume inspection. Do not
  implement cryptography or terminal emulation protocols from scratch.
- Never reveal or log credentials while debugging. Record boolean presence, reference identity hash,
  result stage, and sanitized errors only.
- Credential migration must be fail-closed and transaction-like. A failed migration must not delete
  the last usable secret.
- Do not silently fall back from interactive SSH to native auth, from system credentials to obscure,
  from NFS to FUSE, or between mountpoints.
- Any background process must have explicit ownership, identity, lifecycle, and cleanup checks.
- Errors that affect user data or mounting must be durable and actionable, not transient status text.
- UI fields must accurately communicate which values are authoritative for the selected source and
  transport.
- Do not modify cloud/server behavior.

## Recent development log

The previous agent completed the following immediately before this handoff:

1. Implemented Ed25519 update manifest verification with a multi-key registry and production
   `key_id`.
2. Added signing CLI commands, tamper/rotation tests, GitHub digest checks, strict platform asset
   selection, and update installation gating.
3. Added protected-Environment release signing and six-platform release aggregation.
4. Fixed CI-only GitHub API rate limiting and Ubuntu ARM mirror failures.
5. Published `v0.4.1-alpha.1` after explicit public-key confirmation.
6. During publication, fixed missing Linux Secret Service build dependencies in signing jobs.
7. Added manual-release checkout of the requested immutable tag rather than moving a failed tag.
8. Recovered from GitHub draft lookup/`untagged-*` URL behavior with preverification, automatic
   rollback protection, and actual post-publication verification.
9. Hardened the permanent workflow to discover drafts explicitly, verify canonical expected URLs,
   verify actual public metadata, and restore draft state on failure.
10. Removed the temporary Environment branch policy; only `v*` tags remain allowed.

No product code for the newly reported issues was changed in these handoff turns. The only intended
changes are the English and Chinese handoff documents.

## Known residual risks

- The system credential defect is untriaged and may represent real secret loss. Treat it as the
  first task and do not prerelease until it is resolved with real native-store evidence.
- App-managed interactive terminals are a significant cross-platform feature. Do not underestimate
  PTY, focus, resize, encoding, prompt, and lifecycle work.
- Windows directory mountpoints have WinFsp constraints beyond simple path existence; verify native
  behavior instead of assuming all local-looking paths work.
- SSH config is a dynamic OpenSSH language. A displayed parsed fragment cannot be advertised as a
  complete behavior model.
- The production signing key has no offline backup. Avoid unnecessary Environment or secret changes.
- Packages are still not Authenticode-signed or Apple Developer ID signed/notarized.
- Windows/Linux onedir packages are not self-update assets.
- macOS interactive-login has less real lifecycle evidence than Linux/Windows.

## Completion and prerelease checklist

Before creating the next tag:

- all reports above are implemented, not merely documented;
- credential migration has real native evidence and no last-copy deletion path;
- Windows unsupported mountpoints fail before rclone starts;
- errors and logs are readable and actionable;
- transport UI matches actual command authority;
- no separate black console is created for interactive login;
- Unix shared sockets pass owner/type/mode checks;
- mounted Settings is read-only and visually disabled;
- onboarding, required markers, and theme choices are complete;
- local format/Clippy/workspace tests pass;
- all six platform CI jobs pass;
- self-review checks Windows/Linux/macOS regressions, secret handling, process cleanup, and release
  workflow integrity;
- Cargo/app version and release notes are bumped to a new prerelease;
- the new annotated tag points to the exact green commit;
- protected production signing, draft rollback, public metadata, and Ed25519 verification pass;
- Release is marked prerelease, not stable;
- if a branch merge is performed, it contains only the reviewed green changes and preserves the
  release/tag rules above; merge authorization is already granted.
