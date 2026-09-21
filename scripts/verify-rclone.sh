#!/usr/bin/env bash
set -euo pipefail
binary=${1:?rclone binary path required}
expected_version=${RCLONE_CUSTOM_VERSION:-v1.74.4-lustre-quota}
test -s "$binary"
expected=$(tr -d '[:space:]' < "$binary.sha256")
[[ "$expected" =~ ^[0-9a-f]{64}$ ]]
if [[ "$(uname -s)" = Darwin ]]; then
  actual=$(shasum -a 256 "$binary" | awk '{print $1}')
else
  actual=$(sha256sum "$binary" | awk '{print $1}')
fi
test "$actual" = "$expected"
version_output=$("$binary" version)
grep -Fx "rclone $expected_version" <<< "$version_output"
grep -Fx -- "- os/type: $(go env GOOS)" <<< "$version_output"
grep -Fx -- "- os/arch: $(go env GOARCH)" <<< "$version_output"
grep -Fx -- "- go/version: $(go env GOVERSION)" <<< "$version_output"
grep -Eq '^- go/tags:.*cmount' <<< "$version_output"
"$binary" mount --help >/dev/null
# Capture first so pipefail does not treat an early grep exit as a broken pipe.
flags=$("$binary" help flags sftp)
grep -F -- '--sftp-lustre-quota' <<< "$flags" >/dev/null
