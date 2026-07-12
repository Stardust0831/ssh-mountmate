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
