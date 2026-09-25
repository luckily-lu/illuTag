# Collect illuTag startup samples (default: 5 runs) and print P50/P90.
#
# WARM run:  close illuTag first, then run as-is.
# COLD run:  reboot the PC, wait for the desktop to settle (~30s), then
#            double-click collect-cold-start.cmd immediately.
#
# The script force-terminates each test instance between samples. It never
# writes to the database. Output: $OutDir (default %TEMP%\illutag-startup-perf).

param(
  [string]$Label = "cold",
  [int]$Runs = 5,
  [int]$DurationSec = 30,
  [string]$Exe = "D:\Ai\illuTag-danbooru-tag-query\illutag.exe",
  [string]$WorkDir = "D:\Ai\illuTag-danbooru-tag-query",
  [string]$OutDir = "$env:TEMP\illutag-startup-perf"
)

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

if (Get-Process illutag -ErrorAction SilentlyContinue) {
  Write-Host "[!] illuTag is already running. Close it before measuring." -ForegroundColor Yellow
  exit 1
}
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
  Write-Host "[!] Node.js >= 22 not found on PATH; needed for console capture." -ForegroundColor Red
  exit 1
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$env:ILLUTAG_EXE = $Exe
$env:ILLUTAG_CWD = $WorkDir
$env:ILLUTAG_OUT = $OutDir

Write-Host "Running $Runs startup sample(s) -> $OutDir"
for ($i = 1; $i -le $Runs; $i++) {
  Write-Host ("  run {0}{1} ..." -f $Label, $i)
  node "$here\harness.mjs" "$Label$i" ($DurationSec * 1000) *> "$OutDir\$Label$i.harness.log"
}

Write-Host "`n=== aggregate ==="
python "$here\aggregate.py" $Label
