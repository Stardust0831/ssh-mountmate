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
grep -F 'https://github.com/jrsoftware/issrc/releases/download/is-6_4_3/innosetup-6.4.3.exe' "${workflow}"
grep -F 'f3c42116542c4cc57263c5ba6c4feabfc49fe771f2f98a79d2f7628b8762723b' "${workflow}"
grep -F 'Get-FileHash -LiteralPath $installer -Algorithm SHA256' "${workflow}"
grep -F '"/DIR=$installDir"' "${workflow}"
grep -F "\$isccPath = Join-Path \$installDir 'ISCC.exe'" "${workflow}"
grep -F "\$version -notmatch '^6\\.4\\.3(?:\\.0)?\$'" "${workflow}"
if grep -F 'choco install innosetup' "${workflow}"; then
  echo 'release workflow must not depend on Chocolatey package versions for Inno Setup' >&2
  exit 1
fi
if grep -F 'Get-Command iscc.exe' "${workflow}"; then
  echo 'release workflow must not trust a runner-provided ISCC.exe shim' >&2
  exit 1
fi

echo "Windows installer static checks passed"
