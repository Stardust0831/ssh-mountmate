#!/usr/bin/env bash
set -euo pipefail

source_package_root="$(realpath "${1:?packaged SSH MountMate root is required}")"
test_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/ssh-mountmate-mount-e2e-XXXXXX")"
package_root="${test_root}/install/SSHMountMate"
mkdir -p "$(dirname "${package_root}")"
cp -a "${source_package_root}" "${package_root}"
binary="${package_root}/SSHMountMate"
rclone=""
server_rclone="${test_root}/server-rclone"
server_user="mountmate"
server_password="test-only-password"
remote_root="${test_root}/remote"
mountpoint="${test_root}/mount"
secondary_mount="${test_root}/mount-projects"
secondary_id="local-sftp--mount-0123456789abcdef0123456789abcdef"
server_pid=""

allocate_loopback_port() {
  python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
}

start_server() {
  local host_key="${1:?host key is required}"
  if [[ -z "${port}" ]]; then
    port="$(allocate_loopback_port)"
  fi
  for _ in $(seq 1 20); do
    "${server_rclone}" --cache-dir "${test_root}/server-cache" \
      --log-file "${test_root}/sftp-server.log" -vv \
      serve sftp "${remote_root}" --addr "127.0.0.1:${port}" \
      --user "${server_user}" --pass "${server_password}" --key "${host_key}" \
      --dir-cache-time 0s --poll-interval 0 &
    server_pid=$!
    for _ in $(seq 1 50); do
      if ! kill -0 "${server_pid}" 2>/dev/null; then
        break
      fi
      if ss -H -ltn "sport = :${port}" | grep -q .; then
        return 0
      fi
      sleep 0.1
    done
    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
    server_pid=""
    port="$(allocate_loopback_port)"
    config_file="${XDG_CONFIG_HOME}/rsshmount/servers.json"
    if [[ -f "${config_file}" ]]; then
      jq --arg port "${port}" 'map(.port = $port)' "${config_file}" >"${config_file}.tmp"
      mv "${config_file}.tmp" "${config_file}"
    fi
  done
  echo 'failed to start the SFTP server on an available loopback port' >&2
  return 1
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
    "${binary}" --unmount-all >/dev/null 2>&1 || true
  fi
  for local_mount in "${secondary_mount}" "${mountpoint}"; do
    if mountpoint -q "${local_mount}"; then
      fusermount3 -u "${local_mount}" 2>/dev/null || sudo umount "${local_mount}" 2>/dev/null || true
    fi
    if mountpoint -q "${local_mount}"; then
      echo "Leaving fixture in place because a mount is still active: ${local_mount}" >&2
      return
    fi
  done
  if [[ -n "${server_pid}" ]]; then
    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
  fi
  rm -rf "${test_root}"
}
trap cleanup EXIT

mkdir -p "${remote_root}" "${mountpoint}" "${test_root}/home"
export CARGO_HOME="${CARGO_HOME:-${HOME}/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-${HOME}/.rustup}"
export HOME="${test_root}/home"
export XDG_CONFIG_HOME="${test_root}/config"
export XDG_CACHE_HOME="${test_root}/cache"
export XDG_STATE_HOME="${test_root}/state"
export XDG_DATA_HOME="${test_root}/data"

test -x "${binary}"
rclone="$("${binary}" --rclone-path)"
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

printf '%s\n' 'initial remote content' >"${remote_root}/initial.txt"
first_host_key="${test_root}/host-key-first"
second_host_key="${test_root}/host-key-second"
ssh-keygen -q -t ecdsa -b 256 -N '' -f "${first_host_key}"
ssh-keygen -q -t ecdsa -b 256 -N '' -f "${second_host_key}"
chmod 600 "${first_host_key}" "${second_host_key}"

port=""
start_server "${first_host_key}"

config_dir="${XDG_CONFIG_HOME}/rsshmount"
mkdir -p "${config_dir}"
awk -v marker="[127.0.0.1]:${port}" '{ print marker, $1, $2 }' "${first_host_key}.pub" >"${config_dir}/known_hosts"
SSH_MOUNTMATE_HOST_KEY_TEST_PORT="${port}" \
SSH_MOUNTMATE_HOST_KEY_TEST_PUBLIC="$(cat "${first_host_key}.pub")" \
  cargo test --package mountmate-core host_key::tests::live_host_key_probe --all-features -- --ignored --exact --test-threads=1
password_obscured="$("${rclone}" obscure "${server_password}")"
jq -n \
  --arg user "${server_user}" \
  --arg port "${port}" \
  --arg password "${password_obscured}" \
  --arg mountpoint "${mountpoint}" \
  --arg secondary_mount "${secondary_mount}" \
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
    mounts: [{id: "0123456789abcdef0123456789abcdef", remote_path: "projects", mountpoint: $secondary_mount}],
    cache_mode: "full"
  }]' >"${config_dir}/servers.json"
jq -n '{
  settings_schema_version: 8,
  vfs_cache_mode: "full",
  vfs_cache_max_age: "30m",
  vfs_write_back: "10m",
  dir_cache_time: "5m",
  auto_show_transfers: false,
  auto_check_updates: false,
  language: "en"
}' >"${config_dir}/settings.json"

mkdir -p "${remote_root}/projects"
printf '%s' 'secondary remote content' >"${remote_root}/projects/secondary.txt"
"${binary}" --mount-all
mountpoint -q "${secondary_mount}"
test "$(cat "${secondary_mount}/secondary.txt")" = 'secondary remote content'
secondary_state="${XDG_STATE_HOME}/rsshmount/${secondary_id}.json"
secondary_pid="$(jq -r .pid "${secondary_state}")"
"${binary}" --refresh-path "${secondary_mount}" >"${test_root}/secondary-refresh"
grep -F 'Remote verified:' "${test_root}/secondary-refresh"
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

test "$(jq -r .pid "${secondary_state}")" = "${secondary_pid}"
kill -0 "${secondary_pid}"
test "$(cat "${secondary_mount}/secondary.txt")" = 'secondary remote content'
"${binary}" --unmount-id "${secondary_id}"
if mountpoint -q "${secondary_mount}"; then
  echo 'secondary mapping remained active after unmount' >&2
  exit 1
fi
mountpoint -q "${mountpoint}"
test "$(cat "${mountpoint}/initial.txt")" = 'initial remote content'

# Keep the upload queued throughout package copying and verification, including
# on slower ARM64 runners. Release it explicitly only after the update test has
# verified that the original mount process and pending upload both survived.
rc_args=(
  --url "http://$(jq -r .rc_addr "${SSH_MOUNTMATE_ACTIVE_STATE_FILE}")"
  --user "$(jq -r .rc_user "${SSH_MOUNTMATE_ACTIVE_STATE_FILE}")"
  --pass "$(jq -r .rc_pass "${SSH_MOUNTMATE_ACTIVE_STATE_FILE}")"
)
queue="$("${rclone}" rc "${rc_args[@]}" vfs/queue)"
upload_id="$(jq -er '.queue[] | select(.name == "upload.bin" and .uploading == false) | .id' <<<"${queue}")"
"${rclone}" rc "${rc_args[@]}" vfs/queue-set-expiry "id=${upload_id}" expiry=-1

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
