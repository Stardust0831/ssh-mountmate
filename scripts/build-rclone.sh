#!/usr/bin/env bash
set -euo pipefail

# Build the pinned rclone release with the small SFTP Lustre quota patch.
# Usage: build-rclone.sh OUTPUT [GOOS] [GOARCH]
output=${1:?output path required}
goos=${2:-$(go env GOOS)}
goarch=${3:-$(go env GOARCH)}
mkdir -p "$(dirname "$output")"
output_dir=$(cd "$(dirname "$output")" && pwd)
output="$output_dir/$(basename "$output")"
version=v1.74.4
custom_version="${RCLONE_CUSTOM_VERSION:-v1.74.4-lustre-quota}"
archive="rclone-${version}.tar.gz"
sha256=23fb09cd209ac6f4540f75cbcfc913fb1e2a35b90cf1a9d67292e2239b4f3a24
work="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/ssh-mountmate-${version}-rclone"
mkdir -p "$work"
# Git Bash receives RUNNER_TEMP as C:\..., which tar interprets as a remote archive.
work=$(cd "$work" && pwd)
archive_path="$work/$archive"
if [[ ! -f "$archive_path" ]]; then
  curl --fail --location --silent --show-error \
    "https://downloads.rclone.org/${version}/${archive}" --output "$archive_path"
fi
if [[ "$(uname -s)" != Darwin ]] && command -v sha256sum >/dev/null 2>&1; then
  echo "$sha256  $archive_path" | sha256sum --check
else
  echo "$sha256  $archive_path" | shasum -a 256 --check
fi
src=$(mktemp -d "$work/source.XXXXXX")
trap 'rm -rf "$src"' EXIT
tar -xzf "$archive_path" -C "$src" --strip-components=1
patch --forward --batch -d "$src" -p1 < "$(cd "$(dirname "$0")/.." && pwd)/patches/rclone-v1.74.4-lustre-quota.patch"
mkdir -p "$output_dir"
case "$goos" in
  linux|darwin) cgo_enabled=1 ;;
  windows) cgo_enabled=0 ;;
  *) echo "unsupported GOOS for SSH MountMate rclone: $goos" >&2; exit 2 ;;
esac
case "$goarch" in
  amd64|arm64) ;;
  *) echo "unsupported GOARCH for SSH MountMate rclone: $goarch" >&2; exit 2 ;;
esac
(
  cd "$src"
  # Run the upstream SFTP unit suite against the patched source before the
  # platform build. This exercises the option/config path without requiring a
  # live Lustre cluster. Packaged mount integration runs later in CI; upstream
  # TestIntegration cases require separately provisioned test servers.
  go test ./backend/sftp -skip '^TestIntegration'
  GOOS="$goos" GOARCH="$goarch" CGO_ENABLED="$cgo_enabled" \
    go build -tags cmount -trimpath -buildvcs=false \
      -ldflags "-s -w -X github.com/rclone/rclone/fs.Version=${custom_version}" \
      -o "$output" .
)
version_output="$("$output" version 2>&1)"
grep -F "${custom_version}" <<<"$version_output" >/dev/null
grep -Eq '^- go/tags:.*cmount' <<<"$version_output"
"$output" mount --help >/dev/null
"$output" help flags sftp | grep -F -- '--sftp-lustre-quota' >/dev/null
printf 'Built patched rclone %s for %s/%s (CGO_ENABLED=%s, tags=cmount)\n' \
  "$custom_version" "$goos" "$goarch" "$cgo_enabled" >&2
