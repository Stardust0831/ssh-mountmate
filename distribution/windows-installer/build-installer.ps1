[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$InputExe,
  [Parameter(Mandatory = $true)][string]$OutputDir,
  [Parameter(Mandatory = $true)][string]$AppVersion,
  [ValidateSet('x64', 'arm64')][string]$Arch = 'x64',
  [string]$IsccPath = 'iscc.exe'
)

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$iss = Join-Path $scriptDir 'SSHMountMate.iss'
if (-not (Test-Path -LiteralPath $InputExe -PathType Leaf)) {
  throw "onefile executable was not found: $InputExe"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$resolvedIscc = Get-Command $IsccPath -ErrorAction SilentlyContinue
if (-not $resolvedIscc) {
  throw "Inno Setup ISCC.exe was not found. Install the pinned Inno Setup toolchain before packaging."
}
$versionText = (& $resolvedIscc.Source /? 2>&1 | Out-String)
if ($versionText -notmatch 'Inno Setup 6\.') {
  throw "unsupported Inno Setup compiler; expected Inno Setup 6.x"
}

& $resolvedIscc.Source "/DARCH=$Arch" "/DAPP_VERSION=$AppVersion" "/DINPUT_EXE=$((Resolve-Path $InputExe).Path)" "/DOUTPUT_DIR=$((Resolve-Path $OutputDir).Path)" $iss
if ($LASTEXITCODE -ne 0) {
  throw "Inno Setup compilation failed with exit code $LASTEXITCODE"
}
$output = Join-Path (Resolve-Path $OutputDir).Path "SSHMountMate-windows-$Arch-setup.exe"
if (-not (Test-Path -LiteralPath $output -PathType Leaf)) {
  throw "Inno Setup did not produce expected output: $output"
}
Write-Output $output
