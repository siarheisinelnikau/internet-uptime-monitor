param([switch]$Remove)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$executable = Join-Path $projectRoot 'target\release\internet-uptime-monitor.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Build first with cargo build --release, or use the downloaded EXE with --enable-startup.' }
$arguments = if ($Remove) { '--disable-startup' } else { '--enable-startup' }
$process = Start-Process -FilePath $executable -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
if ($process.ExitCode -ne 0) { throw 'Could not update automatic startup.' }
Write-Output 'Automatic startup preference updated.'
