#!/usr/bin/env bash
set -euo pipefail

bundle_input="${1:-target/package/SSH MountMate.app}"
bundle_parent="$(cd "$(dirname "$bundle_input")" && pwd)"
bundle="$bundle_parent/$(basename "$bundle_input")"
binary="$bundle/Contents/MacOS/SSHMountMate"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/ssh-mountmate-macos-XXXXXX")"
app_pid=""

cleanup() {
  status=$?
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  if [[ "$status" != 0 ]]; then
    printf '%s\n' '--- SSH MountMate trace ---' >&2
    cat "$test_root/gui.trace" >&2 2>/dev/null || true
    printf '%s\n' '--- SSH MountMate stderr ---' >&2
    cat "$test_root/gui.stderr" >&2 2>/dev/null || true
  fi
  rm -rf "$test_root"
  trap - EXIT
  exit "$status"
}
trap cleanup EXIT

[[ -x "$binary" ]] || { echo "macOS application binary is missing: $binary" >&2; exit 1; }
mkdir -p \
  "$test_root/home" \
  "$test_root/config" \
  "$test_root/cache" \
  "$test_root/state"

export HOME="$test_root/home"
export XDG_CONFIG_HOME="$test_root/config"
export XDG_CACHE_HOME="$test_root/cache"
export XDG_STATE_HOME="$test_root/state"
export SSH_MOUNTMATE_TRACE_FILE="$test_root/gui.trace"
export SSH_MOUNTMATE_E2E_NATIVE_SMOKE=1

lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
"$lsregister" -f "$bundle"

"$binary" >"$test_root/gui.stdout" 2>"$test_root/gui.stderr" &
app_pid=$!
state="$XDG_STATE_HOME/rsshmount/app-command.json"
for _ in {1..400}; do
  [[ -s "$state" ]] && break
  kill -0 "$app_pid" 2>/dev/null || break
  sleep 0.1
done
[[ -s "$state" ]] || { echo "macOS app command state was not created" >&2; exit 1; }
[[ "$(stat -f %Lp "$state")" == "600" ]] || { echo "macOS command state is not mode 600" >&2; exit 1; }

for expected in \
  'main window opened ' \
  'tray initialized' \
  'dock progress updated: Normal { completed: 1, total: 2 }' \
  'native notification submitted'; do
  for _ in {1..300}; do
    grep -Fq "$expected" "$test_root/gui.trace" 2>/dev/null && break
    kill -0 "$app_pid" 2>/dev/null || break
    sleep 0.1
  done
  grep -Fq "$expected" "$test_root/gui.trace" || {
    echo "Missing macOS native integration trace: $expected" >&2
    exit 1
  }
done

"$binary" --show-transfers
for _ in {1..200}; do
  grep -Fq 'ipc-server received ShowTransfers' "$test_root/gui.trace" 2>/dev/null && break
  sleep 0.1
done
grep -Fq 'ipc-server received ShowTransfers' "$test_root/gui.trace"
kill -0 "$app_pid"

process_count=0
while read -r pid command; do
  if [[ "$command" == "$binary" ]]; then
    process_count=$((process_count + 1))
    [[ "$pid" == "$app_pid" ]] || {
      echo "Unexpected macOS SSH MountMate process: $pid" >&2
      exit 1
    }
  fi
done < <(ps -axo pid=,command=)
[[ "$process_count" == 1 ]] || { echo "Expected one macOS GUI process, found $process_count" >&2; exit 1; }

if grep -Eq 'panicked|dock progress failed|native notification failed|tray unavailable' \
  "$test_root/gui.stderr" "$test_root/gui.trace"; then
  echo "macOS native integration reported a failure" >&2
  exit 1
fi

printf 'macOS native integration passed: pid=%s mode=600\n' "$app_pid"
