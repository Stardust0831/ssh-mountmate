#!/bin/sh
set -eu

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: $0 RCLONE_SOURCE OUTPUT [--test]" >&2
  exit 2
fi

source_dir=$1
output=$2
test_mode=${3:-}
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
patch_file="$repo_root/distribution/rclone/rclone-v1.74.4-swr.patch"
expected_commit=5bc93a2a7ab0ebd0a11352bc4968eabeffb18027
expected_patch=ebdf3b6d3043526a29efd285768a829e2291275d6fbd4c4836861c665f440334
build_version=v1.74.4-ssh-mountmate.1

actual_commit=$(git -C "$source_dir" rev-parse HEAD | tr -d '\r')
if [ "$actual_commit" != "$expected_commit" ]; then
  echo "unexpected rclone commit: expected $expected_commit, got $actual_commit" >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  actual_patch=$(sha256sum "$patch_file" | awk '{print $1}')
else
  actual_patch=$(shasum -a 256 "$patch_file" | awk '{print $1}')
fi
if [ "$actual_patch" != "$expected_patch" ]; then
  echo "rclone patch SHA-256 mismatch: expected $expected_patch, got $actual_patch" >&2
  exit 1
fi
go_version=$(go env GOVERSION | tr -d '\r')
if [ "$go_version" != go1.25.0 ]; then
  echo "unexpected Go version: expected go1.25.0, got $go_version" >&2
  exit 1
fi

git -C "$source_dir" apply --check "$patch_file"
git -C "$source_dir" apply "$patch_file"

if [ "$test_mode" = "--test" ]; then
  (cd "$source_dir" && \
    go test ./vfs -run '^(TestDirReadDirSWR|TestRcRefreshSWR)' -count=1)
elif [ -n "$test_mode" ]; then
  echo "unknown option: $test_mode" >&2
  exit 2
fi

mkdir -p "$(dirname -- "$output")"
output=$(CDPATH= cd -- "$(dirname -- "$output")" && pwd)/$(basename -- "$output")
go_os=$(go env GOOS | tr -d '\r')
cgo_enabled=0
cgo_cflags=
cgo_ldflags=
if [ "$go_os" = darwin ]; then
  cgo_enabled=1
  cgo_cflags=-I/usr/local/include
  cgo_ldflags=-L/usr/local/lib
fi
(cd "$source_dir" && \
  CGO_ENABLED="$cgo_enabled" CGO_CFLAGS="$cgo_cflags" CGO_LDFLAGS="$cgo_ldflags" \
  SOURCE_DATE_EPOCH=1783527537 \
  go build -tags cmount -trimpath -buildvcs=false \
    -ldflags="-s -w -buildid= -X github.com/rclone/rclone/fs.Version=$build_version" \
    -o "$output" .)
version_output=$("$output" version)
printf '%s\n' "$version_output"
printf '%s\n' "$version_output" | grep -F "rclone $build_version" >/dev/null
