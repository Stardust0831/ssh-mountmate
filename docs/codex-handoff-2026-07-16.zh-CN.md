# SSH MountMate Codex 交接文档 - 2026-07-16

本文档供后续接手的 Codex 使用，也供项目所有者直接阅读。内容包括仓库基线、最近发布与
签名状态、新反馈的问题、建议实现方案、工程边界、测试要求和下一次预发布条件。

## 仓库基线

- 仓库：`Stardust0831/ssh-mountmate`
- 工作区：`/mnt/g/work/agent/rsshmount`
- 当前分支：`feature/macos-nfs-credentials-ssh`
- 编写英文交接时的分支基线：`d48aa4b`（`Harden signed draft publication recovery`）
- 已发布预览版：`v0.4.1-alpha.1`
- 该 Release 的不可变 tag 提交：`be1b917dc5d527db12964d7e163433116b2d973d`
- Release：<https://github.com/Stardust0831/ssh-mountmate/releases/tag/v0.4.1-alpha.1>
- 当前扩展分支尚未合并。用户已授权在需求实现、自审和必要验证完成后执行分支合并；不能
  因为已有合并权限，就提前合并未完成或测试失败的代码。
- 不要移动、删除或复用 `v0.4.1-alpha.1`。下一次预发布应使用新版本和新 tag，通常为
  `v0.4.1-alpha.2`，除非届时代码版本状态要求其他编号。

以下是用户原有的未跟踪文件，禁止编辑、删除或提交，除非用户明确要求：

- `issue-1-reply.md`
- `屏幕截图 2026-06-27 013756.png`
- `屏幕截图 2026-06-27 031441.png`
- `屏幕截图 2026-07-07 020503.png`
- `屏幕截图 2026-07-07 020603.png`
- `屏幕截图 2026-07-07 020631.png`

## 发布与签名状态

`v0.4.1-alpha.1` 是第一个使用 Ed25519 签名的预发布版本。只有以下项目全部一致时，
客户端才能把资产视为可自动安装：

- manifest 签名；
- `key_id`；
- 版本；
- stable/prerelease 通道；
- 当前平台的规范资产名；
- GitHub REST digest；
- 文件大小；
- SHA-256；
- 规范 GitHub 下载 URL。

生产信任根：

- `key_id`：`ed25519-563e14d2c6b880f9`
- 原始 32 字节公钥 SHA-256：
  `563e14d2c6b880f9326f71c809a49474ec74cf74ca2347cc5ac3bf6efad27a2a`
- 生产私钥只存在于 GitHub 受保护 Environment secret 中。
- 按所有者明确选择，没有离线备份。不要随意打印、下载、复制或重新生成该私钥。
- Environment：`production-update-signing`
- 必需审批人：`Stardust0831`
- 临时发布恢复权限已经删除，目前 Environment 只允许 `v*` tag。
- Windows/Linux onedir 目前仍没有自动更新资产；Release 只包含六个规范 onefile/macOS ZIP。
- v0.4.0 本身没有 Ed25519 公钥，因此从 v0.4.0 首次升级到 v0.4.1 仍存在首次信任引导风险。

权威证据：

- 内置生产公钥的六平台分支测试：
  <https://github.com/Stardust0831/ssh-mountmate/actions/runs/29435256461>
- 精确 tag 的生产构建流程：
  <https://github.com/Stardust0831/ssh-mountmate/actions/runs/29442216176>
- 生产运行中的 quality、六平台构建、真实挂载/更新回滚以及临时密钥 release-set 均成功。
- 自动 publish job 因 GitHub draft 的查询和 `untagged-*` URL 语义安全停止。随后使用自动
  回滚 trap 发布，并用同一个 `update-signing verify-published` 实现验证真实公开元数据。
- Release ID 为 `354659949`，包含六个平台 ZIP、`SHA256SUMS.txt`、生产 manifest 和签名，
  共九个带 GitHub REST SHA-256 digest 的资产。
- 永久发布恢复与回滚逻辑位于 `d48aa4b` 之后的分支历史中，不在旧 tag 内；下一次发布
  提交必须包含这些修复。

相关文档：

- `docs/update-signing.md`
- `docs/development-roadmap.md`
- `docs/rust-rewrite-audit.md`
- `release-notes/v0.4.1-alpha.1.md`
- 英文交接：`docs/codex-handoff-2026-07-16.md`

## 用户对下一阶段的授权

用户已经明确授权：完成本文档中的问题、进行自审和必要验证后，可以直接发布新的
prerelease，不需要再次请求发布授权。

该授权不包括：

- 发布 stable 正式版；
- 绕过或削弱 Ed25519 验证；
- 移动已有 tag；
- 修改云端或服务器代码；
- 把私钥、密码、私钥短语、OAuth token 或动态验证码写入日志、文档或构建产物。

下一次预发布仍要正常使用受保护的生产签名 Environment。只要生产密钥不变，不需要再次
确认公钥指纹，但 Environment 审批和全部安全门禁必须保留。

用户也已明确授权：实现、自审和必要 CI 证据完成后，可以执行分支合并。允许合并不等于允许
发布 stable，也不能绕过 prerelease 签名门禁。

## 产品和平台边界

- 保持纯 Rust 产品，继续打包官方 rclone。
- 不修改远端、云端或服务器代码。
- Windows 继续使用 WinFsp，Linux 继续使用 FUSE3。
- macOS 默认仍是 FUSE；rclone 内置 NFS 保持明确的 Experimental 可选项和 loopback-only。
- 不要在挂载后端、传输方式、凭据存储或认证方式之间静默回退。
- 设置变更默认只影响下一次挂载，除非具体需求明确要求其他行为。
- 保持旧设置、服务器配置、挂载状态、日志、rclone 配置和凭据引用兼容。
- 调试和 UI 中不得泄露明文 secret。

## 新反馈与必须实现的行为

### P0：系统凭据库可能导致私钥短语丢失

用户反馈：

- 启用系统凭据库后，私钥短语输入看起来直接变空。
- 挂载时提示私钥短语缺失。
- 切回 `rclone obscure` 后，再输入并保存私钥短语，仍可能无法持久保存。
- 用户看不到迁移执行到了哪一步、是否成功、当前 secret 到底存在哪里。

这是潜在的 secret 丢失/数据丢失问题，必须作为下一版本的首要阻断项。禁止通过静默使用
空 secret 或自动退回 obscure 来掩盖问题。

重点代码：

- `crates/mountmate-core/src/credential.rs`
  - `migrate_server_to_system`
  - `migrate_server_to_obscure`
  - `hydrate_server_from_system`
  - `replace_verified`、rollback、delete
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

现有语义：

- 编辑已有配置时，密码和私钥短语输入框会故意初始化为空。
- 如果已有有效 obscure 值或系统凭据引用，空输入必须表示“保持不变”，不能表示清除。
- 系统凭据通过 `password_credential` / `key_pass_credential` 引用；成功迁移后对应 obscured
  字段会被清空。
- 只有原生 SFTP 会把系统凭据临时 hydrate 成 rclone 所需的 obscure 值。OpenSSH 和
  Interactive 由 SSH 自己处理认证，不应声称会消费程序保存的私钥短语。

必须满足：

1. 优先在真实 Windows Credential Manager 路径复现，再检查 macOS Keychain 和 Linux
   Secret Service；不要未经验证就假设三平台同因。
2. 调试只记录字段“是否存在”和迁移阶段，不记录任何 secret 内容。
3. 迁移到系统凭据库必须是事务式顺序：
   - 本地解开旧 obscure；
   - 写入系统凭据库；
   - 回读并比较；
   - 保存 credential reference；
   - 重新加载并验证配置文件；
   - 最后才清理旧 obscure/rclone secret。
4. 切回 obscure 必须反向执行：
   - 读取系统凭据；
   - 生成 obscure；
   - 保存并重新加载配置；
   - 最后才删除系统凭据。
5. 任意阶段失败时必须保留最后一份可用 secret，绝不能让 obscure 和 reference 同时为空。
6. UI 应显示“已存入系统凭据库”等非敏感状态，并提供明确的替换/清除操作。空输入框
   不能让用户误以为值已经丢失。
7. 只修改其他字段并保存时，必须保留已有 credential reference。
8. 切回 obscure 后输入新短语，必须在重启程序后仍然存在并可挂载。
9. 增加真实迁移、编辑和挂载回归测试；现有低层 native credential round-trip 不够。

### P0：Windows 文件夹挂载点必须位于支持的本地卷

用户使用 `Z:\test\mount` 时出现：

```text
2026/07/16 04:00:08 ERROR : sftp://xujiacheng@c0.sai.ai-4s.com:12022/: Mount failed
2026/07/16 04:00:08 NOTICE: Z:\test\mount: Unmounted rclone mount
2026/07/16 04:00:08 CRITICAL: Fatal error: failed to umount FUSE fs: mount failed
2026/07/16 04:00:20 NOTICE: sftp://xujiacheng@c0.sai.ai-4s.com:12022/: Symlinks support enabled
Cannot set WinFsp-FUSE file system mount point.
The service rclone-492648a3867d has failed to start (Status=c0000277).
```

当前 allocator 只检查 Windows 自定义挂载点的父目录是否存在、目标目录是否不存在，没有
验证底层卷是否是 WinFsp 支持的本地卷。映射网络盘或其他不支持的挂载文件系统会一直执行到
rclone 后才失败。

重点代码：

- `crates/mountmate-core/src/mountpoint.rs`
- `MountpointProbe` / `SystemMountpointProbe`
- `MountpointAllocator::validate_custom`
- `crates/mountmate-core/src/runtime.rs`
- `crates/mountmate-core/src/service.rs`
- `crates/mountmate-platform` 中的 Windows 原生绑定

必须满足：

1. 启动 rclone 之前解析自定义挂载目录所属的 volume/root。
2. 使用 `GetVolumePathNameW`、`GetDriveTypeW` 等 Windows 原生 API。
3. 研究并确认 WinFsp 文件夹挂载点是否要求固定本地卷，以及是否还有文件系统类型限制。
4. 对 UNC、映射网络盘 `DRIVE_REMOTE`、未知/无根卷和其他不支持卷给出明确的本地化预检错误。
5. 预检失败后不得创建目标子目录，也不得启动 rclone。
6. 自动盘符和正常本地文件夹挂载不能回归。
7. 增加 fake probe 单元测试和 Windows 原生测试。

### P0：错误提示闪得太快

Windows SSH-config + Interactive 的不支持提示，以及挂载失败，目前主要写入底部共享
`status`。周期性挂载/传输刷新会很快覆盖它，用户只能看到一闪而过的文字。

必须实现：

- 增加每配置持久 operation error，不再只依赖全局 status。
- 错误应保持到用户关闭、成功重试、打开关联设置/日志或开始明确覆盖它的新操作。
- 错误面板至少包含配置名、简短原因、详情/日志、关闭，以及适用时的重试。
- 轮询类错误不能反复弹模态窗口。
- 不支持的组合应在编辑器中直接禁用并解释，不要等点挂载后才失败。
- 常规 polling 不能清掉最后一次挂载错误。
- 如需持久保存诊断，只能存脱敏信息，不能存 secret。

入口：

- `App::start_mount_operation`
- `Message::MountFinished`
- `localize_service_error`
- connection card 状态和 `server_card_view`
- 会覆盖 `self.status` 的周期性 mount/transfer polling

### P1：明确 SSH config 和 OpenSSH 的权威字段

用户疑问：

- Windows 中给 SSH-config 配置选择 Interactive shared SSH，会快速提示只支持手动直连。
- 有些 SSH config 是简单直连，不含 ProxyJump，但 UI 仍不应暗示显示出的 IP/用户/端口能够
  完整替代 SSH config。
- OpenSSH 传输到底使用 Host 别名，还是界面中的 IP 配置？

当前真实行为：

- SSH-config 来源存在 Host alias 时，OpenSSH 和 macOS/Linux Interactive 使用等效命令：

  ```text
  ssh -F <选中的配置文件路径> <Host别名>
  ```

- 此时 Host alias 和 SSH config 路径是权威输入。
- 界面显示的 host/user/port/identity 是导入时的解析快照，只用于展示和配置记录，不是对
  `Include`、`Match`、`ProxyJump`、`ProxyCommand`、canonicalization、token expansion、
  agent、certificate 等 OpenSSH 行为的完整替代。
- 手动来源使用 OpenSSH 时，界面字段有效，命令等效于：

  ```text
  ssh -l <user> -p <port> [-i <key>] <host>
  ```

- 程序管理的 SSH profile 也以生成的 Host alias 为权威目标。
- Windows Interactive 当前不支持 SSH-config/batch SSH-config，即使该条目只是简单直连。
  原因是 Plink 路径没有完整翻译并验证 OpenSSH config 的语义。

重点代码：

- `crates/mountmate-core/src/interactive_ssh.rs`
  - `windows_direct_connection_supported`
  - `openssh_target_arguments`
- `crates/mountmate-core/src/rclone.rs`
- `crates/mountmate-core/src/ssh.rs`
- `crates/mountmate-core/src/connection.rs`
- `crates/mountmate-app/src/main.rs` 的连接编辑器

UI 要求：

1. Windows 的 SSH-config/batch 来源中直接禁用 Interactive shared SSH，并在选项旁持续说明。
2. SSH-config + OpenSSH/Interactive 时，冻结非权威字段并明确显示：
   - SSH config 路径；
   - Host alias；
   - 安全转义且不含 secret 的命令等效预览；
   - 在可行范围内显示只读的相关配置/解析片段。
3. 必须注明显示片段不是完整语义展开，真正执行的 OpenSSH 命令才是权威行为。
4. 手动 OpenSSH 中保留 host/user/port/key 可编辑，并说明这些字段组成命令。
5. 增加来源和平台相关的可选传输方式、命令构建和 UI 文案测试。

### P1：交互式 SSH 改为程序内部终端，不再弹黑框

现有实现主动启动外部 UI：

- Windows Plink 使用 `CREATE_NEW_CONSOLE`；
- macOS 写入 `.command` 后交给系统打开；
- Linux 写入脚本并启动外部终端模拟器。

用户不希望黑色控制台或外部 Terminal 抢焦点、打断其他操作。SSH/Plink 子进程本身仍然
需要存在，因为 OAuth、2FA、动态验证码、主机密钥确认等都需要交互；目标是“不出现独立
终端窗口”，不是“完全没有子进程”。

推荐架构：

- Windows 使用 ConPTY 或成熟 Rust PTY 库承载 Plink/OpenSSH，不再使用
  `CREATE_NEW_CONSOLE`。
- macOS/Linux 使用 PTY 启动 OpenSSH，并在 SSH MountMate 自己的登录窗口或面板中显示。
- 为了保持单一分发 EXE，可以让同一个二进制支持隐藏子命令，例如：

  ```text
  SSHMountMate.exe --ssh-session-broker <server-id>
  ```

- broker 进程持有 ConPTY/PTY 和 SSH 子进程；主 GUI 通过仅限当前用户、经过认证的本地 IPC
  连接它。运行时存在辅助进程，但发布物仍然只有一个 EXE。
- 当共享 SSH 会话需要在 GUI 隐藏、重建或短暂重启后继续存在时，broker 模型优于严格的
  单进程模型。严格单进程也可以做，但 GUI 退出通常会关闭 PTY 和会话。
- 不能简单改成 `CREATE_NO_WINDOW` 再接普通 stdin/stdout pipe。SSH、Plink、密码提示、
  host-key 确认、OAuth/device login、2FA 和终端控制序列可能要求真实 TTY/PTY。
- 不要实现依赖提示文字猜测的脆弱 prompt parser。
- 使用成熟的 ANSI/VT 解析或终端组件，处理光标、退格、颜色、resize、Unicode、输入法、
  URL、EOF 和进程退出。不要从零实现终端协议。
- 每个配置维护一个程序管理的登录会话，显示状态、输出、重试和关闭语义。
- 关闭终端视图时，如果共享会话仍被挂载依赖，应明确询问是隐藏视图还是终止会话。
- 不能在没有应用内交互通道前直接把进程改成 headless，否则 OAuth/密码/验证码提示会卡死。
- IPC endpoint 必须只允许当前用户；终端 scrollback 要限长，默认不落盘。
- 剪贴板和导出必须由用户显式触发，因为终端内容可能包含 token、用户名和主机信息。

### P1：加固 macOS/Linux 每配置独立 OpenSSH socket

保留现有 per-server control socket 和短路径 fallback，但在创建、检查、连接、清理和复用前
补齐安全验证。

当前只创建 control dir 并设置 `0700`，没有完整验证已经存在的路径是否为 symlink、所有者
是否正确、类型是否正确，也没有在复用前充分验证 socket 对象。

必须检查：

- control dir 是真实目录而非 symlink；
- 所有者是当前用户；
- 权限为 owner-only，通常是 `0700`；
- control socket 是 Unix socket，不是普通文件、symlink 或设备；
- socket 所有者是当前用户；
- socket 不允许 group/world 访问；
- stale cleanup 只删除经过所有者和预期路径身份确认的对象；
- temp fallback 路径执行同样检查；
- 保留 socket 路径长度测试。

使用 `symlink_metadata`、Unix file type 扩展、uid/mode 检查和项目已有的 `rustix` 能力。
增加恶意 symlink、错误所有者抽象、错误类型、宽松权限、owned stale socket 和短路径测试。

### P1：日志窗口要引导选择，并优先打开失败配置

现有日志窗口已经有 selector，但未选择配置时显示通用“暂时没有可显示的日志”，用户会误以为
系统完全没有日志。

必须实现：

- 没有选择配置时，明显提示用户从 selector 选择挂载配置。
- 从错误或卡片进入时，直接选择最近失败/当前配置。
- 挂载错误提供“查看日志”动作并直达对应日志。
- 日志文件不存在时显示预期路径，并区分“从未挂载”“日志尚未启动”“文件不可用”。
- 保留部分文本选择/复制，以及打开时滚动到最新行。
- 不要退回单一全局日志文本。

入口：`open_log_window`、`open_log`、`log_viewer_view`、`MountLogView`、`main.rs` 和
`i18n.rs`。

### P1：首次运行和无配置状态要引导新建配置

没有任何连接时，首页应提供清晰的空状态和“新建配置”动作，直接打开连接编辑器。可以在真正
首次运行时自动打开，但用户主动关闭后不应每次强制弹出。不要增加营销式 landing page。

### P1：必填项用红色星号标识

红色 `*` 必须与真实 validation 条件来自同一语义，避免 UI 和校验规则漂移。至少检查：

- 配置显示名；
- 手动来源的 IP/Host；
- SSH-config 来源的 Host alias 和配置路径；
- 用户名；
- 端口；
- 密码认证且没有保留 secret 时的密码；
- 原生私钥认证所需的私钥；
- 选择自定义挂载点后必须填写的路径。

条件无关或只读字段不要错误标必填。不能只靠颜色表达，label 和无障碍信息也要明确。

### P2：增加常用主题和主题色

当前 `App::theme` 固定返回 `Theme::Dark`。增加可持久化、可迁移的设置，建议最少包括：

- 跟随系统；
- 浅色；
- 深色；
- 少量克制的强调色预设，如蓝、绿、琥珀、紫。

复用 Iced theme API 和现有组件模式。检查普通、focus、disabled、error、progress 和 selection
状态的对比度。不要为每个颜色复制一套互不兼容的组件样式。补设置迁移、序列化、中英文文案
和 UI 状态测试。

### P1：主页“编辑”改为“设置”，挂载时允许只读查看

每个配置卡片当前在不可修改状态会禁用 Edit。改为“设置”/“Settings”，并允许挂载时打开。

- 未挂载且不 busy：正常编辑，Save 可用。
- 已挂载、正在挂载、正在卸载或其他锁定状态：打开同一配置页，但所有输入和选择控件必须
  灰掉且不可产生 mutation message；Save 隐藏或禁用。
- 明确说明需要先卸载，修改才会在下一次挂载生效。
- secret 在只读模式也只能显示“已保存”等状态，不能显示明文。
- Remove/Delete 仍独立保护，不能因为 Settings 可打开而放宽。

## 推荐实现顺序

1. 复现并修复系统凭据迁移/持久化，增加端到端回归测试。
2. 增加持久的每配置错误和日志直达，先提高后续问题的可诊断性。
3. 增加 Windows 本地卷挂载点预检。
4. 让传输方式选项感知平台和配置来源，并明确 SSH-config/OpenSSH 权威字段。
5. 用程序内 PTY UI 替换外部终端；同阶段加固 Unix socket。
6. 实现挂载时只读 Settings，并把主页“编辑”改成“设置”。
7. 实现首次运行空状态和必填星号。
8. 实现主题设置。
9. focused review、完整本地验证、六平台 CI，然后发布下一 prerelease。

用户要求下一次预发布前完成本文档列出的全部项目，包括主题。P0/P1/P2 只表示风险和顺序，
不是允许省略低优先级项目。

## 最低测试要求

至少覆盖：

- 系统凭据库与 obscure 的双向迁移，包括密码和私钥短语；
- 未修改 secret 的编辑保存保留 credential reference；
- 替换系统 secret 后重启仍有效；
- 每个保存/删除边界的 rollback；
- 原生挂载从系统凭据 hydrate；
- Windows 本地固定卷允许，网络盘/UNC/未知卷拒绝；
- 挂载失败和不支持传输方式的错误保持到用户关闭；
- 错误直达日志、未选择日志和日志缺失的引导；
- Windows SSH-config 中 Interactive 在挂载前禁用；
- SSH-config alias/config 路径命令与手动 host/user/port/key 命令；
- 应用内 PTY 生命周期且不创建独立可见控制台；
- Unix control dir/socket 的 owner/type/mode/symlink/stale/short-path；
- 挂载中 Settings 只读且所有 mutation control 禁用；
- 必填星号跟随条件 validation；
- 首次运行空状态；
- theme 迁移、序列化、中英文文案和对比度相关状态；
- stable/prerelease 更新规则和 Ed25519 测试不回归。

有 Rust 工具链时必须执行：

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

开发中运行 focused tests。发布前要求 Windows x64/ARM64、Linux x64/ARM64、macOS x64/ARM64
六个平台权威任务全部通过。macOS FUSE 和 Experimental NFS 真实生命周期必须继续通过。

## 工程与安全规范

- 每次编辑前检查 dirty worktree，保留用户文件和无关修改。
- 搜索优先使用 `rg`/`rg --files`，手工编辑使用 `apply_patch`。
- 禁止 destructive git 命令和 force-move tag。
- 修改范围要聚焦，不捆绑无关重构。
- 使用强类型和现有项目模式，不散落字符串判断。
- PTY、凭据库、Windows volume 检查使用成熟 crate 或原生 API。
- 不自行实现密码算法或终端模拟协议。
- 调试 secret 时只记录 presence、引用标识哈希、阶段和脱敏错误。
- 凭据迁移必须 fail-closed，失败时不能删除最后一份可用 secret。
- 禁止在 Interactive/Native、System/Obscure、NFS/FUSE 或不同挂载点之间静默回退。
- 后台进程必须有明确 owner、identity、lifecycle 和 cleanup 验证。
- 会影响挂载或用户数据的错误必须持久且可操作，不能只闪一下。
- UI 必须准确说明当前来源/传输方式下哪些字段真正生效。
- 不修改云端或服务器代码。

## 最近开发日志

上一阶段完成：

1. 为自动更新实现 Ed25519 manifest 验证和多公钥 registry。
2. 增加 signing CLI、篡改/轮换测试、GitHub digest、严格平台资产和安装门禁。
3. 增加受保护 Environment 生产签名和六平台 Release 聚合。
4. 修复 CI GitHub API rate limit 和 Ubuntu ARM mirror 问题。
5. 在所有者确认公钥后发布 `v0.4.1-alpha.1`。
6. 修复签名 job 缺失 Linux Secret Service build dependency。
7. 手动发布恢复时让 workflow 检出请求的不可变 tag，而不是移动 tag。
8. 处理 GitHub draft 查询和 `untagged-*` URL：发布前预验证、失败自动回滚、公开后真实验证。
9. 永久 workflow 支持显式发现 draft、规范 URL 预验证、公开元数据复验和失败恢复 draft。
10. 删除临时 Environment 分支权限，恢复只允许 `v*` tag。

本次交接阶段没有修改这些新问题对应的产品代码，只增加和完善交接文档。

## 已知剩余风险

- 系统凭据问题尚未定位，可能是真实 secret 丢失。下一版本发布前必须优先解决并取得真实
  native credential store 证据。
- 应用内交互终端是重要的跨平台功能，PTY、focus、resize、encoding、prompt 和生命周期
  工作量不可低估。
- WinFsp 文件夹挂载限制不只是路径存在性，必须验证原生行为。
- SSH config 是动态 OpenSSH 语言，显示的片段不能被宣传为完整行为模型。
- 生产私钥没有离线备份，避免不必要地修改 Environment 或 secret。
- 包仍没有 Authenticode、Apple Developer ID 和 notarization。
- Windows/Linux onedir 不是自动更新资产。
- macOS Interactive 登录真实生命周期证据少于 Linux/Windows。

## 完成和预发布清单

创建下一 tag 前必须确认：

- 本文全部问题已经实现，不只是写文档；
- 凭据迁移有真实 native 证据，不存在删除最后一份 secret 的路径；
- Windows 不支持的挂载点在 rclone 启动前失败；
- 错误和日志可读、可操作；
- 传输 UI 与实际命令权威字段一致；
- Interactive 登录不创建独立黑色控制台；
- Unix socket 通过 owner/type/mode 检查；
- 挂载中 Settings 为只读且灰显；
- 首次引导、必填星号、主题选择完成；
- fmt、Clippy、workspace tests 通过；
- 六平台 CI 通过；
- 自审覆盖三平台回归、secret、进程清理和发布 workflow；
- Cargo/app 版本和 release notes 更新到新的 prerelease；
- annotated tag 指向精确绿色提交；
- 受保护生产签名、draft rollback、公开 metadata 和 Ed25519 验证通过；
- Release 标记为 prerelease，不是 stable；
- 如执行分支合并，只合并已经审阅并通过验证的修改，同时保持本文的 Release/tag 规则；
  合并权限已经获得。
