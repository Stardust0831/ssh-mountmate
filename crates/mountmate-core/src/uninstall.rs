//! Launch a system-owned process to remove locked files after the GUI exits.
//! No extra executable or script is left behind by the uninstaller.
use std::path::PathBuf;

#[cfg(any(windows, test))]
fn cleanup_script(
    targets: &[PathBuf],
    parent_pid: u32,
    notify: bool,
    mutex_names: &[String],
) -> Result<String, String> {
    use base64::Engine;
    let json = serde_json::to_vec(targets).map_err(|e| e.to_string())?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(json);
    let mutex_json = serde_json::to_vec(mutex_names).map_err(|e| e.to_string())?;
    let mutex_encoded = base64::engine::general_purpose::STANDARD.encode(mutex_json);
    let notify = if notify { "$true" } else { "$false" };
    Ok(format!(
        r#"
$ErrorActionPreference = 'Stop'
$targets = @([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}')) | ConvertFrom-Json)
$parent = [Diagnostics.Process]::GetProcessById({parent_pid})
$null = $parent.Handle
$mutexNames = @([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{mutex_encoded}')) | ConvertFrom-Json)
$instanceLocks = @()
foreach ($mutexName in $mutexNames) {{ $instanceLocks += [Threading.Mutex]::new($false, $mutexName) }}
[Console]::WriteLine('READY')
[Console]::Out.Flush()
$parent.WaitForExit()
$failures = @()
function Remove-Owned([string] $path) {{
    if (-not (Test-Path -LiteralPath $path)) {{ return }}
    $entry = Get-Item -LiteralPath $path -Force
    if (($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {{ throw "Linked path retained: $path" }}
    if ($entry.PSIsContainer) {{
        foreach ($child in Get-ChildItem -LiteralPath $path -Force) {{ Remove-Owned $child.FullName }}
        [IO.Directory]::Delete($path, $false)
    }} else {{
        [IO.File]::SetAttributes($path, [IO.FileAttributes]::Normal)
        [IO.File]::Delete($path)
    }}
}}
foreach ($target in $targets) {{
    $lastError = $null
    for ($attempt = 0; $attempt -lt 30; $attempt++) {{
        try {{
            $ancestor = $target
            while ($ancestor) {{
                if (Test-Path -LiteralPath $ancestor) {{
                    $entry = Get-Item -LiteralPath $ancestor -Force
                    if (($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {{ throw "Linked path retained: $ancestor" }}
                }}
                $ancestor = [IO.Path]::GetDirectoryName($ancestor)
            }}
            Remove-Owned $target; $lastError = $null; break
        }}
        catch {{ $lastError = $_.Exception.Message; Start-Sleep -Milliseconds 200 }}
    }}
    if ($lastError) {{ $failures += "$target : $lastError" }}
}}
if ({notify}) {{ Add-Type -AssemblyName System.Windows.Forms }}
if ($failures.Count -gt 0) {{
    if ({notify}) {{
    [Windows.Forms.MessageBox]::Show("Some files could not be removed / 以下文件未能清理：`n" + ($failures -join "`n"), 'SSH MountMate') | Out-Null
    }}
    exit 1
}}
if ({notify}) {{ [Windows.Forms.MessageBox]::Show('SSH MountMate has been uninstalled. / SSH MountMate 已卸载。', 'SSH MountMate') | Out-Null }}
"#
    ))
}

#[cfg(windows)]
pub fn launch_cleanup(targets: &[PathBuf]) -> Result<(), String> {
    use base64::Engine;
    use std::os::windows::process::CommandExt;
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
        time::Duration,
    };
    // Keep the same named kernel objects alive after the GUI exits, preventing
    // a second instance from reopening the profile during cleanup.
    let mut mutex_names = Vec::new();
    for paths in [
        crate::paths::AppPaths::discover(),
        crate::paths::AppPaths::legacy_windows(),
    ] {
        if paths.state_dir.exists() {
            let name = crate::app_command::windows_mutex_name(&paths.app_instance_lock())
                .map_err(|e| e.to_string())?;
            mutex_names.push(String::from_utf16_lossy(&name[..name.len() - 1]));
        }
    }
    let script = cleanup_script(targets, std::process::id(), true, &mutex_names)?;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    let system_root =
        std::env::var_os("SystemRoot").ok_or("Windows system directory is unavailable")?;
    let mut child = Command::new(
        PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    )
    .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
    .creation_flags(0x0800_0000 | 0x0000_0200)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .map_err(|e| e.to_string())?;
    let output = child
        .stdout
        .take()
        .ok_or("Uninstaller handshake unavailable")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(output).read_line(&mut line).map(|_| line);
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(line)) if line.trim() == "READY" => Ok(()),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            Err("Could not start uninstall cleanup. Files have been retained. 无法启动卸载清理，文件已保留。".into())
        }
    }
}
#[cfg(not(windows))]
pub fn launch_cleanup(_targets: &[PathBuf]) -> Result<(), String> {
    Err("This uninstall action is available on Windows".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_are_data_and_never_powershell_code() {
        let path = PathBuf::from("C:/a'$()`\";exit/SSHMountMate.exe");
        let script = cleanup_script(std::slice::from_ref(&path), 42, false, &[]).unwrap();
        assert!(!script.contains(path.to_str().unwrap()));
        assert!(script.contains("WaitForExit"));
        assert!(script.contains("ReparsePoint"));
        assert!(!script.contains("Remove-Item -Recurse"));
    }
    #[cfg(windows)]
    #[test]
    fn windows_cleanup_waits_for_parent_and_preserves_neighbors() {
        use base64::Engine;
        use std::{
            fs,
            io::{BufRead, BufReader},
            process::{Command, Stdio},
            time::Duration,
        };
        use wait_timeout::ChildExt;
        let temp = tempfile::tempdir().unwrap();
        let owned = temp.path().join("应用 'folder");
        fs::create_dir(&owned).unwrap();
        fs::write(owned.join("helper.exe"), b"owned").unwrap();
        let neighbor = temp.path().join("keep.json");
        fs::write(&neighbor, b"keep").unwrap();
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut parent = Command::new(&powershell)
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .spawn()
            .unwrap();
        let script = cleanup_script(std::slice::from_ref(&owned), parent.id(), false, &[]).unwrap();
        let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let mut cleanup = Command::new(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let output = cleanup.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(output).read_line(&mut line);
            let _ = tx.send(line);
        });
        let ready = rx.recv_timeout(Duration::from_secs(20));
        let existed = owned.exists();
        let _ = parent.kill();
        let _ = parent.wait();
        let status = cleanup.wait_timeout(Duration::from_secs(15)).unwrap();
        if status.is_none() {
            let _ = cleanup.kill();
            let _ = cleanup.wait();
        }
        assert_eq!(ready.unwrap().trim(), "READY");
        assert!(existed);
        assert!(status.unwrap().success());
        assert!(!owned.exists());
        assert_eq!(fs::read(neighbor).unwrap(), b"keep");
    }
}
