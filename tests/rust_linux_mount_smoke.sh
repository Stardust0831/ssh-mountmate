#!/usr/bin/env bash
set -euo pipefail

source_package_root="$(realpath "${1:?packaged SSH MountMate root is required}")"
test_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/ssh-mountmate-mount-e2e-XXXXXX")"
package_root="${test_root}/install/SSHMountMate"
mkdir -p "$(dirname "${package_root}")"
cp -a "${source_package_root}" "${package_root}"
binary="${package_root}/SSHMountMate"
rclone="${package_root}/bin/rclone"
server_rclone="${test_root}/server-rclone"
server_user="mountmate"
server_password="test-only-password"
remote_root="${test_root}/remote"
mountpoint="${test_root}/mount"
server_pid=""

start_server() {
  local host_key="${1:?host key is required}"
  "${server_rclone}" --cache-dir "${test_root}/server-cache" \
    --log-file "${test_root}/sftp-server.log" -vv \
    serve sftp "${remote_root}" --addr "127.0.0.1:${port}" \
    --user "${server_user}" --pass "${server_password}" --key "${host_key}" \
    --dir-cache-time 0s --poll-interval 0 &
  server_pid=$!
  local server_ready=false
  for _ in $(seq 1 50); do
    if ! kill -0 "${server_pid}" 2>/dev/null; then
      break
    fi
    if ss -H -ltn "sport = :${port}" | grep -q .; then
      server_ready=true
      break
    fi
    sleep 0.1
  done
  test "${server_ready}" == true
}

cleanup() {
  status=$?
  if [[ "${status}" -ne 0 ]]; then
    if [[ -f "${test_root}/sftp-server.log" ]]; then
      printf '%s\n' '--- SFTP server log ---' >&2
      tail -100 "${test_root}/sftp-server.log" >&2 || true
    fi
    if [[ -n "${XDG_STATE_HOME:-}" && -d "${XDG_STATE_HOME}/rsshmount" ]]; then
      printf '%s\n' '--- SSH MountMate logs ---' >&2
      find "${XDG_STATE_HOME}/rsshmount" -maxdepth 1 -type f -name '*.log' \
        -exec tail -100 {} \; >&2 || true
    fi
  fi
  if [[ -x "${binary}" ]]; then
    "${binary}" --unmount-id local-sftp >/dev/null 2>&1 || true
  fi
  if mountpoint -q "${mountpoint}"; then
    fusermount3 -u "${mountpoint}" 2>/dev/null || sudo umount "${mountpoint}" 2>/dev/null || true
  fi
  if [[ -n "${server_pid}" ]]; then
    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
  fi
  rm -rf "${test_root}"
}
trap cleanup EXIT

test -x "${binary}"
test -x "${rclone}"
cp "${rclone}" "${server_rclone}"
chmod 755 "${server_rclone}"
if [[ ! -c /dev/fuse ]]; then
  sudo modprobe fuse || true
fi
if [[ ! -c /dev/fuse ]]; then
  sudo mknod -m 666 /dev/fuse c 10 229
fi
test -c /dev/fuse
sudo chmod a+rw /dev/fuse

mkdir -p "${remote_root}" "${mountpoint}" "${test_root}/home"
printf '%s\n' 'initial remote content' >"${remote_root}/initial.txt"
first_host_key="${test_root}/host-key-first"
second_host_key="${test_root}/host-key-second"
ssh-keygen -q -t ecdsa -b 256 -N '' -f "${first_host_key}"
ssh-keygen -q -t ecdsa -b 256 -N '' -f "${second_host_key}"
chmod 600 "${first_host_key}" "${second_host_key}"

port=""
for candidate in $(seq 42000 42100); do
  if ! ss -H -ltn "sport = :${candidate}" | grep -q .; then
    port="${candidate}"
    break
  fi
done
test -n "${port}"
start_server "${first_host_key}"

export HOME="${test_root}/home"
export XDG_CONFIG_HOME="${test_root}/config"
export XDG_CACHE_HOME="${test_root}/cache"
export XDG_STATE_HOME="${test_root}/state"
export XDG_DATA_HOME="${test_root}/data"
config_dir="${XDG_CONFIG_HOME}/rsshmount"
mkdir -p "${config_dir}"
password_obscured="$("${rclone}" obscure "${server_password}")"
jq -n \
  --arg user "${server_user}" \
  --arg port "${port}" \
  --arg password "${password_obscured}" \
  --arg mountpoint "${mountpoint}" \
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
  }]' >"${config_dir}/servers.json"
jq -n '{
  settings_schema_version: 8,
  vfs_cache_mode: "full",
  vfs_cache_max_age: "30m",
  vfs_write_back: "90s",
  dir_cache_time: "5m",
  auto_show_transfers: false,
  auto_check_updates: false,
  language: "en"
}' >"${config_dir}/settings.json"

"${binary}" --mount-id local-sftp
mountpoint -q "${mountpoint}"
test "$(cat "${mountpoint}/initial.txt")" = 'initial remote content'

find "${mountpoint}" -maxdepth 1 -mindepth 1 -printf '%f\n' | sort >"${test_root}/before-refresh"
printf '%s\n' 'created outside the mount' >"${remote_root}/remote-new.txt"
refresh_output="$("${binary}" --refresh-path "${mountpoint}")"
grep -F 'Remote verified:' <<<"${refresh_output}"
for _ in $(seq 1 50); do
  if [[ -f "${mountpoint}/remote-new.txt" ]]; then
    break
  fi
  sleep 0.1
done
test "$(cat "${mountpoint}/remote-new.txt")" = 'created outside the mount'

dd if=/dev/zero of="${mountpoint}/upload.bin" bs=1M count=8 conv=fsync status=none
queued_output="$("${binary}" --refresh-id local-sftp)"
grep -F 'local file(s) are still waiting to upload' <<<"${queued_output}"

export SSH_MOUNTMATE_ACTIVE_PACKAGE_ROOT="${package_root}"
export SSH_MOUNTMATE_ACTIVE_STATE_FILE="${XDG_STATE_HOME}/rsshmount/local-sftp.json"
cargo test --package mountmate-core --test packaged_update --all-features \
  packaged_update_preserves_real_active_mount -- \
  --ignored --exact --test-threads=1

for _ in $(seq 1 1200); do
  if [[ -f "${remote_root}/upload.bin" ]] \
    && [[ "$(stat -c %s "${remote_root}/upload.bin")" -eq $((8 * 1024 * 1024)) ]]; then
    break
  fi
  sleep 0.1
done
test "$(stat -c %s "${remote_root}/upload.bin")" -eq $((8 * 1024 * 1024))
test "$(sha256sum "${mountpoint}/upload.bin" | cut -d ' ' -f 1)" = \
  "$(sha256sum "${remote_root}/upload.bin" | cut -d ' ' -f 1)"
completed_output=""
for _ in $(seq 1 50); do
  completed_output="$("${binary}" --refresh-id local-sftp)"
  if ! grep -Fq 'still waiting to upload' <<<"${completed_output}"; then
    break
  fi
  sleep 0.1
done
if grep -Fq 'still waiting to upload' <<<"${completed_output}"; then
  echo 'refresh still reported a queued upload after remote completion' >&2
  exit 1
fi

"${binary}" --unmount-id local-sftp
if mountpoint -q "${mountpoint}"; then
  echo 'mountpoint remained active after unmount' >&2
  exit 1
fi
test ! -e "${XDG_STATE_HOME}/rsshmount/local-sftp.json"

known_hosts="${config_dir}/known_hosts"
test -s "${known_hosts}"
known_hosts_before="$(sha256sum "${known_hosts}" | cut -d ' ' -f 1)"
kill "${server_pid}"
wait "${server_pid}" || true
server_pid=""
start_server "${second_host_key}"

set +e
mismatch_output="$("${binary}" --mount-id local-sftp 2>&1)"
mismatch_status=$?
set -e
printf '%s\n' "${mismatch_output}"
if [[ "${mismatch_status}" -eq 0 ]]; then
  echo 'mount unexpectedly accepted a changed SSH host key' >&2
  exit 1
fi
if ! grep -Eiq '((host key|knownhosts).*(mismatch|changed)|key mismatch)' <<<"${mismatch_output}"; then
  echo 'changed SSH host key did not produce an explicit user-facing mismatch' >&2
  exit 1
fi
test "$(sha256sum "${known_hosts}" | cut -d ' ' -f 1)" = "${known_hosts_before}"
if mountpoint -q "${mountpoint}"; then
  echo 'changed-key mount attempt left the mountpoint active' >&2
  exit 1
fi
test ! -e "${XDG_STATE_HOME}/rsshmount/local-sftp.json"
