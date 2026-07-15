param(
  [Parameter(Mandatory = $true)]
  [string] $Binary
)

$ErrorActionPreference = 'Stop'
$binary = (Resolve-Path $Binary).Path
$testRoot = Join-Path $env:RUNNER_TEMP "ssh-mountmate-gui-e2e-$PID"
$stdout = Join-Path $testRoot 'gui.stdout'
$stderr = Join-Path $testRoot 'gui.stderr'
$trace = Join-Path $testRoot 'gui.trace'
$gui = $null
$succeeded = $false

function Wait-Until([scriptblock] $Condition, [int] $Attempts = 200) {
  for ($attempt = 0; $attempt -lt $Attempts; $attempt++) {
    if (& $Condition) { return }
    Start-Sleep -Milliseconds 100
  }
  throw 'Timed out waiting for the Windows GUI integration-test condition'
}

function Invoke-SecondInstance([string[]] $Arguments) {
  $processInfo = [System.Diagnostics.ProcessStartInfo]::new($binary)
  $processInfo.UseShellExecute = $false
  $processInfo.CreateNoWindow = $true
  $Arguments | ForEach-Object { $processInfo.ArgumentList.Add($_) }
  $process = [System.Diagnostics.Process]::Start($processInfo)
  if (-not $process.WaitForExit(15000)) {
    $process.Kill($true)
    throw "Second SSH MountMate instance timed out: $($Arguments -join ' ')"
  }
  if ($process.ExitCode -ne 0) {
    throw "Second SSH MountMate instance failed with $($process.ExitCode): $($Arguments -join ' ')"
  }
}

function Trace-Contains([string] $Text) {
  return (Test-Path $trace) -and ((Get-Content $trace -Raw) -match [regex]::Escape($Text))
}

function Trace-LineCount([string] $Text) {
  if (-not (Test-Path $trace)) { return 0 }
  return @(Get-Content $trace | Where-Object { $_ -eq $Text }).Count
}

function Trace-PrefixCount([string] $Prefix) {
  if (-not (Test-Path $trace)) { return 0 }
  return @(Get-Content $trace | Where-Object { $_.StartsWith($Prefix) }).Count
}

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class SSHMountMateWindowTest {
    private delegate bool EnumWindowsCallback(IntPtr window, IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsCallback callback, IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr window, StringBuilder text, int count);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindowVisible(IntPtr window);

    public static IntPtr FindMainWindow(uint processId) {
        IntPtr result = IntPtr.Zero;
        EnumWindows((window, parameter) => {
            GetWindowThreadProcessId(window, out uint owner);
            if (owner != processId || !IsWindowVisible(window)) {
                return true;
            }
            StringBuilder title = new StringBuilder(256);
            GetWindowText(window, title, title.Capacity);
            if (title.ToString().StartsWith("SSH MountMate ", StringComparison.Ordinal)) {
                result = window;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
'@

try {
  New-Item -ItemType Directory -Force $testRoot | Out-Null
  $env:HOME = Join-Path $testRoot 'home'
  $env:USERPROFILE = $env:HOME
  $env:APPDATA = Join-Path $testRoot 'roaming'
  $env:LOCALAPPDATA = Join-Path $testRoot 'local'
  $env:SSH_MOUNTMATE_TRACE_FILE = $trace
  $env:SSH_MOUNTMATE_E2E_NATIVE_SMOKE = '1'
  New-Item -ItemType Directory -Force $env:HOME, $env:APPDATA, $env:LOCALAPPDATA | Out-Null

  $gui = Start-Process -FilePath $binary -PassThru `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr
  $commandState = Join-Path $env:LOCALAPPDATA 'rsshmount/State/app-command.json'
  Wait-Until { (Test-Path $commandState -PathType Leaf) -and ((Get-Item $commandState).Length -gt 0) }
  Wait-Until { [SSHMountMateWindowTest]::FindMainWindow($gui.Id) -ne [IntPtr]::Zero }
  $initialWindow = [SSHMountMateWindowTest]::FindMainWindow($gui.Id)
  Wait-Until { Trace-Contains 'tray initialized' }
  Wait-Until { Trace-Contains 'taskbar progress updated: Normal { completed: 1, total: 2 }' }
  Wait-Until { Trace-Contains 'native notification submitted' }

  Invoke-SecondInstance @('--show-transfers')
  Wait-Until { Trace-Contains 'ipc-server received ShowTransfers' }
  $matching = @(Get-Process | Where-Object {
    try { $_.Path -eq $binary } catch { $false }
  })
  if ($matching.Count -ne 1 -or $matching[0].Id -ne $gui.Id) {
    throw "Expected one SSH MountMate GUI process, found $($matching.Count)"
  }

  if (-not [SSHMountMateWindowTest]::PostMessage($initialWindow, 0x0112, [IntPtr]0xF060, [IntPtr]::Zero)) {
    throw "Could not post SC_CLOSE to SSH MountMate: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
  }
  Wait-Until { Trace-Contains 'closing main window to tray' }
  Wait-Until { -not [SSHMountMateWindowTest]::IsWindowVisible($initialWindow) }
  if ($gui.HasExited) { throw 'SSH MountMate exited instead of remaining in the tray' }

  Invoke-SecondInstance @('--show-main')
  Wait-Until { Trace-Contains 'ipc-server received ShowMain' }
  Wait-Until { Trace-Contains 'opening replacement main window' }
  Wait-Until { (Trace-PrefixCount 'main window opened ') -ge 2 }
  Wait-Until { [SSHMountMateWindowTest]::FindMainWindow($gui.Id) -ne [IntPtr]::Zero }
  $restoredWindow = [SSHMountMateWindowTest]::FindMainWindow($gui.Id)
  if ($gui.Id -ne $matching[0].Id) { throw 'Restored window belongs to another process' }
  Wait-Until {
    (Trace-LineCount 'taskbar progress updated: Normal { completed: 1, total: 2 }') -ge 2
  }
  $matching = @(Get-Process | Where-Object {
    try { $_.Path -eq $binary } catch { $false }
  })
  if ($matching.Count -ne 1 -or $matching[0].Id -ne $gui.Id) {
    throw "Restoring the main window created another GUI process; found $($matching.Count)"
  }

  if ((Test-Path $stdout) -and (Get-Item $stdout).Length -ne 0) {
    throw 'Windows GUI unexpectedly wrote to stdout'
  }
  if ((Test-Path $stderr) -and ((Get-Content $stderr -Raw) -match 'panicked')) {
    throw "Windows GUI reported a native integration failure:`n$(Get-Content $stderr -Raw)"
  }
  $traceContent = Get-Content $trace -Raw
  if ($traceContent -match 'taskbar progress failed|tray unavailable') {
    throw "Windows GUI native integration trace reported a failure:`n$traceContent"
  }
  Write-Host "Windows GUI integration passed: pid=$($gui.Id) initial=$initialWindow restored=$restoredWindow"
  $succeeded = $true
} finally {
  if ($gui -and -not $gui.HasExited) {
    Stop-Process -Id $gui.Id -Force
    $gui.WaitForExit()
  }
  if (-not $succeeded) {
    if (Test-Path $trace) {
      Write-Host '--- SSH MountMate event trace ---'
      Get-Content $trace
    }
    if (Test-Path $stderr) {
      Write-Host '--- SSH MountMate stderr ---'
      Get-Content $stderr
    }
  }
}
