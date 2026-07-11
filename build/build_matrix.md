# Build Matrix Plan

True cross-compilation is not the primary plan. Build the same source on each
target platform instead.

| Target | Builder | Expected artifacts |
| --- | --- | --- |
| Windows x64 | `windows-latest` | `SSHMountMate-windows-x64.zip`, `SSHMountMate-windows-x64-onedir.zip` |
| Windows arm64 | `windows-11-arm` | `SSHMountMate-windows-arm64.zip`, `SSHMountMate-windows-arm64-onedir.zip` |
| macOS x64 | `macos-15-intel` | `SSHMountMate-macos-x64.zip`, `SSHMountMate-macos-x64-onedir.zip` |
| macOS arm64 | `macos-14` | `SSHMountMate-macos-arm64.zip`, `SSHMountMate-macos-arm64-onedir.zip` |
| Linux x64 | `ubuntu-latest` | `SSHMountMate-linux-x64.zip`, `SSHMountMate-linux-x64-onedir.zip` |
| Linux arm64 | `ubuntu-24.04-arm` | `SSHMountMate-linux-arm64.zip`, `SSHMountMate-linux-arm64-onedir.zip` |

CI installs `.[build]` and runs:

```bash
python build/build_local.py
```

The build script downloads the official rclone zip for the runner platform and
embeds the extracted binary in both PyInstaller onefile and onedir variants. Runtime fallback can still
download rclone into a managed per-user bin directory when the bundled binary is
unavailable.
