param([string] $Output = 'target/release/bin/winfsp.msi')
$ErrorActionPreference = 'Stop'
$pin = Get-Content (Join-Path $PSScriptRoot '../distribution/winfsp.json') -Raw | ConvertFrom-Json
New-Item -ItemType Directory -Force (Split-Path $Output) | Out-Null
Invoke-WebRequest $pin.url -OutFile $Output
$actual = (Get-FileHash -Algorithm SHA256 $Output).Hash.ToLowerInvariant()
if ($actual -ne $pin.sha256) { throw "WinFsp installer checksum mismatch: $actual" }
$signature = Get-AuthenticodeSignature -FilePath $Output
if ($signature.Status -ne 'Valid') { throw "WinFsp installer signature is not valid: $($signature.Status)" }
"SSH_MOUNTMATE_EMBED_WINFSP_PATH=$((Resolve-Path $Output).Path)" >> $env:GITHUB_ENV
"SSH_MOUNTMATE_EMBED_WINFSP_SHA256=$actual" >> $env:GITHUB_ENV
Write-Host "Verified official WinFsp $($pin.version) installer for embedding"
