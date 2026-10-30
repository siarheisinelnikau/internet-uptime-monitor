param([switch]$NoBrowser)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$executable = Join-Path $projectRoot 'target\release\internet-uptime-monitor.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Build first with cargo build --release, or double-click the downloaded release EXE.' }
$arguments = @()
if ($NoBrowser) { $arguments += '--no-browser' }
$launch = @{ FilePath = $executable; WorkingDirectory = $projectRoot; WindowStyle = 'Hidden' }
if ($arguments.Count) { $launch.ArgumentList = $arguments }
Start-Process @launch
Write-Output 'Monitor started in the background. Dashboard: http://127.0.0.1:8765'
