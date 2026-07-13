#!/usr/bin/env bash
set -euo pipefail

bundle_input="${1:?packaged SSH MountMate application is required}"
bundle_parent="$(cd "$(dirname "$bundle_input")" && pwd)"
bundle="$bundle_parent/$(basename "$bundle_input")"
binary="$bundle/Contents/MacOS/SSHMountMate"
rclone="$bundle/Contents/Resources/bin/rclone"
test_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/ssh-mountmate-mount-e2e-XXXXXX")"
server_user="mountmate"
server_password="test-only-password"
remote_root="$test_root/remote"
mountpoint="$test_root/mount"
server_pid=""

is_mounted() {
  mount | grep -Fq " on $mountpoint ("
}

file_size() {
  stat -f %z "$1"
}

file_digest() {
  shasum -a 256 "$1" | awk '{print $1}'
}

cleanup() {
  status=$?
  if [[ "$status" -ne 0 ]]; then
    if [[ -f "$test_root/sftp-server.log" ]]; then
      printf '%s\n' '--- SFTP server log ---' >&2
      tail -100 "$test_root/sftp-server.log" >&2 || true
    fi
    if [[ -n "${XDG_STATE_HOME:-}" && -d "$XDG_STATE_HOME/rsshmount" ]]; then
      printf '%s\n' '--- SSH MountMate logs ---' >&2
      find "$XDG_STATE_HOME/rsshmount" -maxdepth 1 -type f -name '*.log' \
        -exec tail -100 {} \; >&2 || true
    fi
  fi
  if [[ -x "$binary" ]]; then
    "$binary" --unmount-id local-sftp >/dev/null 2>&1 || true
  fi
  if is_mounted; then
    umount "$mountpoint" 2>/dev/null || sudo umount -f "$mountpoint" 2>/dev/null || true
  fi
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$test_root"
}
trap cleanup EXIT

test -x "$binary"
test -x "$rclone"
test -e /usr/local/lib/libfuse-t.dylib
command -v jq >/dev/null
command -v nc >/dev/null
export DYLD_FALLBACK_LIBRARY_PATH="/usr/local/lib${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"

mkdir -p "$remote_root" "$mountpoint" "$test_root/home"
printf '%s\n' 'initial remote content' >"$remote_root/initial.txt"

port=""
for candidate in {42000..42100}; do
  if ! nc -z 127.0.0.1 "$candidate" >/dev/null 2>&1; then
    port="$candidate"
    break
  fi
done
test -n "$port"

"$rclone" --cache-dir "$test_root/server-cache" \
  --log-file "$test_root/sftp-server.log" -vv \
  serve sftp "$remote_root" --addr "127.0.0.1:$port" \
  --user "$server_user" --pass "$server_password" \
  --dir-cache-time 0s --poll-interval 0 &
server_pid=$!
server_ready=false
for _ in {1..100}; do
  if ! kill -0 "$server_pid" 2>/dev/null; then
    break
  fi
  if nc -z 127.0.0.1 "$port" >/dev/null 2>&1; then
    server_ready=true
    break
  fi
  sleep 0.1
done
[[ "$server_ready" == true ]]

export HOME="$test_root/home"
export XDG_CONFIG_HOME="$test_root/config"
export XDG_CACHE_HOME="$test_root/cache"
export XDG_STATE_HOME="$test_root/state"
config_dir="$XDG_CONFIG_HOME/rsshmount"
mkdir -p "$config_dir"
password_obscured="$("$rclone" obscure "$server_password")"
jq -n \
  --arg user "$server_user" \
  --arg port "$port" \
  --arg password "$password_obscured" \
  --arg mountpoint "$mountpoint" \
  '[{
    id: "local-sftp",
    name: "Local SFTP",
    mode: "manual",
    source: "manual",
    host: "127.0.0.1",
    user: $user,
    port: $port,
    auth: "password",
    password_obscured: $password,
    connection_method: "native",
    remote_path: "",
    mountpoint: $mountpoint,
    cache_mode: "full"
  }]' >"$config_dir/servers.json"
jq -n '{
  settings_schema_version: 8,
  vfs_cache_mode: "full",
  vfs_cache_max_age: "30m",
  vfs_write_back: "5s",
  dir_cache_time: "5m",
  auto_show_transfers: false,
  auto_check_updates: false,
  language: "en"
}' >"$config_dir/settings.json"

"$binary" --mount-id local-sftp
is_mounted
test "$(cat "$mountpoint/initial.txt")" = 'initial remote content'

printf '%s\n' 'created outside the mount' >"$remote_root/remote-new.txt"
refresh_output="$("$binary" --refresh-path "$mountpoint")"
grep -F 'Remote verified:' <<<"$refresh_output"
for _ in {1..100}; do
  [[ -f "$mountpoint/remote-new.txt" ]] && break
  sleep 0.1
done
test "$(cat "$mountpoint/remote-new.txt")" = 'created outside the mount'

dd if=/dev/zero of="$mountpoint/upload.bin" bs=1048576 count=8 2>/dev/null
sync
queued_output="$("$binary" --refresh-id local-sftp)"
grep -F 'local file(s) are still waiting to upload' <<<"$queued_output"

for _ in {1..400}; do
  if [[ -f "$remote_root/upload.bin" ]] \
    && [[ "$(file_size "$remote_root/upload.bin")" -eq $((8 * 1024 * 1024)) ]]; then
    break
  fi
  sleep 0.1
done
test "$(file_size "$remote_root/upload.bin")" -eq $((8 * 1024 * 1024))
test "$(file_digest "$mountpoint/upload.bin")" = "$(file_digest "$remote_root/upload.bin")"

completed_output=""
for _ in {1..100}; do
  completed_output="$("$binary" --refresh-id local-sftp)"
  if ! grep -Fq 'still waiting to upload' <<<"$completed_output"; then
    break
  fi
  sleep 0.1
done
if grep -Fq 'still waiting to upload' <<<"$completed_output"; then
  echo 'refresh still reported a queued upload after remote completion' >&2
  exit 1
fi

"$binary" --unmount-id local-sftp
if is_mounted; then
  echo 'mountpoint remained active after unmount' >&2
  exit 1
fi
test ! -e "$XDG_STATE_HOME/rsshmount/local-sftp.json"

printf 'macOS real mount integration passed: arch=%s bytes=%s\n' \
  "$(uname -m)" "$(file_size "$remote_root/upload.bin")"
