param([switch]$Background, [switch]$NoBrowser)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$executable = Join-Path $projectRoot 'target\release\internet-uptime-monitor.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Build first with cargo build --release, or double-click the downloaded release EXE.' }
$arguments = @()
if ($Background -or $NoBrowser) { $arguments += '--background' }
$launch = @{ FilePath = $executable; WorkingDirectory = $projectRoot; WindowStyle = 'Hidden' }
if ($arguments.Count) { $launch.ArgumentList = $arguments }
Start-Process @launch
Write-Output 'Native monitor started. Double-click its tray icon to reopen the window.'
