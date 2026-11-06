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
$shareName = "mountmate-ci-$PID"
$shareRoot = Join-Path $testRoot 'smb'
$shareCreated = $false
$networkMappingCreated = $false
$networkDrive = $null
$networkPath = "\\localhost\$shareName"
$rememberedBefore = @()

Add-Type @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;

public static class MountMateDriveEvidence {
    [DllImport("kernel32.dll", SetLastError = true)] public static extern uint GetLogicalDrives();
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint QueryDosDevice(string name, StringBuilder target, int count);
    [DllImport("mpr.dll", CharSet = CharSet.Unicode)]
    private static extern int WNetGetConnection(string name, StringBuilder target, ref int count);
    [DllImport("mpr.dll", CharSet = CharSet.Unicode)]
    public static extern int WNetCancelConnection2(string name, uint flags, bool force);
    [DllImport("mpr.dll", CharSet = CharSet.Unicode)]
    private static extern int WNetOpenEnum(uint scope, uint type, uint usage, IntPtr resource, out IntPtr handle);
    [DllImport("mpr.dll", CharSet = CharSet.Unicode)]
    private static extern int WNetEnumResource(IntPtr handle, ref uint count, IntPtr buffer, ref uint size);
    [DllImport("mpr.dll")] private static extern int WNetCloseEnum(IntPtr handle);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool GetTokenInformation(IntPtr token, int kind, byte[] data, uint size, out uint needed);
    [StructLayout(LayoutKind.Sequential)]
    private struct Resource {
        public uint Scope, Type, DisplayType, Usage;
        public IntPtr LocalName, RemoteName, Comment, Provider;
    }

    private static Dictionary<string, string> RememberedConnections() {
        IntPtr handle;
        int status = WNetOpenEnum(3, 1, 0, IntPtr.Zero, out handle);
        if (status != 0) throw new Win32Exception(status);
        var drives = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        uint capacity = 16384;
        try {
            for (int page = 0; page < 128; page++) {
                IntPtr buffer = Marshal.AllocHGlobal((int)capacity);
                try {
                    uint count = 0xffffffff, size = capacity;
                    status = WNetEnumResource(handle, ref count, buffer, ref size);
                    if (status == 259) return drives;
                    if (status == 234 && size > capacity && size <= 1048576) { capacity = size; continue; }
                    if (status != 0) throw new Win32Exception(status);
                    if (count == 0 || count > capacity / Marshal.SizeOf<Resource>()) throw new Exception("Invalid remembered-drive page");
                    for (int index = 0; index < count; index++) {
                        var item = Marshal.PtrToStructure<Resource>(IntPtr.Add(buffer, index * Marshal.SizeOf<Resource>()));
                        string name = Marshal.PtrToStringUni(item.LocalName);
                        if (!String.IsNullOrEmpty(name)) drives.Add(name, Marshal.PtrToStringUni(item.RemoteName));
                    }
                } finally { Marshal.FreeHGlobal(buffer); }
            }
            throw new Exception("Remembered-drive enumeration exceeded its bound");
        } finally { WNetCloseEnum(handle); }
    }

    public static string[] RememberedDrives() { return new List<string>(RememberedConnections().Keys).ToArray(); }

    public static object Observe(string drive) {
        var target = new StringBuilder(32768);
        uint mask = GetLogicalDrives();
        if (mask == 0) throw new Win32Exception(Marshal.GetLastWin32Error());
        uint dos = QueryDosDevice(drive, target, target.Capacity);
        int dosError = dos == 0 ? Marshal.GetLastWin32Error() : 0;
        int count = 32768;
        var remote = new StringBuilder(count);
        int connection = WNetGetConnection(drive, remote, ref count);
        var remembered = RememberedConnections();
        return new { drive, logicalMaskOccupied = (mask & (1u << (drive[0] - 'A'))) != 0,
            dosDevicePresent = dos != 0, dosError, connectionStatus = connection,
            connectionTarget = connection == 0 ? remote.ToString() : null,
            remembered = remembered.ContainsKey(drive),
            rememberedTarget = remembered.ContainsKey(drive) ? remembered[drive] : null };
    }

    public static object Caller() {
        using (var identity = WindowsIdentity.GetCurrent()) {
            var stats = new byte[64]; uint needed;
            if (!GetTokenInformation(identity.Token, 10, stats, (uint)stats.Length, out needed)) throw new Win32Exception(Marshal.GetLastWin32Error());
            string authenticationId = BitConverter.ToUInt64(stats, 8).ToString("x16");
            var elevated = new byte[4];
            if (!GetTokenInformation(identity.Token, 20, elevated, 4, out needed)) throw new Win32Exception(Marshal.GetLastWin32Error());
            return new { name = identity.Name, authenticationId, elevated = BitConverter.ToUInt32(elevated, 0) != 0 };
        }
    }
}
'@

function Assert-ReservedNetworkDrive([string] $State) {
  $observed = [MountMateDriveEvidence]::Observe($networkDrive)
  if (-not $observed.remembered) { throw 'The fixture is not a native persistent network mapping' }
  if ($State -eq 'connected' -and $observed.connectionStatus -ne 0) {
    throw 'The connected SMB fixture was not confirmed by WNetGetConnection'
  }
  if ($State -eq 'remembered' -and $observed.connectionStatus -eq 0) {
    throw 'The fixture remained connected after the native disconnect'
  }
  Write-Host ('[windows-drive-evidence] ' + (@{
    state = $State; caller = [MountMateDriveEvidence]::Caller(); observed = $observed
    legacyMaskMiss = -not $observed.logicalMaskOccupied
  } | ConvertTo-Json -Compress -Depth 5))
  $env:SSH_MOUNTMATE_TEST_RESERVED_DRIVE = $networkDrive
  $env:SSH_MOUNTMATE_TEST_MAPPING_STATE = $State
  cargo test --package mountmate-core mountpoint::tests::native_reserved_drive_is_unavailable `
    --all-features -- --ignored --exact --test-threads=1
  if ($LASTEXITCODE -ne 0) { throw "Native reserved-drive contract failed for $State" }
}

function Invoke-SSHMountMate([string[]] $Arguments, [switch] $NoCapture, [int] $TimeoutMs = 60000) {
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
  $exited = $process.WaitForExit($TimeoutMs)
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
  $pin = Get-Content (Join-Path $PSScriptRoot '../distribution/winfsp.json') -Raw | ConvertFrom-Json
  $winFspSha256 = $pin.sha256
  $winFspMsi = (Invoke-SSHMountMate @('--winfsp-installer-path')).Trim()
  if (-not (Test-Path $winFspMsi -PathType Leaf)) { throw 'Embedded WinFsp MSI is missing' }
  $actualWinFspSha256 = (Get-FileHash -Algorithm SHA256 $winFspMsi).Hash.ToLowerInvariant()
  if ($actualWinFspSha256 -ne $winFspSha256) {
    throw "WinFsp MSI SHA-256 mismatch: $actualWinFspSha256"
  }
  $winFspLog = Join-Path $env:LOCALAPPDATA 'ssh-mountmate/state/winfsp-install.log'
  Invoke-SSHMountMate @('--install-winfsp') -TimeoutMs 180000 | Write-Host
  Get-Service 'WinFsp.Launcher' -ErrorAction Stop | Out-Null

  Write-Host '[windows-mount-e2e] starting local SFTP server'
  $serverReady = $false
  for ($attempt = 0; $attempt -lt 20 -and -not $serverReady; $attempt++) {
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
    for ($probe = 0; $probe -lt 50; $probe++) {
      if ($server.HasExited) { break }
      $client = [System.Net.Sockets.TcpClient]::new()
      try {
        $client.Connect('127.0.0.1', $port)
        $serverReady = $true
        break
      } catch {
        # The listener may still be starting; retry while the child is alive.
      } finally {
        $client.Dispose()
      }
      Start-Sleep -Milliseconds 100
    }
    if (-not $serverReady) {
      if ($server -and -not $server.HasExited) {
        $server.Kill($true)
        $server.WaitForExit()
      }
      $server = $null
    }
  }
  if (-not $serverReady) {
    throw 'Failed to start the SFTP server on an available loopback port'
  }

  $caller = [MountMateDriveEvidence]::Caller()
  $rememberedBefore = @([MountMateDriveEvidence]::RememberedDrives() | Sort-Object)
  $networkDrive = @('Z', 'Y', 'X', 'W', 'V', 'U', 'T', 'S', 'R') |
    ForEach-Object { "${_}:" } | Where-Object {
      $probe = [MountMateDriveEvidence]::Observe($_)
      -not $probe.logicalMaskOccupied -and -not $probe.dosDevicePresent -and
        -not $probe.remembered -and $probe.dosError -eq 2 -and $probe.connectionStatus -eq 2250
    } | Select-Object -First 1
  if (-not $networkDrive) { throw 'A verified free drive is required for the SMB fixture' }
  if (Get-SmbShare -Name $shareName -ErrorAction SilentlyContinue) { throw 'Fixture share already exists' }
  if ((Get-Service LanmanServer).Status -ne 'Running') { throw 'The runner SMB server must already be running' }
  New-Item -ItemType Directory $shareRoot | Out-Null
  Set-Content (Join-Path $shareRoot 'network-original.txt') 'original SMB content' -NoNewline
  New-SmbShare -Name $shareName -Path $shareRoot -Temporary -CachingMode None -FullAccess $caller.name | Out-Null
  $shareCreated = $true
  New-SmbMapping -LocalPath $networkDrive -RemotePath $networkPath -Persistent $true | Out-Null
  $networkMappingCreated = $true
  if ((Get-Content "${networkDrive}\network-original.txt" -Raw) -ne 'original SMB content') { throw 'SMB fixture content mismatch' }
  Assert-ReservedNetworkDrive 'connected'
  if ([MountMateDriveEvidence]::WNetCancelConnection2($networkDrive, 0, $false) -ne 0) { throw 'Could not disconnect the fixture while retaining its profile' }
  Assert-ReservedNetworkDrive 'remembered'
  if ([MountMateDriveEvidence]::Caller().authenticationId -ne $caller.authenticationId) { throw 'The test changed logon session' }
  $secondaryMount = $null
  $secondaryId = 'local-sftp--mount-0123456789abcdef0123456789abcdef'
  $mountpoint = $null

  $configDir = Join-Path $env:LOCALAPPDATA 'ssh-mountmate/config'
  New-Item -ItemType Directory -Force $configDir | Out-Null
  "[127.0.0.1]:$port $($hostKeyFields[0]) $hostKeyBlob" |
    Set-Content (Join-Path $configDir 'known_hosts')
  $env:SSH_MOUNTMATE_HOST_KEY_TEST_PORT = "$port"
  $env:SSH_MOUNTMATE_HOST_KEY_TEST_PUBLIC = "$($hostKeyFields[0]) $hostKeyBlob"
  cargo test --package mountmate-core host_key::tests::live_host_key_probe --all-features -- --ignored --exact --test-threads=1
  if ($LASTEXITCODE -ne 0) { throw 'SSH host-key handshake fallback failed' }
  $env:SSH_MOUNTMATE_TEST_RCLONE = $rclone
  $env:SSH_MOUNTMATE_TEST_APP = $binary
  cargo test --package mountmate-core host_key::live_tests::live_multi_algorithm_authentication --all-features -- --ignored --exact --test-threads=1
  if ($LASTEXITCODE -ne 0) { throw 'Multi-algorithm SSH trust and authentication failed' }
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
      mountpoint = 'auto'
      mounts = @(@{ id = '0123456789abcdef0123456789abcdef'; remote_path = 'projects'; mountpoint = 'auto' })
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
  New-Item -ItemType Directory -Force (Join-Path $remoteRoot 'projects') | Out-Null
  Set-Content -Path (Join-Path $remoteRoot 'projects/secondary.txt') -Value 'secondary remote content' -NoNewline
  $mounted = $true
  $mountedId = 'local-sftp'
  Invoke-SSHMountMate -Arguments @('--mount-all') -NoCapture | Out-Null
  $stateDir = Join-Path $env:LOCALAPPDATA 'ssh-mountmate/state'
  Wait-Until {
    (Test-Path (Join-Path $stateDir 'local-sftp.json')) -and
      (Test-Path (Join-Path $stateDir "$secondaryId.json"))
  }
  $mountpoint = (Get-Content (Join-Path $stateDir 'local-sftp.json') -Raw | ConvertFrom-Json).mountpoint
  $secondaryMount = (Get-Content (Join-Path $stateDir "$secondaryId.json") -Raw | ConvertFrom-Json).mountpoint
  if ($mountpoint -notmatch '^[A-Z]:$' -or $secondaryMount -notmatch '^[A-Z]:$' -or
      $mountpoint -eq $secondaryMount -or $mountpoint -eq $networkDrive -or $secondaryMount -eq $networkDrive) {
    throw 'Automatic mappings reused a reserved or invalid drive'
  }
  Wait-Until { Test-Path "${mountpoint}\initial.txt" }
  if ((Get-Content "${mountpoint}\initial.txt" -Raw) -ne 'initial remote content') {
    throw 'Mounted initial file content did not match the SFTP source'
  }

  Wait-Until { Test-Path "${secondaryMount}\secondary.txt" }
  if ((Get-Content "${secondaryMount}\secondary.txt" -Raw) -ne 'secondary remote content') { throw 'Secondary directory mapping is incorrect' }
  $secondaryRefresh = Invoke-SSHMountMate @('--refresh-path', $secondaryMount)
  if ($secondaryRefresh -notmatch 'Remote verified:') { throw 'Secondary mapping refresh failed' }
  Invoke-SSHMountMate -Arguments @('--unmount-id', $secondaryId) -NoCapture | Out-Null
  Wait-Until { -not (Test-Path "${secondaryMount}\") }
  if (-not (Test-Path "${mountpoint}\initial.txt")) { throw 'Unmounting one mapping interrupted its sibling' }

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
  $env:SSH_MOUNTMATE_ACTIVE_STATE_FILE = Join-Path $env:LOCALAPPDATA 'ssh-mountmate/state/local-sftp.json'
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
  $state = Join-Path $env:LOCALAPPDATA 'ssh-mountmate/state/local-sftp.json'
  if (Test-Path $state) { throw 'Mount state remained after unmount' }
  if (-not [MountMateDriveEvidence]::Observe($networkDrive).remembered) { throw 'Unmounting SFTP removed the original network mapping' }
  New-SmbMapping -LocalPath $networkDrive -RemotePath $networkPath -Persistent $true | Out-Null
  if ((Get-Content "${networkDrive}\network-original.txt" -Raw) -ne 'original SMB content') { throw 'SFTP lifecycle altered the original SMB content' }
  Assert-ReservedNetworkDrive 'connected'

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
      Invoke-SSHMountMate -Arguments @('--unmount-all') -NoCapture | Out-Null
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
  $networkCleaned = -not $shareCreated
  if ($shareCreated) {
    try {
      $share = Get-SmbShare -Name $shareName -ErrorAction Stop
      if ($share.Path -ne $shareRoot) { throw 'Fixture share path changed; preserving it' }
      if ($networkMappingCreated) {
        $mapping = [MountMateDriveEvidence]::Observe($networkDrive)
        if (($mapping.remembered -and $mapping.rememberedTarget -ne $networkPath) -or
            ($mapping.connectionStatus -eq 0 -and $mapping.connectionTarget -ne $networkPath)) {
          throw 'Fixture drive now refers to another mapping; preserving it'
        }
        $status = [MountMateDriveEvidence]::WNetCancelConnection2($networkDrive, 1, $true)
        if ($status -ne 0 -and $status -ne 2250) { throw "Fixture mapping cleanup failed: $status" }
        if ([MountMateDriveEvidence]::RememberedDrives() -contains $networkDrive) { throw 'Fixture mapping profile remains' }
      }
      Remove-SmbShare -Name $shareName -Force -Confirm:$false
      $rememberedAfter = @([MountMateDriveEvidence]::RememberedDrives() | Sort-Object)
      if (($rememberedBefore -join ',') -ne ($rememberedAfter -join ',')) { throw 'Unrelated remembered mappings changed' }
      $networkCleaned = $true
    } catch {
      $succeeded = $false
      Write-Warning "SMB fixture cleanup requires inspection: $($_.Exception.Message)"
    }
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
    $stateDir = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA 'ssh-mountmate/state' } else { '' }
    if ($stateDir -and (Test-Path $stateDir)) {
      Write-Host '--- SSH MountMate logs ---'
      Get-ChildItem $stateDir -Filter '*.log' | ForEach-Object { Get-Content $_ -Tail 100 }
    }
  }
  if ($networkCleaned) { Remove-Item $testRoot -Recurse -Force -ErrorAction SilentlyContinue }
  if (-not $networkCleaned) { throw 'SMB fixture cleanup did not finish' }
}
