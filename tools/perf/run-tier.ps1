param(
  [Parameter(Mandatory=$true)][ValidateSet('H','M','M-sql','X','boundary')][string]$Layer,
  [int]$Count = 1000,
  [int]$Width = 640,
  [int]$Height = 480,
  [string]$RunId = ""
)
$ErrorActionPreference = 'Stop'
$perfRoot = 'D:\Ai\illuTag-refactor-work\perf-million-hires'
$repo = 'D:\Ai\illuTag-src'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if ([string]::IsNullOrWhiteSpace($RunId)) { $RunId = "run-$Layer-$Count-$stamp" }
$runDir = Join-Path $perfRoot $RunId
$runDir = [System.IO.Path]::GetFullPath($runDir)
if (-not $runDir.StartsWith([System.IO.Path]::GetFullPath($perfRoot), [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "run directory escapes perf root: $runDir"
}
if ($runDir.ToLowerInvariant().Contains('danbooru')) { throw 'refusing real library path' }
New-Item -ItemType Directory -Force $runDir | Out-Null
$temp = Join-Path $runDir 'temp'
New-Item -ItemType Directory -Force $temp | Out-Null
New-Item -ItemType File -Force (Join-Path $runDir '.perf-isolated') | Out-Null

# --- resource gate -----------------------------------------------------------
$drive = Get-PSDrive D
$freeGb = [math]::Round($drive.Free / 1GB, 1)
$os = Get-CimInstance Win32_OperatingSystem
$freeMemGb = [math]::Round($os.FreePhysicalMemory * 1KB / 1GB, 1)
$perImage = if ($Layer -eq 'H' -or $Layer -eq 'X') { [double]$Width * $Height * 2.2 } else { 2048 }
$estimatedGb = [math]::Round(($Count * $perImage) / 1GB, 2)
$requiredGb = [math]::Max(5, [math]::Round($estimatedGb * 1.3, 1))
if ($Layer -eq 'boundary') { $requiredGb = 4 }
if ($freeGb -lt $requiredGb) {
  $msg = "resource gate failed: estimated ${estimatedGb} GB, need ${requiredGb} GB, D: free ${freeGb} GB"
  $msg | Set-Content (Join-Path $runDir 'RUN-BLOCKED.txt')
  throw $msg
}
if ($freeMemGb -lt 8) {
  $msg = "resource gate failed: free physical memory ${freeMemGb} GB < 8 GB"
  $msg | Set-Content (Join-Path $runDir 'RUN-BLOCKED.txt')
  throw $msg
}

$git = (& git -C $repo rev-parse HEAD).Trim()
$status = (& git -C $repo status --porcelain) -join "`n"
$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
$vs = ''
if (Test-Path $vswhere) {
  $found = & $vswhere -latest -products * -property installationPath 2>$null | Select-Object -First 1
  if ($null -ne $found) { $vs = "$found".Trim() }
}
$vcvars = if ($vs) { Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat' } else { '' }
if (-not $vcvars -or -not (Test-Path $vcvars)) {
  $vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
}
$toolchainCargo = "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe"
$toolchainRustc = "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\rustc.exe"
$cargo = if (Test-Path $toolchainCargo) { $toolchainCargo } elseif (Get-Command cargo -ErrorAction SilentlyContinue) { (Get-Command cargo).Source } else { 'cargo' }
$rustc = if (Test-Path $toolchainRustc) { $toolchainRustc } else { 'rustc' }
$rustVersion = if (Test-Path $rustc) { (& $rustc --version) } else { 'rustc unavailable' }
$sqlite = (Get-ChildItem "$repo\src-tauri\target\release\build" -Recurse -Filter 'libsqlite3*.rlib' -ErrorAction SilentlyContinue | Select-Object -First 1).Name
$envInfo = [ordered]@{
  run_id = $RunId; layer = $Layer; count = $Count; width = $Width; height = $Height
  git_head = $git; git_dirty = ($status.Length -gt 0); rustc = $rustVersion; cargo = $cargo
  os = $os.Caption; cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
  logical_cpus = $env:NUMBER_OF_PROCESSORS; free_memory_gb_at_start = $freeMemGb
  disk_free_gb_at_start = $freeGb; estimated_data_gb = $estimatedGb; required_gb = $requiredGb
  temp_root = $temp; start = (Get-Date).ToString('o'); sqlite_rlib = $sqlite
}
$envInfo | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $runDir 'environment.json')
[ordered]@{ layer=$Layer; count=$Count; width=$Width; height=$Height; seed='fixed-mix-0x9e3779b97f4a7c15'; test_root=$temp } |
  ConvertTo-Json -Depth 4 | Set-Content (Join-Path $runDir 'run-config.json')

$test = switch ($Layer) {
  'H' { 'library::perf_tests::perf_h_scan_cascade' }
  'M' { 'library::perf_tests::perf_m_numeric_scale' }
  'M-sql' { 'library::perf_tests::perf_m_sql_paging' }
  'X' { 'library::perf_tests::perf_x_dense' }
  'boundary' {
    if ($Count -eq 24) { 'library::perf_tests::perf_boundary_24mp' }
    elseif ($Count -eq 36) { 'library::perf_tests::perf_boundary_over32mp' }
    else { 'library::perf_tests::perf_boundary_16mp' }
  }
}
$cmdPath = Join-Path $runDir 'run.cmd'
@"
@echo off
call "$vcvars" >nul
set PATH=$($cargo | Split-Path);%PATH%
set ILLUTAG_PERF_RUN_DIR=$runDir
set ILLUTAG_PERF_COUNT=$Count
set ILLUTAG_PERF_N=$Count
set ILLUTAG_PERF_WIDTH=$Width
set ILLUTAG_PERF_HEIGHT=$Height
set ILLUTAG_TEST_ROOT=$temp
set TEMP=$temp
set TMP=$temp
"$cargo" test --release --manifest-path "$repo\src-tauri\Cargo.toml" --lib $test -- --ignored --exact --nocapture --test-threads=1
"@ | Set-Content -Encoding ascii $cmdPath

$out = Join-Path $runDir 'runner.out'
$err = Join-Path $runDir 'runner.err'
$res = Join-Path $runDir 'resources.jsonl'
if (Test-Path $res) { Remove-Item $res }
$started = Get-Date
$proc = Start-Process -FilePath 'cmd.exe' -ArgumentList @('/c', $cmdPath) -WorkingDirectory $repo -NoNewWindow -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
$peakWs = 0L; $peakPrivate = 0L; $last = Get-Date
while (-not $proc.HasExited) {
  $pidFile = Join-Path $runDir 'test.pid'
  if (Test-Path $pidFile) {
    $testPid = [int](Get-Content $pidFile -ErrorAction SilentlyContinue | Select-Object -First 1)
    if ($testPid -gt 0) {
      try {
        $p = Get-Process -Id $testPid -ErrorAction Stop
        $ws = $p.WorkingSet64; $priv = $p.PrivateMemorySize64
        if ($ws -gt $peakWs) { $peakWs = $ws }
        if ($priv -gt $peakPrivate) { $peakPrivate = $priv }
        if (((Get-Date) - $last).TotalMilliseconds -ge 1000) {
          [pscustomobject]@{ ts=(Get-Date).ToString('o'); pid=$testPid; working_set=$ws; private_bytes=$priv; cpu_seconds=$p.CPU } |
            ConvertTo-Json -Compress | Add-Content $res
          $last = Get-Date
        }
      } catch {}
    }
  }
  Start-Sleep -Milliseconds 100
}
$proc.WaitForExit()
$elapsed = (Get-Date) - $started
$exit = $proc.ExitCode

$jsonFiles = Get-ChildItem $runDir -Filter '*.json' -Recurse | Where-Object { $_.Name -notin @('environment.json','run-config.json','summary.json') }
$manifest = Get-ChildItem $runDir -Filter 'manifest.jsonl' -Recurse | Select-Object -First 1
$manifestHash = if ($manifest) { (Get-FileHash $manifest.FullName -Algorithm SHA256).Hash } else { $null }
$usedBytes = (Get-ChildItem $runDir -Recurse -File | Measure-Object Length -Sum).Sum
$freeEnd = (Get-PSDrive D).Free
$summary = [ordered]@{
  run_id = $RunId; layer = $Layer; count = $Count; resolution = "$Width x $Height"
  exit_code = $exit; elapsed_seconds = [math]::Round($elapsed.TotalSeconds, 3)
  peak_working_set_bytes = $peakWs; peak_private_bytes = $peakPrivate
  manifest_sha256 = $manifestHash; run_bytes = $usedBytes
  disk_free_gb_at_end = [math]::Round($freeEnd / 1GB, 1)
  result_files = @($jsonFiles | ForEach-Object { $_.FullName })
  completed_at = (Get-Date).ToString('o')
}
$summary | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $runDir 'summary.json')

$md = @"
# $RunId

- layer: $Layer
- count: $Count
- resolution: $Width x $Height
- exit: $exit
- elapsed: $([math]::Round($elapsed.TotalSeconds, 3)) s
- peak working set: $peakWs bytes
- peak private bytes: $peakPrivate bytes
- manifest SHA-256: $manifestHash
- data bytes: $usedBytes
- disk free at end: $([math]::Round($freeEnd / 1GB, 1)) GB
- git: $git $(if ($status.Length -gt 0) { '(dirty)' } else { '(clean)' })

## Result files
$((@($jsonFiles | ForEach-Object { "- $($_.FullName)" })) -join "`n")

## Runner output tail
``````
$((Get-Content $out -Tail 40 -ErrorAction SilentlyContinue) -join "`n")
``````
"@
$md | Set-Content (Join-Path $runDir 'summary.md')
Write-Output "RUN=$runDir EXIT=$exit PEAK=$peakWs ELAPSED=$([math]::Round($elapsed.TotalSeconds,1))s"
