# SSH MountMate

SSH MountMate is a cross-platform desktop app for mounting Linux servers as local drives or folders over SSH/SFTP.

It uses rclone for the actual mount operation and provides a small GUI around the parts that are usually tedious: dependency checks, SSH config import, rclone remote generation, mount options, logs, and startup mounts.

## What It Does

- Mount a Linux server directory on Windows, macOS, or Linux.
- Import hosts from your existing OpenSSH config and use them as editable defaults.
- Batch import all concrete hosts from a selected SSH config file.
- Add connections manually with host, username, port, password, key file, and key passphrase.
- Choose the connection method per mount: rclone native SFTP or system OpenSSH.
- Store passwords and key passphrases through `rclone obscure`, not as plain text.
- Check for rclone and platform mount dependencies.
- Download the official rclone zip into an app-managed local bin directory when rclone is missing.
- Fall back to winget for rclone on Windows if the managed download fails.
- Show copyable manual install commands when automatic installation cannot finish.
- Configure global rclone VFS cache options in the GUI.
- Show mount status, capacity usage, logs, and common actions per connection.
- Mount or unmount all saved connections from the main window.
- Build single-file executables for Windows, macOS, and Linux with GitHub Actions.

## Requirements

SSH MountMate does not bundle rclone. If rclone is missing, the app can download the official rclone zip into its own local bin directory and use that managed copy. Windows can also fall back to winget.

Windows:

- Windows 10 or 11
- rclone
- WinFsp
- OpenSSH Client

macOS:

- rclone
- macFUSE
- OpenSSH Client

Important macOS note: do not use the Homebrew `rclone` package for mounting. Homebrew's rclone package cannot run `rclone mount` on macOS. Install the official rclone binary instead:

```bash
curl https://rclone.org/install.sh | sudo bash
```

SSH MountMate's dependency installer also uses the official rclone zip on macOS and stores it inside the app's user data directory, so it does not require `sudo` for rclone itself.

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

- rclone
- FUSE support, usually `fuse3`
- OpenSSH Client

The remote server is assumed to be a Linux server reachable over SSH/SFTP.

## Download

Use the latest GitHub Release and download the package for your platform:

- `SSHMountMate-windows-x64.zip`
- `SSHMountMate-macos-arm64-x64.zip`
- `SSHMountMate-linux-x64.zip`

Release builds are produced by GitHub Actions from the same Python source tree.

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
   - `Manual`: enter host, username, port, and authentication details yourself.
6. Pick a remote path. `$HOME` is the default base.
7. Choose a connection method if the default does not fit.
8. Save, then click the mount button on the connection card.

On Windows, `Auto` mountpoint picks an available drive letter. On macOS and Linux, the app uses a per-connection mount folder by default.

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

## Settings

The Settings window contains:

- dependency checks
- mount log access
- language selection
- Windows startup mount option
- rclone VFS cache root
- VFS cache mode
- max cache size
- max cache age
- minimum free space
- write-back delay
- directory cache time
- read buffer size

Each option has a tooltip in the GUI.

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
```

## License

SSH MountMate's application code is released under the MIT License. See `LICENSE`.

The bundled Noto Sans CJK SC font is distributed under the SIL Open Font License. See `src/ssh_mountmate/assets/fonts/LICENSE-Noto-CJK.txt`.

## 中文说明

SSH MountMate 是一个跨平台桌面程序，用来通过 SSH/SFTP 把 Linux 服务器目录挂载成本地磁盘或本地文件夹。

它底层使用 rclone 完成真正的挂载，GUI 负责处理依赖检查、SSH 配置导入、rclone 配置生成、挂载选项、日志查看和开机挂载等操作。

## 功能

- 在 Windows、macOS、Linux 上挂载 Linux 服务器目录。
- 从已有 OpenSSH config 导入 Host，并作为可编辑默认值。
- 从指定 SSH config 文件中批量导入全部具体 Host。
- 手动添加连接，支持主机、用户名、端口、密码、密钥文件和密钥短语。
- 每个挂载配置都可以选择连接方式：rclone 原生 SFTP 或系统 OpenSSH。
- 密码和密钥短语通过 `rclone obscure` 保存，不明文存储。
- 检查 rclone 和系统挂载依赖。
- 缺少 rclone 时，可下载官方 rclone zip 到应用自己的本地 bin 目录。
- Windows 上如果托管下载失败，可回退通过 winget 安装 rclone。
- 自动安装无法完成时，会显示可复制的手动安装命令。
- 在 GUI 中配置全局 rclone VFS 缓存选项。
- 在连接卡片中显示挂载状态、容量、日志和常用操作。
- 在主窗口中批量挂载或批量取消挂载全部已保存连接。
- 通过 GitHub Actions 构建 Windows、macOS、Linux 三个平台的单文件可执行程序。

## 运行依赖

SSH MountMate 不内置 rclone。缺少 rclone 时，程序可以下载官方 rclone zip 到应用自己的本地 bin 目录，并使用这份托管副本。Windows 上也可以回退使用 winget。

Windows：

- Windows 10 或 11
- rclone
- WinFsp
- OpenSSH Client

macOS：

- rclone
- macFUSE
- OpenSSH Client

macOS 重要提示：不要使用 Homebrew 安装的 `rclone` 做挂载。Homebrew 版 rclone 在 macOS 上不能执行 `rclone mount`。请改用 rclone 官方二进制：

```bash
curl https://rclone.org/install.sh | sudo bash
```

SSH MountMate 的依赖安装器在 macOS 上也会使用官方 rclone zip，并保存到应用用户数据目录；rclone 本身不需要 `sudo`。

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

- rclone
- FUSE 支持，通常是 `fuse3`
- OpenSSH Client

远端服务器默认按 Linux SSH/SFTP 服务器处理。

## 下载

在 GitHub Release 中下载对应平台的包：

- `SSHMountMate-windows-x64.zip`
- `SSHMountMate-macos-arm64-x64.zip`
- `SSHMountMate-linux-x64.zip`

这些发布包由 GitHub Actions 从同一份 Python 代码构建。

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
   - `Manual`：手动填写主机、用户名、端口和认证信息。
6. 选择远端路径。默认基准目录是 `$HOME`。
7. 如果默认连接方式不适合，可以选择连接方式。
8. 保存后，在连接卡片上点击挂载按钮。

Windows 上 `Auto` 挂载点会自动选择可用盘符。macOS 和 Linux 上默认使用每个连接自己的挂载目录。

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

## 设置

Settings 页面包含：

- 依赖检查
- 挂载日志
- 语言选择
- Windows 开机挂载
- rclone VFS 缓存目录
- VFS 缓存模式
- 最大缓存大小
- 最大缓存寿命
- 最小剩余空间
- 写回延迟
- 目录缓存时间
- 读取缓冲大小

每个选项在 GUI 中都有鼠标悬停说明。

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
```

## 授权

SSH MountMate 的应用代码使用 MIT License，详见 `LICENSE`。

内置的 Noto Sans CJK SC 字体使用 SIL Open Font License，详见 `src/ssh_mountmate/assets/fonts/LICENSE-Noto-CJK.txt`。
