# SSH MountMate

[中文说明](README.zh-CN.md)

SSH MountMate is a cross-platform desktop app for mounting Linux servers as local drives or folders over SSH/SFTP.

It uses rclone for the actual mount operation and provides a small GUI around the parts that are usually tedious: dependency checks, SSH config import, rclone remote generation, mount options, logs, and startup mounts.

## What It Does

- Mount a Linux server directory on Windows, macOS, or Linux.
- Import hosts from your existing OpenSSH config and use them as editable defaults.
- Batch import all concrete hosts from a selected SSH config file.
- Start from an SAI cluster preset and write app-managed SSH config entries.
- Add connections manually with host, username, port, password, key file, and key passphrase.
- Optionally copy a selected key into `~/.ssh` and write the copied `IdentityFile` path.
- Choose the connection method per mount: rclone native SFTP or system OpenSSH.
- Store passwords and key passphrases through `rclone obscure`, not as plain text.
- Check for rclone and platform mount dependencies.
- Bundle and verify the official rclone binary in release builds.
- Configure global rclone VFS cache options in the GUI.
- Show mount status, capacity usage, logs, and common actions per connection.
- Show the real rclone upload queue and remote-transfer progress after local file copies appear complete.
- Verify remote directory contents on refresh and expose refresh/transfer actions from connection-card context menus.
- Mount or unmount all saved connections from the main window.
- Build native Rust packages for Windows, macOS, and Linux on x64 and arm64 with GitHub Actions.

## Requirements

SSH MountMate release builds bundle the official rclone binary for the target platform and verify it before use. Source builds can use an explicitly configured rclone, a previously managed copy, or a compatible rclone found on `PATH`.

Windows:

- Windows 10 or 11
- bundled rclone, or a source-build configured/system rclone
- WinFsp
- OpenSSH Client

Copyable Windows dependency commands:

WinFsp can be downloaded directly from https://winfsp.dev/rel/ . If winget works well on your network, this command is also available:

```powershell
winget install --id WinFsp.WinFsp -e
powershell -NoProfile -ExecutionPolicy Bypass -Command "Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0"
```

macOS:

- bundled rclone, or a source-build configured/system rclone
- macFUSE
- OpenSSH Client

Important macOS note: SSH MountMate release builds use the bundled official rclone binary, so users normally do not need Homebrew rclone. If you override rclone or run from source, do not use the Homebrew `rclone` package for mounting. Homebrew's rclone package cannot run `rclone mount` on macOS. Use the official rclone binary instead:

```bash
curl https://rclone.org/install.sh | sudo bash
```

macFUSE is still required for mounting on macOS, and it can be installed with Homebrew Cask:

```bash
brew install --cask macfuse
```

After installing macFUSE, macOS may ask you to allow the system extension in `System Settings -> Privacy & Security`. Approve it if prompted, then retry the mount.

If macOS blocks the downloaded app because it is not notarized, remove the quarantine attribute after unzipping:

```bash
sudo xattr -r -d com.apple.quarantine /path/to/SSHMountMate*
```

Linux:

- bundled rclone, or a source-build configured/system rclone
- FUSE support, usually `fuse3`
- OpenSSH Client

SSH MountMate detects Linux distributions from `/etc/os-release` and shows the matching FUSE/OpenSSH command first in the app. The main families are:

- Debian family: Debian, Ubuntu, Linux Mint, Pop!_OS
- Fedora/RHEL family: Fedora, RHEL, CentOS Stream, Rocky Linux, AlmaLinux
- Arch family: Arch Linux, Manjaro, EndeavourOS
- openSUSE/SUSE family: openSUSE Leap, Tumbleweed, SLES

<details>
<summary>All common Linux dependency commands</summary>

```bash
# Debian family: Debian, Ubuntu, Linux Mint, Pop!_OS
sudo apt update && sudo apt install -y fuse3 openssh-client

# Fedora/RHEL family: Fedora, RHEL, CentOS Stream, Rocky Linux, AlmaLinux
sudo dnf install -y fuse3 openssh-clients

# Arch family: Arch Linux, Manjaro, EndeavourOS
sudo pacman -S --needed fuse3 openssh

# openSUSE/SUSE family: openSUSE Leap, Tumbleweed, SLES
sudo zypper install -y fuse3 openssh
```

</details>

In the Settings window, `Check dependencies` reports rclone, OpenSSH, and the current mount-layer dependency (`WinFsp`, `macFUSE`, or `FUSE`). SSH MountMate does not silently modify system packages.

## Bundled And Managed rclone

Release workflows download a pinned official rclone archive for the target platform and architecture, verify its SHA-256 digest, and place rclone beside the Rust application inside the package. At runtime SSH MountMate verifies the bundled digest again and materializes a content-addressed managed copy in the application data directory. Explicitly configured and existing legacy managed copies remain supported for migration; a compatible system rclone is the final source-build fallback.

The remote server is assumed to be a Linux server reachable over SSH/SFTP.

## Download

Use the latest GitHub Release and download the package for your platform:

- `SSHMountMate-windows-x64.zip`
- `SSHMountMate-windows-arm64.zip`
- `SSHMountMate-macos-x64.zip`
- `SSHMountMate-macos-arm64.zip`
- `SSHMountMate-linux-x64.zip`
- `SSHMountMate-linux-arm64.zip`

Release builds are produced from the Rust workspace by six native GitHub Actions runners. Extract the complete package and keep its adjacent `bin` and license files together. macOS packages contain `SSH MountMate.app`.

Bundled third-party notices can be viewed from Settings or with:

```bash
SSHMountMate --licenses
```

Program updates can be checked from Settings -> Check for updates, or from the command line:

```bash
SSHMountMate --check-update
```

The in-app updater downloads the matching native GitHub Release asset, verifies its size and SHA-256 digest, rejects unsafe ZIP paths, stages the new directory bundle or macOS application beside the current installation, and restarts SSH MountMate after confirmation. A startup health handshake commits the update; timeout or failure restores and relaunches the previous build. Existing rclone mounts and uploads continue while the GUI restarts.

Automatic installation requires SSH MountMate to be extracted to a permanent, user-writable folder. Builds launched directly from a ZIP temporary directory and assets without a trusted SHA-256 digest remain manual-update only. Automatic background checks can be disabled in Settings.

Check CPU architecture:

```powershell
# Windows
$env:PROCESSOR_ARCHITECTURE
```

```bash
# macOS / Linux
uname -m
```

Use `x64` packages for `AMD64` / `x86_64`, and `arm64` packages for `ARM64` / `arm64` / `aarch64`. On macOS, use `SSHMountMate-macos-x64.zip` for Intel Macs and `SSHMountMate-macos-arm64.zip` for Apple Silicon Macs.

## Quick Start

1. Install the platform dependencies above.
2. Confirm normal SSH login works:

   ```bash
   ssh your-host
   ```

3. Start `SSHMountMate`.
4. Click `Add config`.
5. Choose either:
   - `SSH config`: select an existing `Host` entry and let the app fill defaults.
   - `SSH config (batch)`: choose an SSH config file, preview it, then import all concrete `Host` entries.
   - `SAI cluster`: start from the SAI preset. HostName and port are prefilled; fill username and key file.
   - `Manual`: enter host, username, port, and authentication details yourself.
6. Pick a remote path. `$HOME` is the default base.
7. Choose a connection method if the default does not fit.
8. Save, then click the mount button on the connection card.

On Windows, `Auto` mountpoint picks an available drive letter. On macOS and Linux, the app uses a per-connection mount folder by default. You can also type a custom mountpoint path.

Mountpoint rules:

- Windows drive letters such as `Z:` must be unused.
- Windows folder mountpoints must be absolute paths. The parent folder must exist, and the target folder itself must not already exist.
- macOS/Linux custom mountpoints must be absolute paths or start with `~`.
- macOS/Linux custom mountpoint folders are created automatically if missing.
- Existing macOS/Linux mountpoints are rejected to avoid mounting over another filesystem.

## SSH Config Import

SSH MountMate can read your OpenSSH config and list concrete `Host` entries. Selecting one fills:

- name
- host/IP
- username
- port
- key file

After import, the connection is saved as an editable rclone SFTP configuration. The mount behavior follows the values shown in the GUI, not a hidden live SSH command.

Batch import uses the selected config file and resolves each host with OpenSSH's `ssh -F <config> -G <host>` behavior. This keeps OpenSSH include/default handling while still saving normal editable SSH MountMate connections.

During batch import, duplicate entries are marked in the preview and skipped:

- `SAME`: same SSH `Host` alias and same HostName/User/Port.
- `SAME HOST`: same SSH `Host` alias but different resolved target.
- `SAME TARGET`: different alias but same HostName/User/Port.

Manual and SAI preset connections can also write an app-managed SSH config entry. For SAI, the default profile name and SSH `Host` are `SAI-<username>`, with `HostName c1.sai.ai-4s.com` and `Port 12022`. SSH MountMate creates `~/.ssh` when needed, adds this include line to `~/.ssh/config`, and writes each managed Host into its own file:

```sshconfig
Include ~/.ssh/ssh-mountmate.d/*.conf
```

If `Copy key to ~/.ssh` is enabled, the selected private key is copied into `~/.ssh`, and both the mount profile and generated SSH config use the copied `IdentityFile` path. Passwords and key passphrases are never written to SSH config.

## Connection Method

Each saved connection can use one of two methods:

- `rclone native SFTP`: the default. rclone handles SSH/SFTP itself and can use saved rclone-obscured passwords or key passphrases.
- `OpenSSH`: rclone calls the system `ssh` command. This is useful for OpenSSH features such as `ProxyJump`, `ProxyCommand`, custom `Include` logic, or system ssh-agent behavior.

When `OpenSSH` is selected, SSH MountMate does not save or pass key passphrases to `ssh`. Add passphrase-protected keys to your agent first:

```bash
ssh-add ~/.ssh/id_ed25519
```

On macOS, use Keychain support when available:

```bash
ssh-add --apple-use-keychain ~/.ssh/id_ed25519
```

## Passwords And Key Passphrases

Passwords and key passphrases are passed through:

```bash
rclone obscure
```

The obscured value is stored in SSH MountMate's private rclone config. This avoids plain-text storage, but it is not strong encryption. On macOS and Linux, SSH MountMate writes configuration files with owner-only permissions. Treat the local user account and its config directory as sensitive.

## Host Key Validation

SSH MountMate enables rclone host key validation when possible.

For rclone SFTP remotes, the app maintains its own `known_hosts` file. The first connection to a host and port records the keys returned by `ssh-keyscan`; later connections keep those pinned keys instead of replacing them from the network.

If host key scanning is unavailable, the app falls back to the user's default OpenSSH `known_hosts` file.

If rclone reports `knownhosts: key mismatch`, SSH MountMate stops the mount rather than disabling validation. Verify the new fingerprint with the server administrator before removing that host's old entry from the app-managed `known_hosts` file and trying again.

## Transfers And Remote Refresh

Mounted connection cards show rclone's real VFS upload queue. When an upload starts, that connection gets its own bottom-right progress window; multiple active connections are stacked separately. The Transfer center remains available for manually viewing all mounts together. A file is only shown as cloud-synced after rclone reports no queued or active uploads. SSH MountMate warns before unmounting or exiting while uploads remain.

Refresh clears the VFS directory cache, actively reloads the requested directory, and verifies it with a direct remote listing. If local writes are still queued, the result states that the verified remote snapshot does not yet include those uploads.

Right-click a connection card for Open, Refresh, Transfers, and Log actions. Settings can register Refresh and Transfers commands in Windows Explorer, macOS Finder Quick Actions, and Nautilus, Nemo, or KDE file managers on Linux. The commands point back to the same SSH MountMate executable; no helper program is installed. A short-lived file-manager process forwards its request to the running app over authenticated loopback IPC and exits.

The Rust application keeps a native system-tray icon on Windows, a menu-bar item on macOS, and an AppIndicator on supported Linux desktops. Closing the main window hides it without stopping mounts or transfer monitoring. The tray menu can restore the main window, open Transfers, mount or unmount all connections, and explicitly exit the interface. Exit asks for confirmation when uploads are active or cloud state is unknown; rclone mount processes remain independent of the GUI.

## Capacity Display

For mounted connections, SSH MountMate shows used and total capacity on each card. On Lustre paths, it first tries to read the remote directory's project ID with `lfs project -d` and then reads project quota with `lfs quota -p`. If the path is not on Lustre, `lfs` is unavailable, or the project has no nonzero hard block limit, the app falls back to `rclone about`.

## Settings

The Settings window contains:

- dependency checks
- program update check
- mount log access
- transfer center and file-manager command registration
- language selection
- login startup mount option
- rclone VFS cache root
- VFS cache mode
- max cache size
- max cache age
- minimum free space
- write-back delay
- directory cache time
- read buffer size

Each setting option has a `?` help icon in the GUI. Hover the icon to see what the option does. Batch mount and unmount concurrency are fixed internally at 4 and 8 workers.

Login startup uses the current user's Windows Run key, a macOS LaunchAgent under `~/Library/LaunchAgents/`, or a Linux XDG autostart entry. It calls the Rust application's headless `--mount-startup-all` entrypoint after login.

## Building From Source

Install the Rust toolchain declared in `rust-toolchain.toml` and the GUI development libraries required by your operating system.

Run from the repository root:

```bash
cargo build --release --package ssh-mountmate
```

The executable is written to `target/release/`. Release packaging downloads and verifies the platform-specific rclone binary, so use the release workflow or the corresponding native operating system to produce distributable packages.

## Development

Run the GUI from source:

```bash
cargo run --package ssh-mountmate
```

Useful checks:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo run --package ssh-mountmate -- --version
cargo run --package ssh-mountmate -- --licenses
```

## License

SSH MountMate's application code is released under the MIT License. See `LICENSE`.

Release builds bundle rclone. rclone is distributed under the MIT License. See `THIRD_PARTY_NOTICES.md`, `licenses/rclone-COPYING.txt`, or the in-app Settings -> View licenses window.

Bundled Rust dependency notices are listed in `THIRD_PARTY_NOTICES.md` and `licenses/`.
