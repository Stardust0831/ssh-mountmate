#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
iss="${root}/distribution/windows-installer/SSHMountMate.iss"
builder="${root}/distribution/windows-installer/build-installer.ps1"

test -s "${iss}"
test -s "${builder}"
grep -F 'PrivilegesRequired=lowest' "${iss}"
grep -F 'DefaultDirName={#InstallRoot}' "${iss}"
grep -F 'WINDOWS_INSTALL_RECORD' "${root}/crates/mountmate-core/src/installed.rs" >/dev/null
grep -F 'installer-uninstall-preflight' "${iss}"
grep -F 'InitializeSetup' "${iss}"
grep -F 'Downgrade' "${root}/crates/mountmate-core/src/installed.rs"
grep -F 'SHA256SUMS.txt' "${root}/.github/workflows/release.yml"

echo "Windows installer static checks passed"
