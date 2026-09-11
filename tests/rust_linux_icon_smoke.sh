#!/usr/bin/env bash
set -euo pipefail

window_id="${1:?X11 application window ID is required}"
data_home="${2:?isolated XDG data directory is required}"
executable="${3:?application executable is required}"
assets="$(cd "$(dirname "$0")/../assets" && pwd)"

python3 - "$window_id" "$data_home" "$executable" "$assets" <<'PY'
import ctypes
import os
from pathlib import Path
import subprocess
import sys

window_id, data_home, executable, assets = sys.argv[1:]
identity = "io.github.stardust0831.ssh-mountmate"
xlib = ctypes.CDLL("libX11.so.6")
xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
xlib.XOpenDisplay.restype = ctypes.c_void_p
xlib.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
xlib.XInternAtom.restype = ctypes.c_ulong
xlib.XGetWindowProperty.argtypes = [
    ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_long,
    ctypes.c_long, ctypes.c_int, ctypes.c_ulong,
    ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int),
    ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong),
    ctypes.POINTER(ctypes.POINTER(ctypes.c_ubyte)),
]
xlib.XFree.argtypes = [ctypes.c_void_p]
xlib.XCloseDisplay.argtypes = [ctypes.c_void_p]
display = xlib.XOpenDisplay(os.environ["DISPLAY"].encode())
assert display, "could not open the smoke-test display"
data = ctypes.POINTER(ctypes.c_ubyte)()
try:
    actual_type, count, remaining = ctypes.c_ulong(), ctypes.c_ulong(), ctypes.c_ulong()
    actual_format = ctypes.c_int()
    result = xlib.XGetWindowProperty(
        display, int(window_id, 0), xlib.XInternAtom(display, b"_NET_WM_ICON", 0),
        0, 100000, 0, 0, ctypes.byref(actual_type), ctypes.byref(actual_format),
        ctypes.byref(count), ctypes.byref(remaining), ctypes.byref(data),
    )
    assert result == 0 and actual_format.value == 32
    assert count.value == 256 * 256 + 2 and remaining.value == 0
    values = ctypes.cast(data, ctypes.POINTER(ctypes.c_ulong))
    assert (values[0], values[1]) == (256, 256)
    rgba = bytearray()
    for index in range(2, count.value):
        argb = values[index]
        rgba.extend(((argb >> 16) & 255, (argb >> 8) & 255, argb & 255, (argb >> 24) & 255))
    assert bytes(rgba) == (Path(assets) / "ssh-mountmate-logo-256.rgba").read_bytes()
finally:
    if data:
        xlib.XFree(data)
    xlib.XCloseDisplay(display)

wm_class = subprocess.check_output(["xprop", "-id", window_id, "WM_CLASS"], text=True)
assert wm_class.count(identity) == 2, wm_class
launcher = Path(data_home) / "applications" / f"{identity}.desktop"
content = launcher.read_text()
assert f"Icon={identity}\n" in content and f"StartupWMClass={identity}\n" in content
assert Path(executable).name in content
icon = Path(data_home) / "icons/hicolor/256x256/apps" / f"{identity}.png"
assert icon.read_bytes() == (Path(assets) / "ssh-mountmate-logo-256.png").read_bytes()
print("Linux window icon pixels, desktop identity, and installed PNG verified")
PY
