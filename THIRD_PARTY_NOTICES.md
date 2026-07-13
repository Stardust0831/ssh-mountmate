# Third-Party Notices

SSH MountMate release builds bundle the official rclone binary for the target platform.

## rclone

- Project: https://rclone.org/
- Source: https://github.com/rclone/rclone
- License: MIT
- License text: `licenses/rclone-COPYING.txt`

The bundled rclone binary is downloaded from the official rclone download host during the SSH MountMate build:

```text
https://downloads.rclone.org/rclone-current-<platform>-<arch>.zip
```

Platform is `windows`, `osx`, or `linux`; architecture is selected from the build machine, usually `amd64` or `arm64`.

## rfd

- Project: https://github.com/PolyMeilex/rfd
- License: MIT
- License text: `licenses/rfd-LICENSE.txt`

The Rust application links `rfd` to provide native file and folder selection dialogs.

## sys-locale

- Project: https://github.com/1Password/sys-locale
- License: MIT OR Apache-2.0 (distributed under the MIT option)
- License text: `licenses/sys-locale-LICENSE-MIT.txt`

The Rust application links `sys-locale` to select the interface language from the active operating-system locale.

## tray-icon and muda

- Projects: https://github.com/tauri-apps/tray-icon and https://github.com/tauri-apps/muda
- License: MIT OR Apache-2.0 (distributed under the MIT option)
- License text: `licenses/tray-icon-LICENSE-MIT.txt`

The Rust application links `tray-icon` and its `muda` menu library to provide native Windows
system-tray, macOS menu-bar, and Linux AppIndicator controls. Both projects use the same Tauri
Programme MIT license notice included above.

## windows

- Project: https://github.com/microsoft/windows-rs
- License: MIT OR Apache-2.0 (distributed under the MIT option)
- License text: `licenses/windows-LICENSE-MIT.txt`

The Rust application links Microsoft's `windows` crate to expose native Windows shell APIs,
including truthful taskbar transfer progress.

## notify-rust

- Project: https://github.com/hoodie/notify-rust
- License: MIT OR Apache-2.0 (distributed under the MIT option)
- License text: `licenses/notify-rust-LICENSE-MIT.txt`

The Rust application links `notify-rust` to deliver Windows Toast, macOS Notification Center,
and Linux freedesktop desktop notifications.

## tauri-winrt-notification

- Project: https://github.com/tauri-apps/winrt-notification
- License: MIT OR Apache-2.0 (distributed under the MIT option)
- License text: `licenses/tauri-winrt-notification-LICENSE-MIT.txt`

`notify-rust` uses this Tauri library as its Windows Toast backend.
