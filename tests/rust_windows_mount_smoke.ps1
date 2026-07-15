param(
  [Parameter(Mandatory = $true)]
  [string] $PackageRoot
)

$ErrorActionPreference = 'Stop'
$sourcePackageRoot = (Resolve-Path $PackageRoot).Path
$testRoot = Join-Path $env:RUNNER_TEMP "ssh-mountmate-mount-e2e-$PID"
$packageRoot = Join-Path $testRoot 'install/SSHMountMate'
New-Item -ItemType Directory -Force $packageRoot | Out-Null
Get-ChildItem -LiteralPath $sourcePackageRoot -Force |
  Copy-Item -Destination $packageRoot -Recurse -Force
$binary = Join-Path $packageRoot 'SSHMountMate.exe'
$rclone = $null
$plink = $null
$plinkMaster = $null
$plinkMasterStdout = $null
$plinkMasterStderr = $null
$serverRclone = Join-Path $testRoot 'server-rclone.exe'
$remoteRoot = Join-Path $testRoot 'remote'
$serverLog = Join-Path $testRoot 'sftp-server.log'
$hostKey = Join-Path $testRoot 'sftp-host-key'
$hostKeyBlob = $null
$winFspLog = Join-Path $testRoot 'winfsp-install.log'
$server = $null
$mounted = $false
$mountedId = $null
$succeeded = $false

function Invoke-SSHMountMate([string[]] $Arguments, [switch] $NoCapture) {
  $processInfo = [System.Diagnostics.ProcessStartInfo]::new($binary)
  $processInfo.UseShellExecute = $false
  $processInfo.CreateNoWindow = $true
  $processInfo.RedirectStandardOutput = -not $NoCapture
  $processInfo.RedirectStandardError = -not $NoCapture
  $Arguments | ForEach-Object { $processInfo.ArgumentList.Add($_) }
  $process = [System.Diagnostics.Process]::Start($processInfo)
  if (-not $NoCapture) {
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
  }
  $exited = $process.WaitForExit(60000)
  if (-not $exited) {
    $process.Kill($true)
    $process.WaitForExit()
  }
  $output = if ($NoCapture) { '' } else { $stdout.GetAwaiter().GetResult() }
  $errorOutput = if ($NoCapture) { '' } else { $stderr.GetAwaiter().GetResult() }
  if (-not $exited) {
    throw "SSH MountMate $($Arguments -join ' ') timed out`n$output$errorOutput"
  }
  if ($process.ExitCode -ne 0) {
    throw "SSH MountMate $($Arguments -join ' ') failed with $($process.ExitCode)`n$output$errorOutput"
  }
  return "$output$errorOutput"
}

function Wait-Until([scriptblock] $Condition, [int] $Attempts = 100) {
  for ($attempt = 0; $attempt -lt $Attempts; $attempt++) {
    if (& $Condition) { return }
    Start-Sleep -Milliseconds 100
  }
  throw 'Timed out waiting for the integration-test condition'
}

try {
  if (-not $env:CARGO_HOME) { $env:CARGO_HOME = Join-Path $env:USERPROFILE '.cargo' }
  if (-not $env:RUSTUP_HOME) { $env:RUSTUP_HOME = Join-Path $env:USERPROFILE '.rustup' }
  $env:HOME = Join-Path $testRoot 'home'
  $env:USERPROFILE = $env:HOME
  $env:APPDATA = Join-Path $testRoot 'roaming'
  $env:LOCALAPPDATA = Join-Path $testRoot 'local'

  if (-not (Test-Path $binary -PathType Leaf)) { throw 'Packaged SSH MountMate is missing' }
  $rclone = (Invoke-SSHMountMate @('--rclone-path')).Trim()
  if (-not (Test-Path $rclone -PathType Leaf)) { throw 'Packaged rclone is missing' }
  $plink = (Invoke-SSHMountMate @('--plink-path')).Trim()
  if (-not (Test-Path $plink -PathType Leaf)) { throw 'Packaged Plink is missing' }
  Copy-Item $rclone $serverRclone
  New-Item -ItemType Directory -Force $remoteRoot | Out-Null
  Set-Content -Path (Join-Path $remoteRoot 'initial.txt') -Value 'initial remote content' -NoNewline

  $hostKeyInfo = [System.Diagnostics.ProcessStartInfo]::new('ssh-keygen.exe')
  $hostKeyInfo.UseShellExecute = $false
  $hostKeyInfo.CreateNoWindow = $true
  @('-q', '-t', 'ecdsa', '-b', '256', '-N', '', '-f', $hostKey) |
    ForEach-Object { $hostKeyInfo.ArgumentList.Add($_) }
  $hostKeyProcess = [System.Diagnostics.Process]::Start($hostKeyInfo)
  $hostKeyProcess.WaitForExit()
  if ($hostKeyProcess.ExitCode -ne 0) { throw 'ssh-keygen could not create the test host key' }
  $hostKeyFields = (Get-Content "$hostKey.pub" -Raw).Trim() -split '\s+'
  if ($hostKeyFields.Count -lt 2 -or -not $hostKeyFields[1]) {
    throw 'ssh-keygen produced an invalid public host key'
  }
  $hostKeyBlob = $hostKeyFields[1]

  Write-Host '[windows-mount-e2e] installing WinFsp'
  $winFspUrl = 'https://github.com/winfsp/winfsp/releases/download/v2.1/winfsp-2.1.25156.msi'
  $winFspSha256 = '073a70e00f77423e34bed98b86e600def93393ba5822204fac57a29324db9f7a'
  $winFspMsi = Join-Path $testRoot 'winfsp-2.1.25156.msi'
  Invoke-WebRequest $winFspUrl -OutFile $winFspMsi
  $actualWinFspSha256 = (Get-FileHash -Algorithm SHA256 $winFspMsi).Hash.ToLowerInvariant()
  if ($actualWinFspSha256 -ne $winFspSha256) {
    throw "WinFsp MSI SHA-256 mismatch: $actualWinFspSha256"
  }
  $installer = Start-Process msiexec.exe -ArgumentList @(
    '/i', "`"$winFspMsi`"", '/qn', '/norestart', 'INSTALLLEVEL=1000',
    '/l*v', "`"$winFspLog`""
  ) -Wait -PassThru
  if ($installer.ExitCode -notin @(0, 3010)) {
    throw "WinFsp installation failed with $($installer.ExitCode)"
  }
  Get-Service 'WinFsp.Launcher' -ErrorAction Stop | Out-Null

  Write-Host '[windows-mount-e2e] starting local SFTP server'
  $listener = [System.Net.Sockets.TcpListener]::new(
    [System.Net.IPAddress]::Loopback,
    0
  )
  $listener.Start()
  $port = ([System.Net.IPEndPoint] $listener.LocalEndpoint).Port
  $listener.Stop()

  $serverInfo = [System.Diagnostics.ProcessStartInfo]::new($serverRclone)
  $serverInfo.UseShellExecute = $false
  $serverInfo.CreateNoWindow = $true
  @(
    '--cache-dir', (Join-Path $testRoot 'server-cache'),
    '--log-file', $serverLog,
    '-vv', 'serve', 'sftp', $remoteRoot,
    '--addr', "127.0.0.1:$port",
    '--key', $hostKey,
    '--user', 'mountmate', '--pass', 'test-only-password',
    '--dir-cache-time', '0s', '--poll-interval', '0'
  ) | ForEach-Object { $serverInfo.ArgumentList.Add($_) }
  $server = [System.Diagnostics.Process]::Start($serverInfo)
  Wait-Until {
    if ($server.HasExited) { throw "SFTP server exited with $($server.ExitCode)" }
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
      $client.Connect('127.0.0.1', $port)
      return $true
    } catch {
      return $false
    } finally {
      $client.Dispose()
    }
  } 50

  $drive = @('R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z') |
    Where-Object { -not (Test-Path "${_}:\") } |
    Select-Object -First 1
  if (-not $drive) { throw 'No free drive letter is available for the mount test' }
  $mountpoint = "${drive}:"

  $configDir = Join-Path $env:APPDATA 'rsshmount'
  New-Item -ItemType Directory -Force $configDir | Out-Null
  $passwordObscured = (& $rclone obscure 'test-only-password').Trim()
  if ($LASTEXITCODE -ne 0 -or -not $passwordObscured) { throw 'rclone obscure failed' }
  $servers = @(
    [ordered]@{
      id = 'local-sftp'
      name = 'Local SFTP'
      mode = 'manual'
      source = 'manual'
      host = '127.0.0.1'
      user = 'mountmate'
      port = "$port"
      auth = 'password'
      password_obscured = $passwordObscured
      connection_method = 'native'
      remote_path = ''
      mountpoint = $mountpoint
      cache_mode = 'full'
    }
  )
  ConvertTo-Json -InputObject $servers -Depth 4 |
    Set-Content (Join-Path $configDir 'servers.json')
  [ordered]@{
    settings_schema_version = 8
    vfs_cache_mode = 'full'
    vfs_cache_max_age = '30m'
    vfs_write_back = '90s'
    dir_cache_time = '5m'
    auto_show_transfers = $false
    auto_check_updates = $false
    language = 'en'
  } | ConvertTo-Json | Set-Content (Join-Path $configDir 'settings.json')

  Write-Host '[windows-mount-e2e] mounting drive'
  Invoke-SSHMountMate -Arguments @('--mount-id', 'local-sftp') -NoCapture | Out-Null
  $mounted = $true
  $mountedId = 'local-sftp'
  Wait-Until { Test-Path "${mountpoint}\initial.txt" }
  if ((Get-Content "${mountpoint}\initial.txt" -Raw) -ne 'initial remote content') {
    throw 'Mounted initial file content did not match the SFTP source'
  }

  Get-ChildItem "${mountpoint}\" | Out-Null
  Set-Content -Path (Join-Path $remoteRoot 'remote-new.txt') `
    -Value 'created outside the mount' -NoNewline
  Write-Host '[windows-mount-e2e] refreshing quoted drive root'
  $refreshOutput = Invoke-SSHMountMate @('--refresh-path', "$mountpoint`"")
  if ($refreshOutput -notmatch 'Remote verified:') { throw 'Refresh was not remotely verified' }
  Wait-Until { Test-Path "${mountpoint}\remote-new.txt" } 50
  if ((Get-Content "${mountpoint}\remote-new.txt" -Raw) -ne 'created outside the mount') {
    throw 'Refreshed file content did not match the SFTP source'
  }

  Write-Host '[windows-mount-e2e] verifying queued write-back and remote upload'
  $upload = [byte[]]::new(8MB)
  [System.IO.File]::WriteAllBytes("${mountpoint}\upload.bin", $upload)
  $queuedOutput = Invoke-SSHMountMate @('--refresh-id', 'local-sftp')
  if ($queuedOutput -notmatch 'local file\(s\) are still waiting to upload') {
    throw 'A queued write was reported as remotely complete'
  }
  $env:SSH_MOUNTMATE_ACTIVE_PACKAGE_ROOT = $packageRoot
  $env:SSH_MOUNTMATE_ACTIVE_STATE_FILE = Join-Path $env:LOCALAPPDATA 'rsshmount/State/local-sftp.json'
  & cargo test --package mountmate-core --test packaged_update --all-features `
    packaged_update_preserves_real_active_mount -- `
    --ignored --exact --test-threads=1
  if ($LASTEXITCODE -ne 0) {
    throw 'Active-mount packaged update integration test failed'
  }
  $remoteUpload = Join-Path $remoteRoot 'upload.bin'
  Wait-Until {
    (Test-Path $remoteUpload -PathType Leaf) -and
      ((Get-Item $remoteUpload).Length -eq (8 * 1024 * 1024))
  } 1200
  $mountedHash = (Get-FileHash -Algorithm SHA256 "${mountpoint}\upload.bin").Hash
  $remoteHash = (Get-FileHash -Algorithm SHA256 $remoteUpload).Hash
  if ($mountedHash -ne $remoteHash) { throw 'Uploaded file digest did not match the mount' }

  Wait-Until {
    $completedOutput = Invoke-SSHMountMate @('--refresh-id', 'local-sftp')
    return $completedOutput -notmatch 'still waiting to upload'
  } 50

  Write-Host '[windows-mount-e2e] unmounting drive'
  Invoke-SSHMountMate -Arguments @('--unmount-id', 'local-sftp') -NoCapture | Out-Null
  $mounted = $false
  $mountedId = $null
  Wait-Until { -not (Test-Path "${mountpoint}\") }
  $state = Join-Path $env:LOCALAPPDATA 'rsshmount/State/local-sftp.json'
  if (Test-Path $state) { throw 'Mount state remained after unmount' }

  Write-Host '[windows-mount-e2e] establishing verified Plink connection sharing'
  $masterInfo = [System.Diagnostics.ProcessStartInfo]::new($plink)
  $masterInfo.UseShellExecute = $false
  $masterInfo.CreateNoWindow = $true
  $masterInfo.RedirectStandardOutput = $true
  $masterInfo.RedirectStandardError = $true
  @('-batch', '-ssh', '-share', '-N', '-P', "$port", '-l', 'mountmate', '-pw', 'test-only-password') |
    ForEach-Object { $masterInfo.ArgumentList.Add($_) }
  $masterInfo.ArgumentList.Add('-hostkey')
  $masterInfo.ArgumentList.Add($hostKeyBlob)
  $masterInfo.ArgumentList.Add('127.0.0.1')
  $plinkMaster = [System.Diagnostics.Process]::Start($masterInfo)
  $plinkMasterStdout = $plinkMaster.StandardOutput.ReadToEndAsync()
  $plinkMasterStderr = $plinkMaster.StandardError.ReadToEndAsync()
  Wait-Until {
    if ($plinkMaster.HasExited) {
      $masterOutput = $plinkMasterStdout.GetAwaiter().GetResult()
      $masterError = $plinkMasterStderr.GetAwaiter().GetResult()
      throw "Plink sharing master exited with $($plinkMaster.ExitCode)`n$masterOutput$masterError"
    }
    $check = Start-Process -FilePath $plink -ArgumentList @(
      '-batch', '-ssh', '-shareexists', '-P', "$port", '-l', 'mountmate', '127.0.0.1'
    ) -Wait -PassThru -WindowStyle Hidden
    return $check.ExitCode -eq 0
  } 100

  $interactiveServers = @(
    [ordered]@{
      id = 'interactive-sftp'
      name = 'Interactive SFTP'
      mode = 'manual'
      source = 'manual'
      host = '127.0.0.1'
      user = 'mountmate'
      port = "$port"
      auth = 'key'
      connection_method = 'interactive'
      remote_path = ''
      mountpoint = $mountpoint
      cache_mode = 'full'
    }
  )
  ConvertTo-Json -InputObject $interactiveServers -Depth 4 |
    Set-Content (Join-Path $configDir 'servers.json')
  Invoke-SSHMountMate -Arguments @('--mount-id', 'interactive-sftp') -NoCapture | Out-Null
  $mounted = $true
  $mountedId = 'interactive-sftp'
  Wait-Until { Test-Path "${mountpoint}\initial.txt" }
  if ((Get-Content "${mountpoint}\initial.txt" -Raw) -ne 'initial remote content') {
    throw 'Plink-shared mount did not read the SFTP source'
  }
  $interactiveRemote = Get-Content (Join-Path $configDir 'rclone.conf') -Raw
  if ($interactiveRemote -notmatch 'plink[^\r\n]*-batch[^\r\n]*-share') {
    throw 'Interactive rclone remote did not use Plink sharing'
  }
  if ($interactiveRemote -match 'test-only-password') {
    throw 'Interactive rclone remote leaked the test password'
  }
  Invoke-SSHMountMate -Arguments @('--unmount-id', 'interactive-sftp') -NoCapture | Out-Null
  $mounted = $false
  $mountedId = $null
  Wait-Until { -not (Test-Path "${mountpoint}\") }
  $plinkMaster.Kill($true)
  $plinkMaster.WaitForExit()
  $null = $plinkMasterStdout.GetAwaiter().GetResult()
  $null = $plinkMasterStderr.GetAwaiter().GetResult()
  $plinkMaster = $null
  Write-Host '[windows-mount-e2e] lifecycle passed'
  $succeeded = $true
} finally {
  if ($mounted) {
    try {
      Invoke-SSHMountMate -Arguments @('--unmount-id', $mountedId) -NoCapture | Out-Null
    } catch {
      Write-Warning "Failed to unmount $mountedId during cleanup: $($_.Exception.Message)"
    }
  }
  if ($server -and -not $server.HasExited) {
    $server.Kill($true)
    $server.WaitForExit()
  }
  if ($plinkMaster -and -not $plinkMaster.HasExited) {
    $plinkMaster.Kill($true)
    $plinkMaster.WaitForExit()
  }
  if ($plinkMasterStdout) { $null = $plinkMasterStdout.GetAwaiter().GetResult() }
  if ($plinkMasterStderr) { $null = $plinkMasterStderr.GetAwaiter().GetResult() }
  if (-not $succeeded) {
    if (Test-Path $winFspLog) {
      Write-Host '--- WinFsp installer log ---'
      Get-Content $winFspLog -Tail 100
    }
    if (Test-Path $serverLog) {
      Write-Host '--- SFTP server log ---'
      Get-Content $serverLog -Tail 100
    }
    $stateDir = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA 'rsshmount/State' } else { '' }
    if ($stateDir -and (Test-Path $stateDir)) {
      Write-Host '--- SSH MountMate logs ---'
      Get-ChildItem $stateDir -Filter '*.log' | ForEach-Object { Get-Content $_ -Tail 100 }
    }
  }
  Remove-Item $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
