#!/usr/bin/env bash
set -euo pipefail

binary="$(realpath "${1:-target/debug/SSHMountMate}")"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/ssh-mountmate-wayland-XXXXXX")"
weston_pid=""
notification_probe_pid=""
app_pid=""

cleanup() {
  status=$?
  for pid in "$app_pid" "$notification_probe_pid" "$weston_pid"; do
    if [[ -n "$pid" ]]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  if [[ "$status" != 0 ]]; then
    printf '%s\n' '--- SSH MountMate trace ---' >&2
    cat "$test_root/gui.trace" >&2 2>/dev/null || true
    printf '%s\n' '--- SSH MountMate stderr ---' >&2
    cat "$test_root/gui.stderr" >&2 2>/dev/null || true
    printf '%s\n' '--- Weston log ---' >&2
    cat "$test_root/weston.log" >&2 2>/dev/null || true
    printf '%s\n' '--- Notification probe stderr ---' >&2
    cat "$test_root/notification-probe.stderr" >&2 2>/dev/null || true
    printf '%s\n' '--- Notification records ---' >&2
    cat "$test_root/notifications.log" >&2 2>/dev/null || true
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
export WAYLAND_DISPLAY=wayland-ssh-mountmate
export WINIT_UNIX_BACKEND=wayland
export GDK_BACKEND=wayland
export WGPU_BACKEND=gl
export LIBGL_ALWAYS_SOFTWARE=1
export NO_AT_BRIDGE=1
export SSH_MOUNTMATE_TRACE_FILE="$test_root/gui.trace"
export SSH_MOUNTMATE_E2E_NATIVE_SMOKE=1
unset DISPLAY WAYLAND_SOCKET

weston --backend=headless-backend.so \
  --socket="$WAYLAND_DISPLAY" \
  --idle-time=0 \
  --renderer=pixman \
  --log="$test_root/weston.log" &
weston_pid=$!
for _ in {1..200}; do
  [[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]] && break
  kill -0 "$weston_pid" 2>/dev/null || break
  sleep 0.05
done
[[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]] || {
  echo "Weston did not create its Wayland socket" >&2
  exit 1
}

notification_probe="$(dirname "$binary")/examples/freedesktop_notification_probe"
[[ -x "$notification_probe" ]] || {
  echo "Freedesktop notification probe is missing: $notification_probe" >&2
  exit 1
}
"$notification_probe" "$test_root/notifications.log" \
  >"$test_root/notification-probe.stdout" \
  2>"$test_root/notification-probe.stderr" &
notification_probe_pid=$!
notification_ready=false
for _ in {1..100}; do
  if gdbus call --session \
    --dest org.freedesktop.DBus \
    --object-path /org/freedesktop/DBus \
    --method org.freedesktop.DBus.NameHasOwner org.freedesktop.Notifications \
    2>/dev/null | grep -q true; then
    notification_ready=true
    break
  fi
  sleep 0.05
done
[[ "$notification_ready" == true ]] || { echo "Wayland notification protocol probe did not start" >&2; exit 1; }

"$binary" >"$test_root/gui.stdout" 2>"$test_root/gui.stderr" &
app_pid=$!
state="$XDG_STATE_HOME/rsshmount/app-command.json"
for _ in {1..400}; do
  [[ -s "$state" ]] && break
  kill -0 "$app_pid" 2>/dev/null || break
  sleep 0.05
done
[[ -s "$state" ]] || { echo "Wayland app command state was not created" >&2; exit 1; }
[[ "$(stat -c %a "$state")" == "600" ]] || { echo "Wayland command state is not mode 600" >&2; exit 1; }

for expected in \
  'main window opened ' \
  'tray unavailable: No StatusNotifierWatcher tray host is available on this desktop' \
  'native notification submitted'; do
  for _ in {1..200}; do
    grep -Fq "$expected" "$test_root/gui.trace" 2>/dev/null && break
    sleep 0.05
  done
  grep -Fq "$expected" "$test_root/gui.trace" || {
    echo "Missing Wayland integration trace: $expected" >&2
    exit 1
  }
done

for expected in \
  'app=SSH MountMate' \
  'summary=SSH MountMate native integration' \
  'body=Native notification delivery is active.'; do
  for _ in {1..200}; do
    grep -Fq "$expected" "$test_root/notifications.log" 2>/dev/null && break
    sleep 0.05
  done
  grep -Fq "$expected" "$test_root/notifications.log" || {
    echo "Missing Freedesktop notification record: $expected" >&2
    exit 1
  }
done

"$binary" --show-transfers
for _ in {1..200}; do
  grep -Fq 'ipc-server received ShowTransfers' "$test_root/gui.trace" 2>/dev/null && break
  sleep 0.05
done
grep -Fq 'ipc-server received ShowTransfers' "$test_root/gui.trace"
kill -0 "$app_pid"

process_count=0
for process in /proc/[0-9]*; do
  executable="$(readlink "$process/exe" 2>/dev/null || true)"
  [[ "$executable" == "$binary" ]] && process_count=$((process_count + 1))
done
[[ "$process_count" == 1 ]] || { echo "Expected one Wayland GUI process, found $process_count" >&2; exit 1; }
if grep -Eq 'panicked|native notification failed' "$test_root/gui.stderr" "$test_root/gui.trace"; then
  echo "Wayland native integration reported a failure" >&2
  exit 1
fi

printf 'Wayland native integration passed: pid=%s mode=600\n' "$app_pid"
