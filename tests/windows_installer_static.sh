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
grep -F 'installer-recorded-version' "${iss}"
grep -F 'active mounts or uploads' "${iss}"
grep -F 'Repair or uninstall' "${iss}"
grep -F 'points outside the fixed install directory' "${iss}"
grep -F 'cannot be verified. Restore the installed executable' "${iss}"
grep -F 'InitializeSetup' "${iss}"
grep -F 'Downgrade' "${root}/crates/mountmate-core/src/installed.rs"
workflow="${root}/.github/workflows/release.yml"
grep -F 'SHA256SUMS.txt' "${workflow}"
grep -F -- 'choco install innosetup --version $expected --yes --no-progress --allow-downgrade --force' "${workflow}"
grep -F '${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe' "${workflow}"
grep -F "\$version -notmatch '^6\\.4\\.3(?:\\.0)?\$'" "${workflow}"
if grep -F '"$env:ProgramFiles(x86)\Inno Setup 6\ISCC.exe"' "${workflow}"; then
  echo 'release workflow must brace the ProgramFiles(x86) environment variable' >&2
  exit 1
fi
if grep -F 'Get-Command iscc.exe' "${workflow}"; then
  echo 'release workflow must not trust a runner-provided ISCC.exe shim' >&2
  exit 1
fi

echo "Windows installer static checks passed"
