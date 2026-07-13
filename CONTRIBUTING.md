# Contributing to SSH MountMate

Testing, issues, and pull requests are welcome.

## Testing

Please test the latest release or pre-release on your platform and report problems with:

- operating system and CPU architecture;
- SSH MountMate version;
- rclone, WinFsp, macFUSE, or FUSE status if relevant;
- what you expected to happen;
- what actually happened;
- logs or screenshots when they help explain the issue.

## Issues

Use GitHub Issues for bugs, usability problems, dependency installation problems, and feature requests. Clear reproduction steps are the most useful.

## Pull Requests

Pull requests are welcome. Please keep changes focused and include a short explanation of the behavior change. For UI changes, screenshots are helpful.

Before submitting, run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

---

# 参与 SSH MountMate

欢迎测试、提交 issue 和 pull request。

## 测试

如果你在自己的平台上测试 release 或 pre-release，反馈问题时建议包含：

- 操作系统和 CPU 架构；
- SSH MountMate 版本；
- 相关时提供 rclone、WinFsp、macFUSE 或 FUSE 状态；
- 你预期发生什么；
- 实际发生了什么；
- 有助于说明问题的日志或截图。

## Issue

Bug、易用性问题、依赖安装问题和功能建议都可以通过 GitHub Issues 反馈。清晰的复现步骤最有帮助。

## Pull Request

欢迎提交 PR。请尽量保持改动聚焦，并简单说明行为变化。涉及 UI 的改动建议附截图。

提交前建议运行：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```
