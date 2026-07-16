# v0.4.1-alpha.3 deferred manual acceptance

This checklist collects checks that require the Windows host, visible UI judgement, or real user
credentials. Automated CI remains the release gate; these checks are intentionally deferred until
the implementation is otherwise substantially complete.

Do not paste passwords, key passphrases, credential-store values, rclone configuration contents, or
private-key contents into this document or an issue. Attach only sanitized logs.

## Windows host: passphrase-protected SSH-config identity

1. Install `v0.4.1-alpha.3` through the in-app prerelease updater.
2. Use an SSH config `Host` whose resolved `IdentityFile` is protected by a passphrase.
3. Select Native transport and `rclone obscure` storage, replace and save the passphrase, restart
   SSH MountMate, then mount. Confirm the mount succeeds and no
   `private key is passphrase protected` critical line is produced.
4. Switch to system credential storage, confirm the compatibility-copy disclosure, restart the app,
   and mount again. Confirm the same result.
5. Temporarily make the system credential entry unavailable. Confirm mounting reports an explicit
   system-store error instead of silently using the compatibility copy. Restore the entry afterward.
6. Switch back to `rclone obscure`, restart, and confirm the connection still mounts.

## Error-card and full-log presentation

1. Trigger a safe mount failure, for example with a deliberately invalid test host.
2. Confirm the connection card uses at most two compact error lines and does not expand into a full
   page.
3. Confirm Retry preserves the operation, View full log opens the connection's dedicated read-only
   log window, and Dismiss removes only the durable card error.
4. Confirm missing and not-yet-created logs show the expected path and guidance without repeatedly
   opening modal errors.

## OpenSSH source-of-truth presentation

1. Open Settings for an imported SSH-config connection using OpenSSH transport.
2. Confirm config path, Host alias, and the quoted `ssh -F ... alias` preview are understandable.
3. Confirm host, user, and port read as an import snapshot rather than an editable source of truth.
4. On Windows, confirm Interactive shared SSH is absent for SSH-config and app-managed profiles.
   Also test selecting Interactive on a manual profile and then enabling app-managed SSH: the
   transport must change to OpenSSH with an explanation.

## Visual and accessibility pass deferred to later UI work

- Check disabled/read-only contrast, focus order, keyboard operation, Chinese/English wrapping,
  window sizing, and 100%/150%/200% scaling after mounted Settings, required markers, onboarding,
  and theme choices are all implemented.
- Check connection search, sorting, and folders/groups only after that low-priority feature reaches
  an implementation branch; it is not part of alpha.3 acceptance.

## Appearance settings

1. In Settings, switch between System, Light, and Dark. Confirm the whole app previews the choice
   immediately, including secondary windows, without requiring Save or a restart.
2. Cancel after changing the appearance. Confirm the previously saved theme and accent return.
3. Save each appearance mode, restart the app, and confirm the saved choice is restored.
4. With System selected, change the Windows host theme while the app is closed, then reopen it and
   confirm the matching light or dark palette is selected.
5. Try Blue, Green, Amber, and Purple accents in both light and dark modes. Confirm primary buttons,
   selections, focus states, text, disabled controls, warnings, errors, and required markers remain
   distinguishable at 100%, 150%, and 200% scaling.
6. Repeat the Settings flow in Chinese and English and check labels, wrapping, keyboard focus, and
   mounted read-only Settings presentation.

## App-managed interactive SSH terminal

1. On Linux and macOS, use direct OpenSSH profiles to exercise password, private-key passphrase,
   keyboard-interactive/2FA, host-key confirmation, and an OAuth/device-login URL. Confirm no
   external terminal opens and the queued mount resumes exactly once after the shared session is
   ready.
2. On Windows x64 and ARM64, repeat the lifecycle with a direct manual Plink profile. Confirm no
   visible console is created and SSH-config/app-managed profiles still cannot select Interactive.
3. Confirm Unicode input/output, IME input, resize, scrolling, explicit selection/copy, explicit URL
   opening, EOF, and child exit behave without terminal input or output appearing in app logs.
4. Click Mount repeatedly while authentication is pending. Confirm one PTY exists for that
   connection and only one mount starts when login succeeds.
5. Hide and reopen the terminal. Confirm the PTY and shared session remain alive. Close the window
   using its window control and confirm the same behavior.
6. Use End session and confirm the PTY is terminated and later mounts request a new login. Use Retry
   after a failed/exited child and confirm stale output/events do not resume the old queued mount.
7. With a live or authenticating terminal, exit from the tray. Confirm the bilingual warning counts
   the interactive session and Cancel leaves it alive.
8. While a terminal exists, change or delete its connection after it is safe to edit. Confirm the
   obsolete PTY is ended and its terminal window closes rather than using stale connection data.

## Evidence to record after testing

- App version and exact platform architecture.
- Whether each numbered check passed, failed, or was not applicable.
- Sanitized mount-log timestamps and error categories for failures.
- Screenshots only where presentation is the subject of the check; redact hostnames, usernames,
  paths, aliases, and remote names when needed.
