#!/usr/bin/env bash
set -euo pipefail

package_root="${1:?packaged SSH MountMate root is required}"
export SSH_MOUNTMATE_PACKAGE_ROOT="$(realpath "$package_root")"

openbox >"${RUNNER_TEMP:-/tmp}/update-e2e-openbox.stdout" \
  2>"${RUNNER_TEMP:-/tmp}/update-e2e-openbox.stderr" &
wm_pid=$!
cleanup() {
  kill "$wm_pid" 2>/dev/null || true
  wait "$wm_pid" 2>/dev/null || true
}
trap cleanup EXIT

cargo test --package mountmate-core --test packaged_update -- --ignored --test-threads=1
