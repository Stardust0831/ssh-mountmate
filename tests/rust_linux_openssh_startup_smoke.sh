#!/usr/bin/env bash
set -euo pipefail

package_root="$(realpath "${1:?packaged SSH MountMate root is required}")"
binary="${package_root}/SSHMountMate"
rclone="${package_root}/bin/rclone"
test_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/ssh-mountmate-openssh-e2e-XXXXXX")"
remote_root="${test_root}/remote"
mount_root="${test_root}/mounts"
server_pid=""
server_ids=(native-a native-b openssh-a openssh-b)

cleanup() {
  status=$?
  if [[ "${status}" -ne 0 ]]; then
    if [[ -f "${test_root}/sftp-server.log" ]]; then
      printf '%s\n' '--- SFTP server log ---' >&2
      tail -100 "${test_root}/sftp-server.log" >&2 || true
    fi
    if [[ -n "${XDG_STATE_HOME:-}" && -d "${XDG_STATE_HOME}/rsshmount" ]]; then
      printf '%s\n' '--- SSH MountMate state and logs ---' >&2
      find "${XDG_STATE_HOME}/rsshmount" -maxdepth 1 -type f -print -exec tail -100 {} \; >&2 || true
    fi
  fi
  if [[ -x "${binary}" ]]; then
    "${binary}" --unmount-all >/dev/null 2>&1 || true
    "${binary}" --unregister-login-startup >/dev/null 2>&1 || true
  fi
  for server_id in "${server_ids[@]}"; do
    mountpoint="${mount_root}/${server_id}"
    if mountpoint -q "${mountpoint}"; then
      fusermount3 -u "${mountpoint}" 2>/dev/null || sudo umount "${mountpoint}" 2>/dev/null || true
    fi
  done
  if [[ -n "${server_pid}" ]]; then
    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
  fi
  rm -rf "${test_root}"
}
trap cleanup EXIT

test -x "${binary}"
test -x "${rclone}"
command -v ssh >/dev/null
command -v ssh-keygen >/dev/null
command -v ssh-keyscan >/dev/null
if [[ ! -c /dev/fuse ]]; then
  sudo modprobe fuse || true
fi
if [[ ! -c /dev/fuse ]]; then
  sudo mknod -m 666 /dev/fuse c 10 229
fi
test -c /dev/fuse
sudo chmod a+rw /dev/fuse

mkdir -p "${remote_root}" "${mount_root}" "${test_root}/home"
for server_id in "${server_ids[@]}"; do
  mkdir -p "${remote_root}/${server_id}" "${mount_root}/${server_id}"
  printf '%s\n' "content from ${server_id}" >"${remote_root}/${server_id}/identity.txt"
done

client_key="${test_root}/client-key"
host_key="${test_root}/host-key"
authorized_keys="${test_root}/authorized_keys"
known_hosts="${test_root}/known_hosts"
ssh_config="${test_root}/ssh-config"
ssh-keygen -q -t rsa -b 3072 -N '' -f "${client_key}"
ssh-keygen -q -t ecdsa -b 256 -N '' -f "${host_key}"
cp "${client_key}.pub" "${authorized_keys}"
chmod 600 "${client_key}" "${host_key}" "${authorized_keys}"

port=""
for candidate in $(seq 42200 42300); do
  if ! ss -H -ltn "sport = :${candidate}" | grep -q .; then
    port="${candidate}"
    break
  fi
done
test -n "${port}"

"${rclone}" --cache-dir "${test_root}/server-cache" \
  --log-file "${test_root}/sftp-server.log" -vv \
  serve sftp "${remote_root}" --addr "127.0.0.1:${port}" \
  --authorized-keys "${authorized_keys}" --key "${host_key}" \
  --dir-cache-time 0s --poll-interval 0 &
server_pid=$!
server_ready=false
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

ssh-keyscan -T 5 -t ecdsa -p "${port}" 127.0.0.1 >"${known_hosts}" 2>/dev/null
test -s "${known_hosts}"
chmod 600 "${known_hosts}"
cat >"${ssh_config}" <<EOF
Host local-openssh-a local-openssh-b
    HostName 127.0.0.1
    User mountmate
    Port ${port}
    IdentityFile ${client_key}
    IdentitiesOnly yes
    BatchMode yes
    StrictHostKeyChecking yes
    UserKnownHostsFile ${known_hosts}
EOF
chmod 600 "${ssh_config}"

export HOME="${test_root}/home"
export XDG_CONFIG_HOME="${test_root}/config"
export XDG_CACHE_HOME="${test_root}/cache"
export XDG_STATE_HOME="${test_root}/state"
export XDG_DATA_HOME="${test_root}/data"
config_dir="${XDG_CONFIG_HOME}/rsshmount"
state_dir="${XDG_STATE_HOME}/rsshmount"
mkdir -p "${config_dir}"

jq -n \
  --arg port "${port}" \
  --arg key "${client_key}" \
  --arg ssh_config "${ssh_config}" \
  --arg native_a_mount "${mount_root}/native-a" \
  --arg native_b_mount "${mount_root}/native-b" \
  --arg openssh_a_mount "${mount_root}/openssh-a" \
  --arg openssh_b_mount "${mount_root}/openssh-b" \
  '[
    {
      id: "native-a", name: "Native A", mode: "manual", source: "manual",
      host: "127.0.0.1", user: "mountmate", port: $port, auth: "key",
      key_file: $key, connection_method: "native", remote_path: "native-a",
      mountpoint: $native_a_mount, cache_mode: "full"
    },
    {
      id: "native-b", name: "Native B", mode: "manual", source: "manual",
      host: "127.0.0.1", user: "mountmate", port: $port, auth: "key",
      key_file: $key, connection_method: "native", remote_path: "native-b",
      mountpoint: $native_b_mount, cache_mode: "full"
    },
    {
      id: "openssh-a", name: "OpenSSH A", mode: "ssh_config", source: "ssh_config",
      host_alias: "local-openssh-a", host: "127.0.0.1", user: "mountmate",
      port: $port, auth: "key", connection_method: "openssh",
      ssh_config_path: $ssh_config, remote_path: "openssh-a",
      mountpoint: $openssh_a_mount, cache_mode: "full"
    },
    {
      id: "openssh-b", name: "OpenSSH B", mode: "ssh_config", source: "ssh_config",
      host_alias: "local-openssh-b", host: "127.0.0.1", user: "mountmate",
      port: $port, auth: "key", connection_method: "openssh",
      ssh_config_path: $ssh_config, remote_path: "openssh-b",
      mountpoint: $openssh_b_mount, cache_mode: "full"
    }
  ]' >"${config_dir}/servers.json"
jq -n '{
  settings_schema_version: 8,
  vfs_cache_mode: "full",
  vfs_cache_max_age: "30m",
  vfs_write_back: "0s",
  dir_cache_time: "5m",
  startup_all: true,
  auto_show_transfers: false,
  auto_check_updates: false,
  language: "en"
}' >"${config_dir}/settings.json"

"${binary}" --register-login-startup
startup="${XDG_CONFIG_HOME}/autostart/ssh-mountmate.desktop"
test -f "${startup}"
grep -Fx "Exec=${binary} --mount-startup-all" "${startup}"

"${binary}" --mount-startup-all
for server_id in "${server_ids[@]}"; do
  mountpoint="${mount_root}/${server_id}"
  mountpoint -q "${mountpoint}"
  test "$(cat "${mountpoint}/identity.txt")" = "content from ${server_id}"
  test "$(jq -r '.phase' "${state_dir}/${server_id}.json")" = 'mounted'
done

rclone_config="${config_dir}/rclone.conf"
grep -F '[local-openssh-a]' "${rclone_config}"
grep -F "ssh = ssh -o BatchMode=yes -F ${ssh_config} local-openssh-a" "${rclone_config}"
grep -F '[local-openssh-b]' "${rclone_config}"
grep -F "ssh = ssh -o BatchMode=yes -F ${ssh_config} local-openssh-b" "${rclone_config}"
if sed -n '/\[local-openssh-a\]/,/^$/p;/\[local-openssh-b\]/,/^$/p' "${rclone_config}" \
  | grep -Eq '^(pass|key_file|key_file_pass|key_use_agent) ='; then
  echo 'OpenSSH remotes unexpectedly contain a native-auth fallback' >&2
  exit 1
fi

state_files=()
for server_id in "${server_ids[@]}"; do
  state_files+=("${state_dir}/${server_id}.json")
done
start_spread="$(jq -s 'map(.process_started_at) as $starts | ($starts | max) - ($starts | min)' "${state_files[@]}")"
if (( start_spread > 1 )); then
  echo "Login mounts did not start concurrently; process start spread was ${start_spread}s" >&2
  exit 1
fi

"${binary}" --unmount-all
for server_id in "${server_ids[@]}"; do
  if mountpoint -q "${mount_root}/${server_id}"; then
    echo "${server_id} remained mounted after --unmount-all" >&2
    exit 1
  fi
  test ! -e "${state_dir}/${server_id}.json"
done
"${binary}" --unregister-login-startup
test ! -e "${startup}"
printf 'OpenSSH and concurrent login mounts passed: spread=%ss mounts=%s\n' \
  "${start_spread}" "${#server_ids[@]}"
