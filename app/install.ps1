$ErrorActionPreference = "Stop"

$Prefix = if ($args.Count -gt 0) { $args[0] } else { Join-Path $env:LOCALAPPDATA "rsshmount" }
$Src = Split-Path -Parent $MyInvocation.MyCommand.Path

New-Item -ItemType Directory -Force -Path $Prefix | Out-Null

Copy-Item (Join-Path $Src "rsshmount.py") (Join-Path $Prefix "rsshmount.py") -Force
Copy-Item (Join-Path $Src "rsshmount_gui.pyw") (Join-Path $Prefix "rsshmount_gui.pyw") -Force
Copy-Item (Join-Path $Src "rsshmount.cmd") (Join-Path $Prefix "rsshmount.cmd") -Force
Copy-Item (Join-Path $Src "rsshmount-gui.cmd") (Join-Path $Prefix "rsshmount-gui.cmd") -Force

Write-Host "installed: $(Join-Path $Prefix 'rsshmount.cmd')"
Write-Host "gui:       $(Join-Path $Prefix 'rsshmount-gui.cmd')"
