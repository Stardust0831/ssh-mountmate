#!/usr/bin/env bash
set -euo pipefail

package_root="$(realpath "${1:?packaged SSH MountMate root is required}")"
binary="${package_root}/SSHMountMate"
test_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/ssh-mountmate-mount-e2e-XXXXXX")"
test_user="mountmatee2e"
server_home="${test_root}/server-home"
remote_name="remote"
remote_root="${server_home}/${remote_name}"
mountpoint="${test_root}/mount"
sshd_pid=""
user_created=false

cleanup() {
  status=$?
  if [[ "${status}" -ne 0 ]]; then
    if [[ -f "${test_root}/sshd.log" ]]; then
      printf '%s\n' '--- sshd log ---' >&2
      tail -100 "${test_root}/sshd.log" >&2 || true
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
  if [[ -n "${sshd_pid}" ]]; then
    sudo kill "${sshd_pid}" 2>/dev/null || true
  fi
  if [[ "${user_created}" == true ]]; then
    sudo userdel --remove "${test_user}" >/dev/null 2>&1 || true
  fi
  sudo rm -rf "${test_root}"
}
trap cleanup EXIT

test -x "${binary}"
test -x "${package_root}/bin/rclone"
if [[ ! -c /dev/fuse ]]; then
  sudo modprobe fuse || true
fi
if [[ ! -c /dev/fuse ]]; then
  sudo mknod -m 666 /dev/fuse c 10 229
fi
test -c /dev/fuse
sudo chmod a+rw /dev/fuse

chmod 755 "${test_root}"
sudo useradd --create-home --home-dir "${server_home}" --shell /bin/bash "${test_user}"
user_created=true
sudo passwd --delete "${test_user}" >/dev/null
sudo chmod 755 "${server_home}"
sudo install -d -o "${test_user}" -g "${test_user}" -m 700 "${server_home}/.ssh"
sudo install -d -o "${test_user}" -g "${test_user}" -m 777 "${remote_root}"
mkdir -p "${mountpoint}" "${test_root}/home"
printf '%s\n' 'initial remote content' >"${remote_root}/initial.txt"
ssh-keygen -q -t ed25519 -N '' -f "${test_root}/client-key"
ssh-keygen -q -t ed25519 -N '' -f "${test_root}/host-key"
sudo install -o "${test_user}" -g "${test_user}" -m 600 \
  "${test_root}/client-key.pub" "${server_home}/.ssh/authorized_keys"
chmod 600 "${test_root}/client-key" "${test_root}/host-key"
sudo chown root:root "${test_root}/host-key"

port=""
for candidate in $(seq 42000 42100); do
  if ! ss -H -ltn "sport = :${candidate}" | grep -q .; then
    port="${candidate}"
    break
  fi
done
test -n "${port}"

cat >"${test_root}/sshd_config" <<EOF
Port ${port}
ListenAddress 127.0.0.1
HostKey ${test_root}/host-key
PidFile ${test_root}/sshd.pid
AuthorizedKeysFile .ssh/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
AllowUsers ${test_user}
Subsystem sftp internal-sftp
LogLevel VERBOSE
EOF
sudo mkdir -p /run/sshd
sudo /usr/sbin/sshd -f "${test_root}/sshd_config" -E "${test_root}/sshd.log"
sshd_pid="$(cat "${test_root}/sshd.pid")"

for _ in $(seq 1 50); do
  if ssh -i "${test_root}/client-key" -p "${port}" \
    -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null "${test_user}@127.0.0.1" true 2>/dev/null; then
    break
  fi
  sleep 0.1
done
ssh -i "${test_root}/client-key" -p "${port}" \
  -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null "${test_user}@127.0.0.1" true

export HOME="${test_root}/home"
export XDG_CONFIG_HOME="${test_root}/config"
export XDG_CACHE_HOME="${test_root}/cache"
export XDG_STATE_HOME="${test_root}/state"
export XDG_DATA_HOME="${test_root}/data"
config_dir="${XDG_CONFIG_HOME}/rsshmount"
mkdir -p "${config_dir}"
jq -n \
  --arg user "${test_user}" \
  --arg port "${port}" \
  --arg key "${test_root}/client-key" \
  --arg remote "${remote_name}" \
  --arg mountpoint "${mountpoint}" \
  '[{
    id: "local-sftp",
    name: "Local SFTP",
    mode: "manual",
    source: "manual",
    host: "127.0.0.1",
    user: $user,
    port: $port,
    auth: "key",
    key_file: $key,
    connection_method: "native",
    remote_path: $remote,
    mountpoint: $mountpoint,
    cache_mode: "full"
  }]' >"${config_dir}/servers.json"
jq -n '{
  settings_schema_version: 8,
  vfs_cache_mode: "full",
  vfs_cache_max_age: "30m",
  vfs_write_back: "5s",
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
test "$(cat "${mountpoint}/remote-new.txt")" = 'created outside the mount'

dd if=/dev/zero of="${mountpoint}/upload.bin" bs=1M count=8 conv=fsync status=none
queued_output="$("${binary}" --refresh-id local-sftp)"
grep -F 'local file(s) are still waiting to upload' <<<"${queued_output}"

for _ in $(seq 1 300); do
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
