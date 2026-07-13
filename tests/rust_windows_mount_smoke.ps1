param(
  [Parameter(Mandatory = $true)]
  [string] $PackageRoot
)

$ErrorActionPreference = 'Stop'
$packageRoot = (Resolve-Path $PackageRoot).Path
$binary = Join-Path $packageRoot 'SSHMountMate.exe'
$rclone = Join-Path $packageRoot 'bin/rclone.exe'
$testRoot = Join-Path $env:RUNNER_TEMP "ssh-mountmate-mount-e2e-$PID"
$remoteRoot = Join-Path $testRoot 'remote'
$serverLog = Join-Path $testRoot 'sftp-server.log'
$winFspLog = Join-Path $testRoot 'winfsp-install.log'
$server = $null
$mounted = $false
$succeeded = $false

function Invoke-SSHMountMate([string[]] $Arguments) {
  $processInfo = [System.Diagnostics.ProcessStartInfo]::new($binary)
  $processInfo.UseShellExecute = $false
  $processInfo.CreateNoWindow = $true
  $processInfo.RedirectStandardOutput = $true
  $processInfo.RedirectStandardError = $true
  $Arguments | ForEach-Object { $processInfo.ArgumentList.Add($_) }
  $process = [System.Diagnostics.Process]::Start($processInfo)
  $stdout = $process.StandardOutput.ReadToEndAsync()
  $stderr = $process.StandardError.ReadToEndAsync()
  $process.WaitForExit()
  $output = $stdout.GetAwaiter().GetResult()
  $errorOutput = $stderr.GetAwaiter().GetResult()
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
  if (-not (Test-Path $binary -PathType Leaf)) { throw 'Packaged SSH MountMate is missing' }
  if (-not (Test-Path $rclone -PathType Leaf)) { throw 'Packaged rclone is missing' }
  New-Item -ItemType Directory -Force $remoteRoot | Out-Null
  Set-Content -Path (Join-Path $remoteRoot 'initial.txt') -Value 'initial remote content' -NoNewline

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

  $listener = [System.Net.Sockets.TcpListener]::new(
    [System.Net.IPAddress]::Loopback,
    0
  )
  $listener.Start()
  $port = ([System.Net.IPEndPoint] $listener.LocalEndpoint).Port
  $listener.Stop()

  $serverInfo = [System.Diagnostics.ProcessStartInfo]::new($rclone)
  $serverInfo.UseShellExecute = $false
  $serverInfo.CreateNoWindow = $true
  @(
    '--cache-dir', (Join-Path $testRoot 'server-cache'),
    '--log-file', $serverLog,
    '-vv', 'serve', 'sftp', $remoteRoot,
    '--addr', "127.0.0.1:$port",
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

  $env:HOME = Join-Path $testRoot 'home'
  $env:USERPROFILE = $env:HOME
  $env:APPDATA = Join-Path $testRoot 'roaming'
  $env:LOCALAPPDATA = Join-Path $testRoot 'local'
  $configDir = Join-Path $env:APPDATA 'rsshmount'
  New-Item -ItemType Directory -Force $configDir | Out-Null
  $passwordObscured = (& $rclone obscure 'test-only-password').Trim()
  if ($LASTEXITCODE -ne 0 -or -not $passwordObscured) { throw 'rclone obscure failed' }
  @(
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
  ) | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $configDir 'servers.json')
  [ordered]@{
    settings_schema_version = 8
    vfs_cache_mode = 'full'
    vfs_cache_max_age = '30m'
    vfs_write_back = '10s'
    dir_cache_time = '5m'
    auto_show_transfers = $false
    auto_check_updates = $false
    language = 'en'
  } | ConvertTo-Json | Set-Content (Join-Path $configDir 'settings.json')

  Invoke-SSHMountMate @('--mount-id', 'local-sftp') | Out-Null
  $mounted = $true
  Wait-Until { Test-Path "${mountpoint}\initial.txt" }
  if ((Get-Content "${mountpoint}\initial.txt" -Raw) -ne 'initial remote content') {
    throw 'Mounted initial file content did not match the SFTP source'
  }

  Get-ChildItem "${mountpoint}\" | Out-Null
  Set-Content -Path (Join-Path $remoteRoot 'remote-new.txt') `
    -Value 'created outside the mount' -NoNewline
  $refreshOutput = Invoke-SSHMountMate @('--refresh-path', "$mountpoint`"")
  if ($refreshOutput -notmatch 'Remote verified:') { throw 'Refresh was not remotely verified' }
  Wait-Until { Test-Path "${mountpoint}\remote-new.txt" } 50
  if ((Get-Content "${mountpoint}\remote-new.txt" -Raw) -ne 'created outside the mount') {
    throw 'Refreshed file content did not match the SFTP source'
  }

  $upload = [byte[]]::new(8MB)
  [System.IO.File]::WriteAllBytes("${mountpoint}\upload.bin", $upload)
  $queuedOutput = Invoke-SSHMountMate @('--refresh-id', 'local-sftp')
  if ($queuedOutput -notmatch 'local file\(s\) are still waiting to upload') {
    throw 'A queued write was reported as remotely complete'
  }
  $remoteUpload = Join-Path $remoteRoot 'upload.bin'
  Wait-Until {
    (Test-Path $remoteUpload -PathType Leaf) -and
      ((Get-Item $remoteUpload).Length -eq (8 * 1024 * 1024))
  } 300
  $mountedHash = (Get-FileHash -Algorithm SHA256 "${mountpoint}\upload.bin").Hash
  $remoteHash = (Get-FileHash -Algorithm SHA256 $remoteUpload).Hash
  if ($mountedHash -ne $remoteHash) { throw 'Uploaded file digest did not match the mount' }

  Wait-Until {
    $completedOutput = Invoke-SSHMountMate @('--refresh-id', 'local-sftp')
    return $completedOutput -notmatch 'still waiting to upload'
  } 50

  Invoke-SSHMountMate @('--unmount-id', 'local-sftp') | Out-Null
  $mounted = $false
  Wait-Until { -not (Test-Path "${mountpoint}\") }
  $state = Join-Path $env:LOCALAPPDATA 'rsshmount/State/local-sftp.json'
  if (Test-Path $state) { throw 'Mount state remained after unmount' }
  $succeeded = $true
} finally {
  if ($mounted) {
    try { Invoke-SSHMountMate @('--unmount-id', 'local-sftp') | Out-Null } catch {}
  }
  if ($server -and -not $server.HasExited) {
    $server.Kill($true)
    $server.WaitForExit()
  }
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
}
