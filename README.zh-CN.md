# SSH MountMate

[English](README.md)

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
- 在本地复制窗口结束后继续显示 rclone 真实上传队列和远端传输进度。
- 刷新时核验远端目录，并在连接卡片右键菜单提供刷新和传输操作。
- 在主窗口中批量挂载或批量取消挂载全部已保存连接。
- 通过 GitHub Actions 同时构建 Windows、macOS、Linux 的单文件包和启动更快的 onedir 包。

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

上面的名称是保持兼容的 onefile 包。每个平台还提供一个 `-onedir.zip` 包。请完整解压 onedir 包并运行其中的可执行文件，不要拆散旁边的文件；它无需每次启动解包，通常启动更快。

内置第三方声明可以在 Settings 页面查看，或执行：

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

转换后写入 SSH MountMate 私有的 rclone 配置。这样可以避免明文保存，但这不是强加密。在 macOS 和 Linux 上，SSH MountMate 会把配置文件权限限制为仅文件所有者可读写。本机用户账号和配置目录仍应视为敏感数据。

## 主机指纹校验

SSH MountMate 会尽量启用 rclone 的 host key 校验。

对于 rclone SFTP remote，程序会维护自己的 `known_hosts` 文件。首次连接某个 host 和 port 时，会记录 `ssh-keyscan` 返回的 key；后续连接会固定使用这些 key，不再用网络扫描结果覆盖。

如果无法扫描 host key，程序会回退使用用户默认的 OpenSSH `known_hosts` 文件。

如果 rclone 报 `knownhosts: key mismatch`，SSH MountMate 会停止挂载，不会关闭校验重试。请先向服务器管理员核实新指纹，再从程序托管的 `known_hosts` 文件中删除该主机的旧记录并重试。

## 传输进度和远端刷新

已挂载连接的卡片会显示 rclone 真实的 VFS 上传队列。传输中心会显示正在传输的文件、已传到远端的字节数、速度和等待上传的文件。只有 rclone 报告队列与活动上传均为空时，程序才显示“云端已同步”。仍有上传时取消挂载或退出，程序会先警告。

刷新会清除 VFS 目录缓存、主动重新读取目标目录，并通过直接远端列表进行核验。如果仍有本地写入等待上传，结果会明确说明当前核验的远端快照尚未包含这些文件。

右键连接卡片可以打开目录、刷新、查看传输或日志。Windows 用户还可以在 Settings 中把同样的刷新和传输命令注册到资源管理器右键菜单；这些命令仍由同一个 `SSHMountMate.exe` 处理，不安装辅助程序。右键启动的短生命周期进程会通过带认证的本机 IPC 把请求转发给正在运行的主程序，然后立即退出。

## 容量显示

对已挂载连接，SSH MountMate 会在连接卡片上显示已用容量和总容量。对于 Lustre 路径，程序会优先用 `lfs project -d` 读取远端目录的 project ID，再用 `lfs quota -p` 读取 project quota。如果路径不在 Lustre 上、远端没有 `lfs`，或该 project 没有非零 hard block limit，则回退使用 `rclone about`。

## 设置

Settings 页面包含：

- 依赖检查
- 程序更新检查
- 挂载日志
- 传输中心和 Windows 资源管理器右键菜单注册
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

每个设置项在 GUI 中都有 `?` 帮助图标。鼠标悬停到图标上可以查看该选项的含义。批量挂载和批量取消挂载的并行数由程序固定为 4 和 8，不再作为用户设置项。

在 macOS 上，登录挂载选项会在 `~/Library/LaunchAgents/` 下为每个配置写入用户级 LaunchAgent 文件。每个任务会调用 SSH MountMate 的无界面 `--mount-id` 入口，在用户登录后挂载对应配置。

## 从源码构建

需要 Python 3.10 或更新版本。

在仓库根目录执行：

```bash
python -m pip install -e ".[build]"
python build/build_local.py
```

两种构建产物分别位于：

```text
dist/onefile/
dist/onedir/
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
