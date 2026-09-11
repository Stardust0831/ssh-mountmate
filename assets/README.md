# Application icons

`ssh-mountmate-logo.svg` is the source artwork. The native application and release
packages use these generated assets:

| Asset | Use |
| --- | --- |
| `ssh-mountmate-logo.png` | 1024 × 1024 desktop/application image |
| `ssh-mountmate-logo-256.png` | Linux hicolor `256x256/apps` icon, using the same pixels as the window icon |
| `ssh-mountmate-logo-256.rgba` | Window icon, 256 × 256, row-major RGBA bytes |
| `ssh-mountmate-logo-32.rgba` | Tray icon, 32 × 32, row-major RGBA bytes |
| `ssh-mountmate-logo.ico` | Windows executable resources at 16, 24, 32, 48, 64, 128, and 256 pixels |
| `ssh-mountmate-logo.icns` | macOS application icon at 16–1024 pixels, including Retina sizes |

These assets were generated with resvg-py 0.5.0 and Pillow 12.3.0. Render the entire
SVG at 2048 × 2048, then downsample to the PNG size with Lanczos. Derive every
smaller image and the RGBA bytes from that PNG with the same filter. The ICO and
ICNS contain PNG frames; the ICNS includes `icp4`, `icp5`, `icp6`, `ic07`–`ic14`.

Preserve the embedded SVG's `viewBox` when rendering: its origin is not zero.
Check the complete SAI mark and the drive below it after regenerating, and update
all generated assets together. No image conversion tools are needed to build or
run the application.
