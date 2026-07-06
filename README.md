# SSH MountMate

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

The obscured value is stored in SSH MountMate's private rclone config. This avoids plain-text storage, but it is not strong encryption. Treat the local user account and its config directory as sensitive.

## Host Key Validation

SSH MountMate enables rclone host key validation when possible.

For rclone SFTP remotes, the app maintains its own `known_hosts` file and refreshes entries with `ssh-keyscan` for the target host and port. This helps avoid cases where OpenSSH succeeds but rclone rejects the server because only one host key type was present in the user's `~/.ssh/known_hosts`.

If host key scanning is unavailable, the app falls back to the user's default OpenSSH `known_hosts` file.

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
- batch mount concurrency
- batch unmount concurrency

Each option has a tooltip in the GUI.

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

## 中文说明

SSH MountMate 是一个跨平台桌面程序，用来通过 SSH/SFTP 把 Linux 服务器目录挂载成本地磁盘或本地文件夹。

它底层使用 rclone 完成真正的挂载，GUI 负责处理依赖检查、SSH 配置导入、rclone 配置生成、挂载选项、日志查看和开机挂载等操作。

## 功能

- 在 Windows、macOS、Linux 上挂载 Linux 服务器目录。
- 从已有 OpenSSH config 导入 Host，并作为可编辑默认值。
- 从指定 SSH config 文件中批量导入全部具体 Host。
- 通过 SAI 集群预设创建配置，并写入应用托管的 SSH config。
- 手动添加连接，支持主机、用户名、端口、密码、密钥文件和密钥短语。
- 可选把选中的密钥复制到 `~/.ssh`，并写入复制后的 `IdentityFile` 路径。
- 每个挂载配置都可以选择连接方式：rclone 原生 SFTP 或系统 OpenSSH。
- 密码和密钥短语通过 `rclone obscure` 保存，不明文存储。
- 检查 rclone 和系统挂载依赖。
- Release 构建内置官方 rclone 二进制。
- 如果内置 rclone 不可用，可下载官方 rclone zip 到应用自己的本地 bin 目录。
- 自动安装无法完成时，会显示可复制的手动安装命令。
- 在 GUI 中配置全局 rclone VFS 缓存选项。
- 在连接卡片中显示挂载状态、容量、日志和常用操作。
- 在主窗口中批量挂载或批量取消挂载全部已保存连接。
- 通过 GitHub Actions 构建 Windows、macOS、Linux 三个平台的单文件可执行程序。

## 运行依赖

SSH MountMate 的 Release 构建会内置目标平台的官方 rclone 二进制。如果内置 rclone 不可用，程序可以下载官方 rclone zip 到应用自己的本地 bin 目录，并使用这份托管副本。Windows 上也可以回退使用 winget。

Windows：

- Windows 10 或 11
- 内置 rclone，或源码运行时的托管/系统 rclone
- WinFsp
- OpenSSH Client

Windows 依赖可复制命令：

WinFsp 可以直接从 https://winfsp.dev/rel/ 下载。如果当前网络下 winget 可用，也可以使用下面的命令：

```powershell
winget install --id WinFsp.WinFsp -e
powershell -NoProfile -ExecutionPolicy Bypass -Command "Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0"
```

macOS：

- 内置 rclone，或源码运行时的托管/系统 rclone
- macFUSE
- OpenSSH Client

macOS 重要提示：SSH MountMate Release 构建会使用内置的官方 rclone，通常不需要用户安装 Homebrew rclone。如果你手动覆盖 rclone 或从源码运行，不要使用 Homebrew 安装的 `rclone` 做挂载。Homebrew 版 rclone 在 macOS 上不能执行 `rclone mount`，请改用 rclone 官方二进制：

```bash
curl https://rclone.org/install.sh | sudo bash
```

SSH MountMate 的备用依赖安装器在 macOS 上也会使用官方 rclone zip，并保存到应用用户数据目录；rclone 本身不需要 `sudo`。

macOS 挂载仍然需要 macFUSE，macFUSE 可以直接用 Homebrew Cask 安装：

```bash
brew install --cask macfuse
```

安装 macFUSE 后，macOS 可能要求在 `System Settings -> Privacy & Security` 中允许系统扩展。如果出现提示，允许后再重新尝试挂载。

如果 macOS 因为程序未公证而阻止打开，解压后可以移除 quarantine 属性：

```bash
sudo xattr -r -d com.apple.quarantine /path/to/SSHMountMate*
```

Linux：

- 内置 rclone，或源码运行时的托管/系统 rclone
- FUSE 支持，通常是 `fuse3`
- OpenSSH Client

SSH MountMate 会读取 `/etc/os-release` 识别 Linux 发行版，并在程序里优先显示匹配的 FUSE/OpenSSH 安装命令。主要分类是：

- Debian 系：Debian、Ubuntu、Linux Mint、Pop!_OS
- Fedora/RHEL 系：Fedora、RHEL、CentOS Stream、Rocky Linux、AlmaLinux
- Arch 系：Arch Linux、Manjaro、EndeavourOS
- openSUSE/SUSE 系：openSUSE Leap、Tumbleweed、SLES

<details>
<summary>完整 Linux 依赖命令</summary>

```bash
# Debian 系：Debian、Ubuntu、Linux Mint、Pop!_OS
sudo apt update && sudo apt install -y fuse3 openssh-client

# Fedora/RHEL 系：Fedora、RHEL、CentOS Stream、Rocky Linux、AlmaLinux
sudo dnf install -y fuse3 openssh-clients

# Arch 系：Arch Linux、Manjaro、EndeavourOS
sudo pacman -S --needed fuse3 openssh

# openSUSE/SUSE 系：openSUSE Leap、Tumbleweed、SLES
sudo zypper install -y fuse3 openssh
```

</details>

Settings 页面里的 `检查依赖` 会按当前平台显示挂载层依赖：`WinFsp`、`macFUSE` 或 `FUSE`。如果 macOS/Linux 缺系统级依赖，`安装缺失依赖` 会打开可复制命令，而不是静默修改系统。

## 内置和托管 rclone

Release 构建会把 rclone 内置进可执行文件。构建时，SSH MountMate 会根据构建 runner 的平台和 CPU 架构下载官方 rclone zip，并用 PyInstaller 嵌入解压出的二进制。

如果没有可用的内置 rclone，SSH MountMate 仍然可以根据当前平台和 CPU 架构拼出官方 zip 下载地址：

```text
https://downloads.rclone.org/rclone-current-<platform>-<arch>.zip
```

其中平台字段是 `windows`、`osx` 或 `linux`。架构字段通常是 Intel/AMD 64 位机器的 `amd64`，或 Apple Silicon/AArch64 机器的 `arm64`。托管的 `rclone` 副本会保存到 Windows 的 `%LOCALAPPDATA%\SSHMountMate\bin`、macOS 的 `~/Library/Application Support/SSHMountMate/bin`，以及 Linux 的 `${XDG_DATA_HOME:-~/.local/share}/ssh-mountmate/bin`。后续启动时会优先使用这些托管副本，而不是系统 PATH 里的 rclone。

远端服务器默认按 Linux SSH/SFTP 服务器处理。

## 下载

在 GitHub Release 中下载对应平台的包：

- `SSHMountMate-windows-x64.zip`
- `SSHMountMate-windows-arm64.zip`
- `SSHMountMate-macos-x64.zip`
- `SSHMountMate-macos-arm64.zip`
- `SSHMountMate-linux-x64.zip`
- `SSHMountMate-linux-arm64.zip`

这些发布包由 GitHub Actions 从同一份 Python 代码构建。

每个 release zip 中只包含对应平台的可执行文件。内置第三方声明可以在 Settings 页面查看，或执行：

```bash
SSHMountMate --licenses
```

程序更新可以在 Settings -> 检查程序更新 中查看，也可以通过命令行查看：

```bash
SSHMountMate --check-update
```

检查更新会读取 GitHub 最新 Release，并显示当前平台和 CPU 架构对应的下载包。

判断 CPU 架构：

```powershell
# Windows
$env:PROCESSOR_ARCHITECTURE
```

```bash
# macOS / Linux
uname -m
```

`AMD64` / `x86_64` 选择 `x64` 包，`ARM64` / `arm64` / `aarch64` 选择 `arm64` 包。Intel Mac 请下载 `SSHMountMate-macos-x64.zip`，Apple Silicon Mac 请下载 `SSHMountMate-macos-arm64.zip`。

## 快速开始

1. 安装上面列出的系统依赖。
2. 确认普通 SSH 可以登录：

   ```bash
   ssh your-host
   ```

3. 启动 `SSHMountMate`。
4. 点击 `Add config`。
5. 选择：
   - `SSH config`：从已有 SSH Host 中选择，并自动填充默认值。
   - `SSH config (batch)`：选择一个 SSH config 文件，预览后批量导入其中全部具体 `Host`。
   - `SAI cluster`：从 SAI 预设开始。HostName 和端口会预填，只需填写用户名和密钥文件。
   - `Manual`：手动填写主机、用户名、端口和认证信息。
6. 选择远端路径。默认基准目录是 `$HOME`。
7. 如果默认连接方式不适合，可以选择连接方式。
8. 保存后，在连接卡片上点击挂载按钮。

Windows 上 `Auto` 挂载点会自动选择可用盘符。macOS 和 Linux 上默认使用每个连接自己的挂载目录。也可以手动输入自定义挂载路径。

挂载点规则：

- Windows 盘符如 `Z:` 必须未被占用。
- Windows 文件夹挂载点必须是绝对路径。父目录必须存在，目标文件夹本身不能已存在。
- macOS/Linux 自定义挂载点必须是绝对路径，或以 `~` 开头。
- macOS/Linux 自定义挂载点文件夹不存在时会自动创建。
- 已经是挂载点的 macOS/Linux 路径会被拒绝，避免覆盖另一个文件系统。

## SSH Config 导入

SSH MountMate 会读取 OpenSSH config 中具体的 `Host` 条目。选择后会自动填充：

- 名称
- 主机/IP
- 用户名
- 端口
- 密钥文件

导入后，连接会作为可编辑的 rclone SFTP 配置保存。实际挂载行为由 GUI 中看到的字段决定，而不是隐藏地实时调用某条 SSH 命令。

批量导入会使用用户选择的 config 文件，并通过 OpenSSH 的 `ssh -F <config> -G <host>` 行为解析每个 Host。这样可以复用 OpenSSH 的 Include 和默认值处理，同时仍然保存为普通可编辑的 SSH MountMate 连接。

批量导入时，重复项会在预览中标记并跳过：

- `SAME`：SSH `Host` 名和 HostName/User/Port 都相同。
- `SAME HOST`：SSH `Host` 名相同，但解析后的目标不同。
- `SAME TARGET`：SSH `Host` 名不同，但 HostName/User/Port 相同。

手动和 SAI 预设连接也可以写入应用托管的 SSH config。SAI 的默认配置名称和 SSH `Host` 是 `SAI-<用户名>`，`HostName` 是 `c1.sai.ai-4s.com`，`Port` 是 `12022`。SSH MountMate 会在需要时创建 `~/.ssh`，向 `~/.ssh/config` 添加下面的 Include，并把每个托管 Host 写到独立文件：

```sshconfig
Include ~/.ssh/ssh-mountmate.d/*.conf
```

如果启用 `复制密钥到 ~/.ssh`，选中的私钥会被复制到 `~/.ssh`，挂载配置和生成的 SSH config 都会使用复制后的 `IdentityFile` 路径。密码和密钥短语不会写入 SSH config。

## 连接方式

每个已保存连接都可以选择两种方式之一：

- `rclone native SFTP`：默认方式。rclone 自己处理 SSH/SFTP，可以使用通过 rclone obscure 保存的密码或密钥短语。
- `OpenSSH`：rclone 调用系统 `ssh` 命令。适合 `ProxyJump`、`ProxyCommand`、复杂 `Include`、系统 ssh-agent 等 OpenSSH 能力。

选择 `OpenSSH` 时，SSH MountMate 不会保存或传递密钥短语给 `ssh`。带短语的密钥需要先加入 agent：

```bash
ssh-add ~/.ssh/id_ed25519
```

macOS 上如可用，建议使用 Keychain：

```bash
ssh-add --apple-use-keychain ~/.ssh/id_ed25519
```

## 密码和密钥短语

密码和密钥短语会通过：

```bash
rclone obscure
```

转换后写入 SSH MountMate 私有的 rclone 配置。这样可以避免明文保存，但这不是强加密；本机用户账号和配置目录仍应视为敏感数据。

## 主机指纹校验

SSH MountMate 会尽量启用 rclone 的 host key 校验。

对于 rclone SFTP remote，程序会维护自己的 `known_hosts` 文件，并用 `ssh-keyscan` 按目标 host 和 port 刷新服务器返回的 host key。这可以避免一种常见情况：OpenSSH 可以连接，但用户 `~/.ssh/known_hosts` 里只保存了一种 key，rclone 选择了另一种 key 后拒绝连接。

如果无法扫描 host key，程序会回退使用用户默认的 OpenSSH `known_hosts` 文件。

## 容量显示

对已挂载连接，SSH MountMate 会在连接卡片上显示已用容量和总容量。对于 Lustre 路径，程序会优先用 `lfs project -d` 读取远端目录的 project ID，再用 `lfs quota -p` 读取 project quota。如果路径不在 Lustre 上、远端没有 `lfs`，或该 project 没有非零 hard block limit，则回退使用 `rclone about`。

## 设置

Settings 页面包含：

- 依赖检查
- 程序更新检查
- 挂载日志
- 语言选择
- Windows/macOS 登录挂载
- rclone VFS 缓存目录
- VFS 缓存模式
- 最大缓存大小
- 最大缓存寿命
- 最小剩余空间
- 写回延迟
- 目录缓存时间
- 读取缓冲大小
- 批量挂载并行数
- 批量取消挂载并行数

每个选项在 GUI 中都有鼠标悬停说明。

在 macOS 上，登录挂载选项会在 `~/Library/LaunchAgents/` 下为每个配置写入用户级 LaunchAgent 文件。每个任务会调用 SSH MountMate 的无界面 `--mount-id` 入口，在用户登录后挂载对应配置。

## 从源码构建

需要 Python 3.10 或更新版本。

在仓库根目录执行：

```bash
python -m pip install -e ".[build]"
python build/build_local.py
```

生成的可执行文件位于：

```text
dist/
```

PyInstaller 只能稳定地为当前操作系统构建。要生成三平台产物，请使用 GitHub Actions 或分别在对应系统上构建。

## 开发

从源码启动 GUI：

```bash
python -m pip install -e .
python -m ssh_mountmate
```

常用检查：

```bash
python -m py_compile $(find src build -name '*.py' -print) launcher.py
python -m ssh_mountmate --version
python -m ssh_mountmate --install-help
python -m ssh_mountmate --licenses
```

## 授权

SSH MountMate 的应用代码使用 MIT License，详见 `LICENSE`。

Release 构建会内置 rclone。rclone 使用 MIT License，详见 `THIRD_PARTY_NOTICES.md`、`licenses/rclone-COPYING.txt`，或应用 Settings -> 查看许可证窗口。

内置的 Noto Sans CJK SC 字体使用 SIL Open Font License，详见 `src/ssh_mountmate/assets/fonts/LICENSE-Noto-CJK.txt`。
