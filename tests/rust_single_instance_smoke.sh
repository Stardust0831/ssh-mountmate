#!/usr/bin/env bash
set -euo pipefail

binary="${1:-target/debug/SSHMountMate}"
binary="$(realpath "$binary")"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/ssh-mountmate-ipc-XXXXXX")"
app_pid=""

cleanup() {
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  rm -rf "$test_root"
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
unset WAYLAND_DISPLAY WAYLAND_SOCKET

"$binary" >"$test_root/gui.stdout" 2>"$test_root/gui.stderr" &
app_pid=$!

state="$XDG_STATE_HOME/rsshmount/app-command.json"
for _ in {1..100}; do
  [[ -s "$state" ]] && break
  sleep 0.05
done
[[ -s "$state" ]]
[[ "$(stat -c %a "$state")" == "600" ]]

"$binary" --show-transfers
"$binary" --mount-id missing
[[ -d "/proc/$app_pid" ]]

process_count=0
for process in /proc/[0-9]*; do
  executable="$(readlink "$process/exe" 2>/dev/null || true)"
  if [[ "$executable" == "$binary" ]]; then
    process_count=$((process_count + 1))
  fi
done
[[ "$process_count" == "1" ]]

window_id=""
for _ in {1..100}; do
  window_id="$(xdotool search --onlyvisible --name "SSH MountMate" 2>/dev/null | head -n 1 || true)"
  [[ -n "$window_id" ]] && break
  sleep 0.05
done
[[ -n "$window_id" ]]
[[ "$(xdotool getwindowpid "$window_id")" == "$app_pid" ]]
[[ ! -s "$test_root/gui.stdout" ]]
if grep -Eq "panicked|ERROR_OUT_OF_HOST_MEMORY" "$test_root/gui.stderr"; then
  cat "$test_root/gui.stderr" >&2
  exit 1
fi

printf 'single-instance smoke passed: pid=%s window=%s mode=600\n' "$app_pid" "$window_id"
