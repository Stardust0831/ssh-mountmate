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
- Bundle the official rclone binary in release builds.
- Download the official rclone zip into an app-managed local bin directory when the bundled binary is unavailable.
- Show copyable manual install commands when automatic installation cannot finish.
- Configure global rclone VFS cache options in the GUI.
- Show mount status, capacity usage, logs, and common actions per connection.
- Mount or unmount all saved connections from the main window.
- Build single-file executables for Windows, macOS, and Linux with GitHub Actions.

## Requirements

SSH MountMate release builds bundle the official rclone binary for the target platform. If the bundled binary is unavailable, the app can download the official rclone zip into its own local bin directory and use that managed copy. Windows can also fall back to winget.

Windows:

- Windows 10 or 11
- bundled rclone, or a source-run managed/system rclone
- WinFsp
- OpenSSH Client

Copyable Windows dependency commands:

WinFsp can be downloaded directly from https://winfsp.dev/rel/ . If winget works well on your network, this command is also available:

```powershell
winget install --id WinFsp.WinFsp -e
powershell -NoProfile -ExecutionPolicy Bypass -Command "Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0"
```

macOS:

- bundled rclone, or a source-run managed/system rclone
- macFUSE
- OpenSSH Client

Important macOS note: SSH MountMate release builds use the bundled official rclone binary, so users normally do not need Homebrew rclone. If you override rclone or run from source, do not use the Homebrew `rclone` package for mounting. Homebrew's rclone package cannot run `rclone mount` on macOS. Use the official rclone binary instead:

```bash
curl https://rclone.org/install.sh | sudo bash
```

SSH MountMate's fallback dependency installer also uses the official rclone zip on macOS and stores it inside the app's user data directory, so it does not require `sudo` for rclone itself.

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

- bundled rclone, or a source-run managed/system rclone
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

In the Settings window, `Check dependencies` shows the current mount-layer dependency as `WinFsp`, `macFUSE`, or `FUSE`. If macOS/Linux system dependencies are missing, `Install missing dependencies` opens copyable commands instead of trying to modify the system silently.

## Bundled And Managed rclone

Release builds bundle rclone inside the executable. During build, SSH MountMate downloads the official rclone zip for the build runner's platform and CPU architecture and embeds the extracted binary with PyInstaller.

If a bundled rclone is not available, SSH MountMate can still build the official zip URL from the current platform and CPU architecture:

```text
https://downloads.rclone.org/rclone-current-<platform>-<arch>.zip
```

The platform part is `windows`, `osx`, or `linux`. The architecture part is usually `amd64` for Intel/AMD 64-bit machines or `arm64` for Apple Silicon/AArch64 machines. Managed `rclone` copies are stored under `%LOCALAPPDATA%\SSHMountMate\bin` on Windows, `~/Library/Application Support/SSHMountMate/bin` on macOS, and `${XDG_DATA_HOME:-~/.local/share}/ssh-mountmate/bin` on Linux. These managed copies are preferred over PATH on later launches.

The remote server is assumed to be a Linux server reachable over SSH/SFTP.

## Download

Use the latest GitHub Release and download the package for your platform:

- `SSHMountMate-windows-x64.zip`
- `SSHMountMate-windows-arm64.zip`
- `SSHMountMate-macos-x64.zip`
- `SSHMountMate-macos-arm64.zip`
- `SSHMountMate-linux-x64.zip`
- `SSHMountMate-linux-arm64.zip`

Release builds are produced by GitHub Actions from the same Python source tree.

Each release zip contains only the platform executable. Bundled third-party notices can be viewed from Settings or with:

```bash
SSHMountMate --licenses
```

Program updates can be checked from Settings -> Check for updates, or from the command line:

```bash
SSHMountMate --check-update
```

The update check reads the latest GitHub Release and shows the matching download asset for the current platform and CPU architecture.

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

## Capacity Display

For mounted connections, SSH MountMate shows used and total capacity on each card. On Lustre paths, it first tries to read the remote directory's project ID with `lfs project -d` and then reads project quota with `lfs quota -p`. If the path is not on Lustre, `lfs` is unavailable, or the project has no nonzero hard block limit, the app falls back to `rclone about`.

## Settings

The Settings window contains:

- dependency checks
- program update check
- mount log access
- language selection
- Windows/macOS login startup mount option
- rclone VFS cache root
- VFS cache mode
- max cache size
- max cache age
- minimum free space
- write-back delay
- directory cache time
- read buffer size

Each setting option has a `?` help icon in the GUI. Hover the icon to see what the option does. Batch mount and unmount concurrency are fixed internally at 4 and 8 workers.

On macOS, the login startup option writes per-config user LaunchAgent files under `~/Library/LaunchAgents/`. Each job calls SSH MountMate's headless `--mount-id` entrypoint and mounts the saved config after the user logs in.

## Building From Source

Install Python 3.10 or newer.

Run from the repository root:

```bash
python -m pip install -e ".[build]"
python build/build_local.py
```

The executable is written to:

```text
dist/
```

PyInstaller builds for the current operating system. Use GitHub Actions or native machines to build all three platforms.

## Development

Run the GUI from source:

```bash
python -m pip install -e .
python -m ssh_mountmate
```

Useful checks:

```bash
python -m py_compile $(find src build -name '*.py' -print) launcher.py
python -m ssh_mountmate --version
python -m ssh_mountmate --install-help
python -m ssh_mountmate --licenses
```

## License

SSH MountMate's application code is released under the MIT License. See `LICENSE`.

Release builds bundle rclone. rclone is distributed under the MIT License. See `THIRD_PARTY_NOTICES.md`, `licenses/rclone-COPYING.txt`, or the in-app Settings -> View licenses window.

The bundled Noto Sans CJK SC font is distributed under the SIL Open Font License. See `src/ssh_mountmate/assets/fonts/LICENSE-Noto-CJK.txt`.
