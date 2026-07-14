# SSH MountMate development roadmap and work log

This document is the persistent execution log for the Rust rewrite. It records planned work,
authoritative evidence, release decisions, and unresolved risks. A task is not marked complete
until its stated evidence exists.

## Current sequence

1. Publish `v0.4.0-alpha.3` from the merge-ready Rust state after six-platform and six-asset gates.
2. Keep the completed merge-readiness audit intact without changing mount backends or server code.
3. Verify settings UI semantics: enums use selectors, booleans use switches, values show units, and
   platform-specific controls are hidden where unsupported.
4. Re-run format, warnings-denied Clippy, workspace tests, and all six native lifecycle jobs.
5. Review remaining risks and decide whether draft PR #11 is ready for merge; do not merge solely
   because a prerelease exists.
6. Design an optional installed distribution later, with Windows as the first target and portable
   execution retained.
7. Implement optional macOS `rclone nfsmount` later as an explicit Experimental backend.
8. Keep macOS FUSE as the migration and UI default; keep Windows WinFsp and Linux FUSE3 unchanged.
9. Do not promote NFS to the default or publish another NFS-related release until real macOS x64 and ARM64
   FUSE/NFS lifecycle evidence has been reviewed.

## Prerelease scope: `v0.4.0-alpha.3`

Included work:

- Pure Rust application and packaging; no Python runtime, fallback, source, or active Python CI.
- Six canonical packages: Windows/Linux onefile and macOS native `.app`, each for x64 and ARM64.
- Verified official rclone, embedded in Windows/Linux onefile and contained in the macOS application.
- Canonical-asset self-update, authenticated health confirmation, rollback, and active-mount survival checks.
- Legacy migration, changed-host-key handling, OpenSSH transport, concurrent login mounts,
  transfer popups/center, native notifications, tray/menu-bar integration, and file-manager refresh.
- File-manager responsiveness improvement through the recommended 5-second VFS write-back window.
- More reliable upload progress by combining VFS queue state with `core/stats` transfer details.
- Capacity discovery fallback through a non-interactive remote `df -Pk` query.
- Main-window Mount all/Unmount all controls and a bounded, refreshable, copyable mount-log viewer.
- Settings switches, typed dropdowns, bilingual value-unit guidance, and explicit platform visibility.
- User-facing operation status based on connection display names rather than stable internal IDs.

Explicitly excluded:

- macOS NFS mount backend.
- Any change to the default macOS FUSE backend.
- Cloud/server changes.
- Merge of the draft Rust rewrite PR.

Required evidence before publishing:

- `cargo fmt --all --check`.
- Workspace Clippy with warnings denied.
- Complete workspace tests.
- Native Windows, macOS, and Linux x64/ARM64 builds.
- Canonical package smoke tests on all six targets.
- Real mount/refresh/queued-upload/unmount lifecycle checks.
- Packaged update commit/rollback and update during a real queued upload.
- A non-publishing `release.yml` run that validates exactly six ZIP assets plus checksums.

## Deferred implementation: optional macOS NFS backend

Planned design constraints:

- Add a strongly typed mount-backend enum to settings and mount state.
- Missing legacy fields deserialize to FUSE; existing users never switch silently.
- Show the selector and Experimental explanation only on macOS.
- Generate `rclone nfsmount` only for macOS users who explicitly select NFS.
- Bind the NFS service to loopback only; never listen on `0.0.0.0` or a LAN interface.
- Do not automatically fall back to FUSE after an NFS failure.
- Keep RC, VFS cache, write-back, refresh, transfer state, ownership validation, and cleanup truthful.
- Run the same real lifecycle suite for macOS FUSE and NFS on x64 and ARM64, including non-blocking
  performance records.

## Post-prerelease design: installed distribution and stable desktop identity

The application does not need installation to perform an rclone mount, but installation should
become the recommended desktop path once its update and rollback model is proven. The portable
package remains useful for first-run evaluation, recovery, and environments where installation is
not permitted.

Planned Windows direction:

- Prefer a per-user installation under a fixed user-writable location so self-update does not need
  administrator elevation or attempt to replace files under `Program Files`.
- Create a stable Start menu shortcut and AUMID for Toast identity, rather than relying on the path
  and identity of an arbitrary downloaded executable.
- Register Explorer refresh/transfer commands and login startup against the installed executable.
- Make upgrades preserve settings, mounts, the managed rclone copy, and the authenticated update
  health/rollback protocol.
- Provide an uninstaller that removes application files, Start menu entries, Explorer commands,
  login startup, and notification registration without deleting user connection data or cache
  unless the user explicitly requests that cleanup.
- Keep the portable onefile download available and clearly report that moving it can invalidate
  startup and file-manager registrations.

Cross-platform considerations:

- macOS already ships a native `.app`; the installed path should be `/Applications` or the user's
  Applications folder, followed later by production signing/notarization and an appropriate
  distribution container.
- Linux should keep a portable binary while evaluating desktop-entry integration and distro-neutral
  or package-manager-specific installers separately.
- Installer choice, signing, update ownership, downgrade behavior, repair, uninstall cleanup, and
  migration from the alpha portable packages require a dedicated design and CI matrix. They are not
  part of `v0.4.0-alpha.3`.

## Work log

### 2026-07-14

- Audited the settings page for merge readiness. Cache mode and language already use typed dropdown
  choices; connection source, authentication, and transport also remain typed selectors.
- Replaced the three settings booleans (mount at login, automatic transfer popup, and automatic
  update checks) with switch controls. Added bilingual size/duration unit guidance and examples for
  cache limits, cache timing, and buffer size without changing their persisted string fields.
- Replaced the compile-time file-manager visibility expression with a small explicit platform
  predicate and tests for Windows, Linux, macOS, and unsupported targets. No settings schema version
  changed, so legacy settings and custom rclone values continue to deserialize unchanged.
- Local format, core warnings-denied Clippy, all 151 core tests, and the legacy migration test passed.
  Full GUI compilation remains delegated to native CI because this workspace lacks GTK/pkg-config.
- Restored explicit Mount all and Unmount all buttons in the main window. The batch operations had
  remained available through tray and command IPC, but were not discoverable in the Rust main UI.
- Added a read-only mount-log viewer reachable from every connection card and from a Logs section in
  Settings. It supports refresh and Copy log, handles not-yet-created logs, and bounds rendering to
  the most recent 2 MiB so a large rclone log cannot freeze the GUI.
- Fixed operation status text to use the connection display name instead of its stable internal ID.
  Renaming a configuration from `NAS` to `jzj`, for example, now reports `jzj` while preserving the
  existing ID, state filename, and backward compatibility.
- Prepared `v0.4.0-alpha.3` on commit `4cacae5`. Local Rust 1.97 format checks, core
  warnings-denied Clippy, all 151 core tests, the legacy migration test, workflow YAML parsing, and
  diff checks passed. The three packaged GUI update tests remained intentionally ignored locally and
  were exercised by native CI.
- Rewrite run [29315522929](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29315522929)
  passed quality and all six native Windows, Linux, and macOS x64/ARM64 build and lifecycle jobs on
  `4cacae5`.
- Non-publishing release run
  [29316640097](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29316640097) passed quality,
  all six native build/lifecycle jobs, exact six-ZIP aggregation, and SHA-256 verification. It
  retained exactly six non-empty canonical artifacts and did not create a GitHub Release.
- Annotated tag `v0.4.0-alpha.3` resolves to the verified product commit `4cacae5`. Publishing run
  [29318232183](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29318232183) passed the same
  quality, six-platform lifecycle, six-ZIP, and checksum gates and published
  [`v0.4.0-alpha.3`](https://github.com/Stardust0831/ssh-mountmate/releases/tag/v0.4.0-alpha.3)
  as a non-draft prerelease with six platform ZIPs plus `SHA256SUMS.txt`. The checksum manifest has
  exactly one entry for every platform ZIP. PR #11 remains Draft.
- Preserved the user-owned untracked issue reply and five screenshots; none were staged. The
  prerelease still excludes an installer and macOS NFS, does not alter mount backends or server code,
  and does not merge Draft PR #11.

- Recorded installation as a post-alpha design task. The main benefit is stable application path
  and desktop identity for self-update, login startup, Explorer commands, Windows Toast/AUMID, and
  complete uninstall cleanup; mounting itself remains available without installation.
- Chose Windows per-user installation as the first design target while retaining the portable
  onefile. macOS continues to use the native `.app`, and Linux installer formats remain a separate
  evaluation. No installer was added to the in-progress six-asset alpha.3 release.
- Rewrite run [29276353414](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29276353414)
  passed the quality gate and all six native Windows, Linux, and macOS x64/ARM64 jobs on commit
  `6838b61`, including canonical artifact upload and real mount lifecycles.
- The first six-asset release dry run
  [29277440840](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29277440840) exposed a
  release-only Windows packaging bug: removal of the old onedir step also removed the command that
  created `release/`. Commit `4030719` makes the Windows onefile step create its output directory.
- The replacement release dry run
  [29278559051](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29278559051) passed quality,
  all six native build/lifecycle jobs, exact six-ZIP aggregation, and SHA-256 verification. It did
  not publish a GitHub Release.
- Annotated tag `v0.4.0-alpha.2` resolves to commit `140f53c`. Publishing run
  [29280113607](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29280113607) passed the same
  quality, six-platform lifecycle, six-ZIP, and checksum gates and published
  [`v0.4.0-alpha.2`](https://github.com/Stardust0831/ssh-mountmate/releases/tag/v0.4.0-alpha.2)
  as a non-draft prerelease with six platform ZIPs plus `SHA256SUMS.txt`. PR #11 remains Draft.

- Reduced the release matrix from twelve duplicate onefile/onedir ZIPs to six canonical ZIPs to
  reduce CI artifact and release download overhead. Windows and Linux keep onefile executables;
  macOS keeps the native `.app` bundle under the canonical asset name.
- Updated self-update asset selection to use the single canonical OS/architecture ZIP. Because all
  published alpha.1 onedir assets had zero downloads, users of an old noncanonical alpha package
  are directed to manually install alpha.2 once rather than maintaining duplicate package tracks.
- Reduced downloadable rewrite-workflow artifacts to the canonical Windows/Linux onefile or macOS
  application archive. Internal directory bundles remain available within jobs for lifecycle tests.
- Started `v0.4.0-alpha.2` prerelease verification. macOS NFS remains documented and deferred.

- Investigated reports that file copies could make the file manager unresponsive, transfer popups
  were not observed, and capacity was unavailable on some SFTP servers.
- Restored the recommended write-back delay from the forced `0s` override to rclone's upstream `5s`
  default. Schema 9 migrates only the recognizable prior recommended profile; custom zero-delay
  profiles remain unchanged. The delay applies on the next mount and gives file close, rename, and
  metadata operations a stable local-cache window before upload begins.
- Transfer snapshots now recover per-file bytes, speed, and percentage from `core/stats` when
  `vfs/stats` confirms an active upload but `vfs/queue` temporarily omits its file details. Core
  transfers are not treated as uploads unless the VFS disk cache independently reports an upload.
- Capacity discovery now falls back to a non-interactive remote `df -Pk` query after local mount,
  Lustre project quota, and `rclone about` data are unavailable. Direct password profiles without a
  managed SSH route continue to avoid password prompts.
- Local Rust 1.97 format checks, core Clippy, all 151 core tests, and legacy migration passed.
  Workspace compilation was locally limited only by missing GTK/pkg-config system packages.
- Native run [29272903757](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29272903757)
  passed quality and all six Windows, Linux, and macOS x64/ARM64 jobs, including package smoke tests
  and real SFTP mount, queued-upload, refresh, remote-completion, and unmount lifecycles. No release
  was published from this change.

### 2026-07-13

- Preserved user-owned untracked files (`issue-1-reply.md` and five screenshots); none are staged.
- Pushed `3e12c79` (`Record verified Rust rewrite integration gates`).
- Pushed `293144f` (`Verify updates preserve active mounts`).
- Workflow run [29253550458](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29253550458)
  proved the active-update path on Windows x64/ARM64, macOS x64, and Linux x64. macOS ARM64 and
  Linux ARM64 did not execute because GitHub Actions could not download actions (`Service
  Unavailable`), so the run is not accepted as a six-platform gate.
- Identified a distribution regression: the Rust workflow produced only an onedir payload under
  the historical onefile asset name. Began restoring true onefile plus `-onedir` assets.
- Added build-time SHA-256 validation and runtime content-addressed materialization for embedded
  rclone; core tests reached 144 passing tests before the additional conditional embedded-payload
  test was added.
- Added standalone update-payload discovery and package-type-aware asset naming.
- Added a `--rclone-path` diagnostic command for package smoke tests and user diagnostics.
- Updated both release workflows to build and verify onefile packages; this work must pass
  native review/CI before the prerelease is published.
- Local Rust 1.97 verification after the distribution changes: format and core Clippy passed,
  all 145 `mountmate-core` unit tests passed, legacy migration passed, and the conditional
  embedded-rclone test passed with a separately hashed controlled executable payload.
- Workflow YAML parsing, macOS/Linux shell syntax, and `git diff --check` passed locally. Full GUI
  compilation is delegated to native CI because this workspace does not provide the GTK/pkg-config
  development environment used by the Linux runner.
- Accepted macOS Experimental NFS as the next implementation task, explicitly after the prerelease.
- Run 29255461001 proved quality and macOS ARM64 including the new onefile path, but Windows ARM64
  exposed a PowerShell-only workflow bug: `$home` conflicts case-insensitively with the read-only
  automatic `$HOME` variable. The workflow variable was renamed to `$onefileHome`; a replacement
  six-platform run is required.
- Run [29256312407](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29256312407)
  passed quality plus Linux and macOS on x64/ARM64. Both Windows jobs compiled the onefile
  executable successfully, then failed because directly piping output from a GUI-subsystem EXE left
  PowerShell with `$null` and closed the child's stdout pipe. Both workflows now launch the
  diagnostic command with explicit stdout/stderr file redirection. A replacement six-platform run
  remains required before the non-publishing release dry run.
- Release-workflow review found that `publish=false` skipped the complete release aggregation job,
  including twelve-asset validation. The aggregation and checksum verification now run for dry
  runs; only the GitHub Release creation step is conditional on tag publication or `publish=true`.
- Run [29260504687](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29260504687)
  passed quality and all six native rewrite jobs on commit `5db0968`.
- The first complete release dry run
  [29262384073](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29262384073) passed quality
  and all six build/lifecycle jobs. Its aggregation job proved all twelve expected ZIPs existed,
  then exposed a checksum working-directory bug: `SHA256SUMS.txt` contained asset basenames but was
  checked from the repository root. Checksum verification now executes inside `release-assets/`;
  a replacement dry run is required and no release was created.
- Run [29264097069](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29264097069)
  passed quality and all six native rewrite jobs on commit `1a9673e`.
- Release dry run
  [29264106144](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29264106144) passed quality,
  Windows x64/ARM64, Linux x64/ARM64, and macOS x64. Its macOS ARM64 real lifecycle reached a live
  mount, remote refresh, and a reported queued upload, but the active-update assertion failed after
  the upload completed and disappeared from the queue during package replacement. The rclone log
  records the completed upload and final remote rename. Per the prerelease decision, this timing
  race is deferred and explicitly non-blocking only for the macOS ARM64 release job; it remains
  visible as a warning and macOS x64 remains blocking.
- Final rewrite run
  [29266212614](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29266212614) passed quality
  and all six strict native jobs on commit `ca66e5c`; macOS ARM64 passed on this run.
- Final non-publishing release run
  [29266223640](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29266223640) passed quality,
  all six native build/lifecycle jobs, exact twelve-ZIP aggregation, and SHA-256 verification.
- Tag release run
  [29267800919](https://github.com/Stardust0831/ssh-mountmate/actions/runs/29267800919) rebuilt and
  passed the same gates, then published
  [`v0.4.0-alpha.1`](https://github.com/Stardust0831/ssh-mountmate/releases/tag/v0.4.0-alpha.1)
  as a non-draft prerelease with twelve ZIP assets plus `SHA256SUMS.txt`. The annotated tag resolves
  to commit `ca66e5c`. PR #11 remains Draft and macOS NFS is not included.

## Release decisions

- The Rust rewrite PR remains Draft.
- No merge is authorized by this document.
- `v0.4.0-alpha.3` must be a prerelease, not a stable release.
- The macOS ARM64 active-upload package-replacement timing race is an explicit alpha exception, not
  evidence that the scenario passed. It must be resolved before a stable release.
- A failed or incomplete architecture gate blocks publication unless it is replaced by successful
  authoritative evidence or recorded above as a narrowly scoped, user-approved prerelease
  exception. Stable releases do not inherit alpha exceptions automatically.
