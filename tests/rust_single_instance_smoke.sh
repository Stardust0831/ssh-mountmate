#!/usr/bin/env bash
set -euo pipefail

binary="${1:-target/debug/SSHMountMate}"
binary="$(realpath "$binary")"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/ssh-mountmate-ipc-XXXXXX")"
app_pid=""
wm_pid=""

cleanup() {
  status=$?
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  if [[ -n "$wm_pid" ]]; then
    kill "$wm_pid" 2>/dev/null || true
    wait "$wm_pid" 2>/dev/null || true
  fi
  if [[ "$status" != "0" ]]; then
    printf '%s\n' '--- SSH MountMate stdout ---' >&2
    cat "$test_root/gui.stdout" >&2 2>/dev/null || true
    printf '%s\n' '--- SSH MountMate stderr ---' >&2
    cat "$test_root/gui.stderr" >&2 2>/dev/null || true
    printf '%s\n' '--- SSH MountMate event trace ---' >&2
    cat "$test_root/gui.trace" >&2 2>/dev/null || true
    printf '%s\n' '--- Matching X11 windows ---' >&2
    xdotool search --name "SSH MountMate" getwindowname %@ getwindowpid %@ >&2 2>/dev/null || true
  fi
  rm -rf "$test_root"
  trap - EXIT
  exit "$status"
}
trap cleanup EXIT

mkdir -p \
  "$test_root/home" \
  "$test_root/config" \
  "$test_root/data" \
  "$test_root/cache" \
  "$test_root/state" \
  "$test_root/runtime"
chmod 700 "$test_root/runtime"

export HOME="$test_root/home"
export XDG_CONFIG_HOME="$test_root/config"
export XDG_DATA_HOME="$test_root/data"
export XDG_CACHE_HOME="$test_root/cache"
export XDG_STATE_HOME="$test_root/state"
export XDG_RUNTIME_DIR="$test_root/runtime"
export WINIT_UNIX_BACKEND=x11
export WGPU_BACKEND="${WGPU_BACKEND:-gl}"
export LIBGL_ALWAYS_SOFTWARE=1
export NO_AT_BRIDGE=1
export SSH_MOUNTMATE_TRACE_FILE="$test_root/gui.trace"
unset WAYLAND_DISPLAY WAYLAND_SOCKET

openbox >"$test_root/openbox.stdout" 2>"$test_root/openbox.stderr" &
wm_pid=$!
sleep 0.3
kill -0 "$wm_pid" || { echo "Openbox did not stay running" >&2; exit 1; }

"$binary" >"$test_root/gui.stdout" 2>"$test_root/gui.stderr" &
app_pid=$!

state="$XDG_STATE_HOME/rsshmount/app-command.json"
for _ in {1..400}; do
  [[ -s "$state" ]] && break
  sleep 0.05
done
[[ -s "$state" ]] || { echo "App command state was not created" >&2; exit 1; }
[[ "$(stat -c %a "$state")" == "600" ]] || { echo "App command state is not mode 600" >&2; exit 1; }

"$binary" --show-transfers
"$binary" --mount-id missing
[[ -d "/proc/$app_pid" ]] || { echo "GUI exited while handling forwarded commands" >&2; exit 1; }

process_count=0
for process in /proc/[0-9]*; do
  executable="$(readlink "$process/exe" 2>/dev/null || true)"
  if [[ "$executable" == "$binary" ]]; then
    process_count=$((process_count + 1))
  fi
done
[[ "$process_count" == "1" ]] || { echo "Expected one GUI process, found $process_count" >&2; exit 1; }

window_id=""
for _ in {1..400}; do
  window_id="$(xdotool search --onlyvisible --name "SSH MountMate" 2>/dev/null | head -n 1 || true)"
  [[ -n "$window_id" ]] && break
  sleep 0.05
done
[[ -n "$window_id" ]] || { echo "Main window did not become visible" >&2; exit 1; }
[[ "$(xdotool getwindowpid "$window_id")" == "$app_pid" ]] || { echo "Main window belongs to another process" >&2; exit 1; }

xdotool windowclose "$window_id"
for _ in {1..200}; do
  visible="$(xdotool search --onlyvisible --name "SSH MountMate" 2>/dev/null | head -n 1 || true)"
  [[ -z "$visible" ]] && break
  sleep 0.05
done
[[ -z "${visible:-}" ]] || { echo "Main window did not hide after close request" >&2; exit 1; }
[[ -d "/proc/$app_pid" ]] || { echo "GUI exited instead of remaining in the tray" >&2; exit 1; }

"$binary" --show-main
restored_window=""
for _ in {1..200}; do
  restored_window="$(xdotool search --onlyvisible --name "SSH MountMate" 2>/dev/null | head -n 1 || true)"
  [[ -n "$restored_window" ]] && break
  sleep 0.05
done
[[ -n "$restored_window" ]] || { echo "IPC did not restore the hidden main window" >&2; exit 1; }
[[ "$(xdotool getwindowpid "$restored_window")" == "$app_pid" ]] || { echo "Restored window belongs to another process" >&2; exit 1; }
[[ ! -s "$test_root/gui.stdout" ]] || { echo "GUI unexpectedly wrote to stdout" >&2; exit 1; }
if grep -Eq "panicked|ERROR_OUT_OF_HOST_MEMORY" "$test_root/gui.stderr"; then
  cat "$test_root/gui.stderr" >&2
  exit 1
fi

printf 'single-instance background smoke passed: pid=%s window=%s restored=%s mode=600\n' \
  "$app_pid" "$window_id" "$restored_window"
