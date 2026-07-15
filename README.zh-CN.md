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
- 每个挂载配置都可以选择连接方式：rclone 原生 SFTP、系统 OpenSSH，或用于 OAuth/2FA
  登录的交互式共享 SSH。
- 密码和密钥短语通过 `rclone obscure` 保存，不明文存储。
- 检查 rclone 和系统挂载依赖。
- Release 构建内置并校验官方 rclone 二进制。
- 在 GUI 中配置全局 rclone VFS 缓存选项。
- 在连接卡片中显示挂载状态、容量、日志和常用操作。
- 在本地复制窗口结束后继续显示 rclone 真实上传队列和远端传输进度。
- 刷新时核验远端目录，并在连接卡片右键菜单提供刷新和传输操作。
- 在主窗口中批量挂载或批量取消挂载全部已保存连接。
- 通过 GitHub Actions 为 Windows、macOS、Linux 构建 x64 和 arm64 原生 Rust 包。

## 运行依赖

SSH MountMate 的 Release 构建会内置目标平台的官方 rclone 二进制，并在使用前校验。源码构建可以使用显式配置的 rclone、已有托管副本或 `PATH` 中兼容的 rclone。

Windows：

- Windows 10 或 11
- 内置 rclone，或源码构建使用的配置/系统 rclone
- WinFsp
- OpenSSH Client

Windows 依赖可复制命令：

WinFsp 可以直接从 https://winfsp.dev/rel/ 下载。如果当前网络下 winget 可用，也可以使用下面的命令：

```powershell
winget install --id WinFsp.WinFsp -e
powershell -NoProfile -ExecutionPolicy Bypass -Command "Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0"
```

macOS：

- 内置 rclone，或源码构建使用的配置/系统 rclone
- macFUSE 或 FUSE-T
- OpenSSH Client

macOS 重要提示：SSH MountMate Release 构建会使用内置的官方 rclone，通常不需要用户安装 Homebrew rclone。如果你手动覆盖 rclone 或从源码运行，不要使用 Homebrew 安装的 `rclone` 做挂载。Homebrew 版 rclone 在 macOS 上不能执行 `rclone mount`，请改用 rclone 官方二进制：

```bash
curl https://rclone.org/install.sh | sudo bash
```

SSH MountMate 支持 rclone 文档中列出的两种 macOS 挂载层。macFUSE 使用系统扩展，可以直接用 Homebrew Cask 安装：

```bash
brew install --cask macfuse
```

安装 macFUSE 后，macOS 可能要求在 `System Settings -> Privacy & Security` 中允许系统扩展。如果出现提示，允许后再重新尝试挂载。

FUSE-T 是不使用内核扩展的替代方案，它会通过本机 NFSv4 挂载暴露 rclone FUSE 文件系统：

```bash
brew install --cask fuse-t
```

FUSE-T 可能需要在 `System Settings -> Privacy & Security -> Files and Folders` 中允许 `Network Volumes` 访问。由于它使用 NFS 语义，访问时间和修改时间等行为存在已知差异，请先阅读 [rclone 的 FUSE-T 注意事项](https://rclone.org/commands/rclone_mount/#fuse-t-limitations-caveats-and-notes)。SSH MountMate 不会内置 FUSE-T；FUSE-T 公布的二进制许可还要求商业使用或随商业软件捆绑时另行取得商业许可。

v0.4.0 之后的开发版本还会在 macOS 的“设置 -> 挂载方式”中提供“rclone 内置 NFS
（实验性）”。这是手动选择的可选后端，rclone 自带的 NFS 服务会被明确限制在本机回环地址，
不需要 macFUSE 或 FUSE-T。新设置和旧设置迁移仍默认使用 FUSE；修改只影响下一次挂载，
不会中断已有挂载，NFS 启动失败时也不会静默回退。NFS 的文件系统语义、性能和缓存行为可能
与 FUSE 不同。无论该 macOS 设置字段为何值，Windows 仍使用 WinFsp，Linux 仍使用 FUSE3。

如果 macOS 因为程序未公证而阻止打开，解压后可以移除 quarantine 属性：

```bash
sudo xattr -r -d com.apple.quarantine /path/to/SSHMountMate*
```

Linux：

- 内置 rclone，或源码构建使用的配置/系统 rclone
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

Settings 页面里的 `检查依赖` 会报告 rclone、OpenSSH 和当前平台的挂载层依赖：`WinFsp`、`macFUSE / FUSE-T` 或 `FUSE`。SSH MountMate 不会静默修改系统软件包。

## 内置和托管 rclone

Release 工作流会下载目标平台和架构的固定版本官方 rclone，校验 SHA-256 后把它纳入对应平台的发布产物。运行时 SSH MountMate 会再次校验内置摘要，并在应用数据目录生成按内容摘要命名的托管副本。为迁移兼容，显式配置和已有旧版托管副本仍然可用；源码构建最后还可以使用系统 `PATH` 中兼容的 rclone。

远端服务器默认按 Linux SSH/SFTP 服务器处理。

## 下载

在 GitHub Release 中下载对应平台的包：

- `SSHMountMate-windows-x64.zip`
- `SSHMountMate-windows-arm64.zip`
- `SSHMountMate-macos-x64.zip`
- `SSHMountMate-macos-arm64.zip`
- `SSHMountMate-linux-x64.zip`
- `SSHMountMate-linux-arm64.zip`

这些发布包由六个原生 GitHub Actions runner 从 Rust 工作区构建。Windows 和 Linux ZIP 内
是一个嵌入并校验官方 rclone 的可执行文件；Windows 还会内嵌单独校验的官方 Plink，用于
交互式共享。macOS ZIP 内是原生 `SSH MountMate.app`，rclone 和许可证声明位于应用包内。

内置第三方声明可以在 Settings 页面查看，或执行：

```bash
SSHMountMate --licenses
```

程序更新可以在 Settings -> 检查程序更新 中查看，也可以通过命令行查看：

```bash
SSHMountMate --check-update
```

应用内更新器会下载唯一匹配当前平台的 GitHub Release 附件，校验大小和 SHA-256，拒绝不安全的 ZIP 路径，把新的可执行文件或 macOS 应用暂存在当前安装目录旁，并在用户确认后重启 SSH MountMate。新版本通过启动健康握手后才提交更新；超时或失败会恢复并重新启动旧版本。GUI 重启期间，已有 rclone 挂载和上传会继续运行。

自动安装要求 SSH MountMate 已完整解压到固定且当前用户可写的目录。从 ZIP 临时目录直接运行，或 Release 附件没有可信 SHA-256 摘要时，只提供手动更新。可以在设置中关闭后台自动检查。

判断 CPU 架构：

```powershell
# Windows
$env:PROCESSOR_ARCHITECTURE
```

```bash
# macOS / Linux
uname -m
```

`AMD64` / `x86_64` 选择 `x64` 包，`ARM64` / `arm64` / `aarch64` 选择 `arm64` 包。Windows 和
Linux 每个架构只提供一个规范 onefile 包，可执行文件首次使用时会把内嵌工具物化为按内容
摘要命名的受管副本；Windows 包含 rclone 和 Plink，Linux 包含 rclone。macOS 每个架构只
提供一个原生 `.app` 包。发行矩阵固定为六个 ZIP，不再分别提供 onefile 和 onedir 变体。

Intel Mac 选择 `x64`，Apple Silicon 选择 `arm64`；两者都包含原生应用包。

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

每个已保存连接都可以选择三种方式之一：

- `rclone native SFTP`：默认方式。rclone 自己处理 SSH/SFTP，可以使用通过 rclone obscure 保存的密码或密钥短语。
- `OpenSSH`：rclone 调用系统 `ssh` 命令。适合 `ProxyJump`、`ProxyCommand`、复杂 `Include`、系统 ssh-agent 等 OpenSSH 能力。
- `交互式共享 SSH`：第一次点击挂载会打开终端，用于完成 OAuth、2FA、动态密码或其他
  keyboard-interactive 登录。完成登录后保持终端打开，再次点击挂载。rclone 只会收到连接到
  已验证共享会话的非交互命令；一次性响应不会进入 SSH MountMate 的参数或配置。

macOS 和 Linux 使用位于私有状态目录中的 OpenSSH ControlMaster socket。Windows 便携包会
内置固定版本的官方 PuTTY Plink 0.84，并在使用 connection sharing 前校验 SHA-256。Windows
第一阶段仅支持 `手动配置` 直连；导入的 SSH config、`ProxyJump` 和 `ProxyCommand` 转换暂不
支持。关闭登录终端会结束可复用会话，之后的新挂载或依赖该会话的容量查询会要求重新登录；
已经运行的 rclone 挂载不会被程序自动卸载，但在重新建立共享会话前可能报告传输错误。

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

转换后写入 SSH MountMate 的私有配置。这样可以避免明文保存，但该值可以逆向还原，并非强加密；
为了兼容性，这仍是默认方式。在 macOS 和 Linux 上，SSH MountMate 会把配置文件权限限制为仅
文件所有者可读写。本机用户账号和配置目录仍应视为敏感数据。

设置页还提供需要用户手动启用的“系统凭据库”模式。它通过平台原生提供者使用 Windows
Credential Manager、macOS Keychain 或 Linux Secret Service。启用时会先要求确认，在本机
解开已有的 rclone-obscured 值，把密码和私钥短语写入系统凭据库，再逐项回读验证；只有全部
成功后才会从 SSH MountMate 文件中移除这些值。私钥文件本身和一次性 2FA/OAuth token 永远
不会存入凭据库。挂载时只会短暂为 rclone 填充秘密，启动后立即从持久配置清除；清理失败会
停止新挂载，而不会显示一个虚假的“已保护”状态。恢复为 `rclone obscure` 也需要再次明确
确认并执行反向迁移。

交互式共享 SSH 会刻意绕过这两种持久凭据模式。密码、OAuth 响应和轮换的 2FA 验证码只在
OpenSSH 或 Plink 自己的终端中输入。

## 主机指纹校验

SSH MountMate 会尽量启用 rclone 的 host key 校验。

对于 rclone SFTP remote，程序会维护自己的 `known_hosts` 文件。首次连接某个 host 和 port 时，会记录 `ssh-keyscan` 返回的 key；后续连接会固定使用这些 key，不再用网络扫描结果覆盖。

如果无法扫描 host key，程序会回退使用用户默认的 OpenSSH `known_hosts` 文件。

如果 rclone 报 `knownhosts: key mismatch`，SSH MountMate 会停止挂载，不会关闭校验重试。请先向服务器管理员核实新指纹，再从程序托管的 `known_hosts` 文件中删除该主机的旧记录并重试。

## 传输进度和远端刷新

已挂载连接的卡片会显示 rclone 真实的 VFS 上传队列。推荐缓存配置保留 rclone 上游默认的 5 秒写回窗口，让资源管理器或 Finder 先完成关闭文件、重命名和属性更新，再开始远端上传。文件进入队列或开始上传时，对应配置会在右下角显示自己的进度窗；多个正在上传的配置会分别堆叠显示。传输中心继续作为手动查看全部挂载的汇总入口。只有 rclone 报告队列与活动上传均为空时，程序才显示“云端已同步”。仍有上传时取消挂载或退出，程序会先警告。

“同时上传文件数”限制 rclone 同时上传多少个不同的缓存文件。默认值为 4，可选择 8、12，或在 1 到 32 之间自定义；超出数量的文件继续留在本地缓存排队。同一路径被再次修改不会形成可靠的并行版本：rclone 会取消或重新安排该路径的写回，最新的本地内容仍可能覆盖其他写入者的远端修改。

刷新会清除 VFS 目录缓存、主动重新读取目标目录，并通过直接远端列表进行核验。如果仍有本地写入等待上传，结果会明确说明当前核验的远端快照尚未包含这些文件。

右键连接卡片可以打开目录、刷新、查看传输或日志。Settings 可以把刷新和传输命令注册到 Windows 资源管理器、macOS Finder 快速操作，以及 Linux 的 Nautilus、Nemo 或 KDE 文件管理器。这些命令仍由同一个 SSH MountMate 可执行文件处理，不安装辅助程序。文件管理器启动的短生命周期进程会通过带认证的本机 IPC 把请求转发给正在运行的主程序，然后立即退出。

## 容量显示

对已挂载连接，SSH MountMate 会在连接卡片上显示已用容量和总容量。对于 Lustre 路径，程序会优先用 `lfs project -d` 读取远端目录的 project ID，再用 `lfs quota -p` 读取 project quota。如果路径不在 Lustre 上、远端没有 `lfs`，或该 project 没有非零 hard block limit，则回退使用 `rclone about`；当 SSH 配置支持非交互登录时，还会继续尝试远端 `df -Pk`。

## 设置

Settings 页面包含：

- 依赖检查
- 程序更新检查
- 挂载日志
- 传输中心和文件管理器命令注册
- 语言选择
- 登录后自动挂载
- rclone VFS 缓存目录
- VFS 缓存模式
- 最大缓存大小
- 最大缓存寿命
- 最小剩余空间
- 写回延迟
- 目录缓存时间
- 读取缓冲大小
- 同时上传的缓存文件数

每个设置项在 GUI 中都有 `?` 帮助图标。鼠标悬停到图标上可以查看该选项的含义。批量挂载和批量取消挂载的并行数由程序固定为 4 和 8，不再作为用户设置项。

登录启动会使用当前用户的 Windows Run 注册表项、macOS `~/Library/LaunchAgents/` 下的 LaunchAgent，或 Linux XDG autostart 条目，并在登录后调用 Rust 程序的无界面 `--mount-startup-all` 入口。

## 从源码构建

安装 `rust-toolchain.toml` 指定的 Rust 工具链，以及当前操作系统需要的 GUI 开发库。

在仓库根目录执行：

```bash
cargo build --release --package ssh-mountmate
```

可执行文件位于 `target/release/`。发布打包还会下载并校验对应平台的 rclone，因此可分发包应使用 Release 工作流或在对应原生操作系统上生成。

## 开发

从源码启动 GUI：

```bash
cargo run --package ssh-mountmate
```

常用检查：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo run --package ssh-mountmate -- --version
cargo run --package ssh-mountmate -- --licenses
```

## 授权

SSH MountMate 的应用代码使用 MIT License，详见 `LICENSE`。

Release 构建会内置 rclone。rclone 使用 MIT License，详见 `THIRD_PARTY_NOTICES.md`、`licenses/rclone-COPYING.txt`，或应用 Settings -> 查看许可证窗口。

Rust 依赖的第三方声明列在 `THIRD_PARTY_NOTICES.md` 和 `licenses/` 中。
