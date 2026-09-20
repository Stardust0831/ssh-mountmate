param(
  [Parameter(Mandatory = $true)]
  [string[]] $Binary
)

$ErrorActionPreference = 'Stop'

# A launch check alone misses missing runtime DLLs on developer/CI machines,
# where the Visual C++ redistributable is already installed. Inspect both normal
# and delay-load imports without executing the binary (works for x64 and ARM64).
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$installation = & $vswhere -latest -products '*' -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $installation) {
  throw 'Visual Studio installation was not found for the DLL dependency check'
}
$hostArch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
$dumpbin = Get-ChildItem "$installation/VC/Tools/MSVC/*/bin/Host$hostArch/$hostArch/dumpbin.exe" |
  Sort-Object FullName -Descending |
  Select-Object -First 1
if (-not $dumpbin) { throw "dumpbin.exe was not found for host $hostArch" }

foreach ($path in $Binary) {
  $resolved = (Resolve-Path $path).Path
  $output = & $dumpbin.FullName /nologo /dependents $resolved 2>&1
  if ($LASTEXITCODE -ne 0) { throw "Could not inspect DLL imports for ${path}:`n$output" }
  $imports = @($output | ForEach-Object {
    if ($_ -match '^\s+([\w.-]+\.dll)\s*$') { $Matches[1] }
  })
  if ($imports.Count -eq 0) { throw "No DLL imports found in $path; dependency check did not run" }
  # msvcrt.dll and ucrtbase.dll are Windows components. Numbered MSVC runtime
  # DLLs require a separate redistributable and must not be imported here.
  $externalRuntime = @($imports | Where-Object {
    $_ -match '^(vcruntime|msvcp|msvcr|concrt|vcomp)\d.*\.dll$'
  })
  if ($externalRuntime.Count -ne 0) {
    throw "$path requires the Visual C++ redistributable: $($externalRuntime -join ', ')"
  }
  Write-Host "No external Visual C++ runtime required: $path"
}
