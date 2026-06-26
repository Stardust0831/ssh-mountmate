# ssh-mountmate

A SSH/SFTP drive mounter powered by rclone and OpenSSH config.

`ssh-mountmate` 是一个面向 Linux 服务器的 rclone 封装程序。用户只维护 SSH config，程序通过系统 rclone 完成 SFTP 挂载；缺少 rclone 时会引导使用 winget 安装。

早期原型名是 `RSSHMount`，含义是 rclone + SSH mount；当前项目名和 GUI 显示名统一为 `SSH MountMate` / `ssh-mountmate`。

## 设计目标

- 不要求用户手动配置 rclone。
- 复用 OpenSSH config 中的 `Host`、`HostName`、`User`、`Port`、`IdentityFile`、`ProxyJump` 等配置。
- rclone 配置写入私有文件 `~/.config/rsshmount/rclone.conf`，不污染用户已有 rclone。
- 发布包内置 rclone，运行时不依赖系统安装 rclone。
- 支持 Linux 和 Windows 客户端；服务器侧只按 Linux 服务器处理。

## Linux 客户端依赖

- Linux
- Python 3
- OpenSSH client
- FUSE：通常是 `fusermount3`

## Windows 客户端依赖

- Windows 10/11
- Python 3
- Windows OpenSSH client
- WinFsp：rclone mount 在 Windows 上需要它提供文件系统挂载能力

## SSH 配置示例

```sshconfig
Host gpu01
  HostName 10.0.0.12
  User ubuntu
  Port 22
  IdentityFile ~/.ssh/gpu01_ed25519
  IdentitiesOnly yes
  ServerAliveInterval 30
  ServerAliveCountMax 3
  ControlMaster auto
  ControlPath ~/.ssh/cm-%r@%h:%p
  ControlPersist 10m
```

先确认 SSH 可以免交互登录：

```bash
ssh gpu01 true
```

如果私钥有 passphrase，需要提前加入 `ssh-agent`。

这一步也会把服务器主机指纹写入默认 `known_hosts`。`rsshmount` 会把这个文件自动写入私有 rclone 配置，避免 rclone 提示未做 host key validation。

## Linux 使用

在解压后的发布包目录中直接运行：

```bash
./rsshmount doctor
./rsshmount init gpu01
./rsshmount mount gpu01 /home/ubuntu ~/mnt/gpu01
```

卸载：

```bash
./rsshmount umount gpu01 ~/mnt/gpu01
```

## Windows 使用

解压后的发布包目录运行：

```bat
启动 SSH MountMate.cmd
```

也可以继续使用命令行：

```bat
rsshmount.cmd doctor
rsshmount.cmd init gpu01
rsshmount.cmd mount gpu01 /home/ubuntu X:
```

Windows 默认会把 SSH config 解析成 rclone 原生 SFTP 配置，避免挂载期间弹出 `ssh.exe` 命令行窗口。普通的 `HostName`、`User`、`Port`、`IdentityFile` 会自动复用。

如果 SSH config 使用了 `ProxyJump` 或 `ProxyCommand`，程序会回退到外部 `ssh.exe` 方式，因为这类跳板逻辑需要 OpenSSH 自己处理。此时 Windows 仍可能短暂出现 `ssh.exe` 窗口。可以显式选择模式：

```bat
rsshmount.cmd --transport native mount gpu01 /home/ubuntu X:
rsshmount.cmd --transport external mount gpu01 /home/ubuntu X:
```

如果缺少 rclone 或 WinFsp，GUI 会提示并通过 `winget` 弹出命令行窗口安装。安装输出也会写入 `%APPDATA%\rsshmount\install-*.log`，便于排查。WinFsp 安装需要管理员权限；Windows 可能弹出 UAC 确认。

## Windows GUI

GUI 入口：

```bat
启动 SSH MountMate.cmd
```

GUI 支持：

- 启动时检查 rclone、WinFsp、OpenSSH。
- 自动安装缺失依赖。Windows 上缺 rclone 或 WinFsp 时优先用 `winget` 安装，并弹出命令行窗口显示进度，同时写入 `%APPDATA%\rsshmount\install-*.log`；安装成功后窗口保留 5 秒自动关闭，安装失败时窗口保留。
- 通过一个入口添加配置：可选择从 SSH config 导入，或手动填写。
- 从 SSH config 导入时会列出 `Host` 条目，选择后自动填充 Name、IP/Host、用户名、端口、密钥路径，并允许继续手动修改。
- 手动添加服务器：IP/Host、用户名、端口、密钥或密码。
- 管理配置并挂载/卸载，已导入配置可以继续编辑。
- 挂载点支持 `Auto` 和可用盘符下拉选择。
- 远端路径支持 `$HOME` 与 `/` 两种基准目录下拉选择，并可继续填写后续路径；默认是 `$HOME`。
- 每个连接以卡片展示，包含挂载状态、盘符、名称、容量、账号/主机、远端路径。
- 每个连接卡片提供 Mount/Unmount、Open、Edit、Delete 四个图标按钮；不可用动作会变灰。
- 依赖检查和安装入口收纳在 Settings 中；启动时如果发现缺依赖，会主动询问是否安装。
- Settings 支持全局缓存设置：cache root、VFS cache mode、max cache size、max cache age。
- Settings 支持全局开机自启：登录 Windows 后挂载全部配置。
- 为某个配置创建登录后自动挂载任务。
- 关闭窗口会退出 GUI；已启动的 rclone 挂载进程继续在后台运行。

密码不会明文写入 rclone 配置；保存时会通过 `rclone obscure` 转成 rclone 可识别的混淆值。
密钥 passphrase 也按同样方式保存为混淆值，并写入 rclone 的 `key_file_pass`，避免每次挂载重复输入。

正式交付物建议使用单文件 `.exe`。在 Windows 机器上执行：

```powershell
.\scripts\build-windows-exe.ps1
```

生成路径：

```text
dist\SSHMountMate.exe
```

GitHub Actions 也会在 Windows runner 上构建并上传 `SSHMountMate.exe` artifact。

卸载：

```bat
rsshmount.cmd umount gpu01
```

Windows 如果省略挂载点，程序会从 `Z:` 往前自动选择一个空闲盘符：

```bat
rsshmount.cmd mount gpu01 /home/ubuntu
```

同步本地目录到服务器：

```bash
./rsshmount sync ./work gpu01 /home/ubuntu/work
```

如果省略远端路径，默认挂载远端登录用户的 home：

```bash
./rsshmount mount gpu01
```

默认挂载点是：

```text
~/mnt/<Host>
```

## Linux 安装到用户目录

```bash
./install.sh
~/.local/bin/rsshmount doctor
```

也可以指定安装前缀：

```bash
./install.sh /opt/rsshmount
```

## Windows 安装到用户目录

```powershell
.\app\install.ps1
%LOCALAPPDATA%\rsshmount\rsshmount.cmd doctor
```

## 打包 Linux

在项目根目录执行：

```bash
./scripts/package-linux-amd64.sh
```

脚本会下载官方 Linux amd64 rclone，并生成：

```text
dist/rsshmount-linux-amd64.tar.gz
```

发布包结构：

```text
rsshmount-linux-amd64/
  rsshmount
  install.sh
  README.md
  bin/
    rclone
```

## 打包 Windows

```bash
./scripts/package-windows-amd64.sh
```

脚本会生成：

```text
dist/ssh-mountmate-windows.zip
```

发布包结构：

```text
ssh-mountmate-windows/
  启动 SSH MountMate.cmd
  README.md
  app/
    rsshmount.py
    rsshmount_gui.pyw
    rsshmount.cmd
    rsshmount-gui.cmd
    install.ps1
```

## 自定义 SSH config

默认使用 OpenSSH 自己的配置查找逻辑。需要指定配置文件时：

```bash
rsshmount --ssh-config ~/.ssh/work_config init gpu01
rsshmount --ssh-config ~/.ssh/work_config mount gpu01 /data ~/mnt/gpu01
```

## 常见问题

后台挂载失败时看日志：

```bash
cat ~/.local/state/rsshmount/gpu01.log
```

如果需要让其他本地用户访问挂载目录，可加 `--allow-other`，但系统需要在 `/etc/fuse.conf` 中启用 `user_allow_other`。
