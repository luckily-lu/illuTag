param(
  [Parameter(Mandatory=$true)][ValidateSet('H','M','M-sql','X','boundary','native')][string]$Layer,
  [int]$Count = 1000,
  [int]$Width = 640,
  [int]$Height = 480,
  [string]$RunId = "",
  [int]$MaxMinutes = 240,
  [int]$TestStopAfterSeconds = 0,
  [int]$TestBuildSleepSeconds = 0,
  [double]$TestMaxPrivateGb = 4,
  [switch]$Reuse,
  [switch]$ForceFailure,
  [switch]$ForceCrash
)
$ErrorActionPreference = 'Stop'
if ($Count -le 0) { throw 'Count must be positive' }
if ($MaxMinutes -le 0) { throw 'MaxMinutes must be positive' }
if ($TestStopAfterSeconds -lt 0) { throw 'TestStopAfterSeconds must be non-negative' }
if ($TestBuildSleepSeconds -lt 0) { throw 'TestBuildSleepSeconds must be non-negative' }
if ($Reuse) { throw 'reuse mode is not implemented; hot/incremental runs must use a new isolated root' }
$perfRoot = 'D:\Ai\illuTag-refactor-work\perf-million-hires'
$repo = 'D:\Ai\illuTag-src'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if ([string]::IsNullOrWhiteSpace($RunId)) { $RunId = "run-$Layer-$Count-$stamp" }
$perfRootFull = [System.IO.Path]::GetFullPath($perfRoot).TrimEnd('\')
$runDir = [System.IO.Path]::GetFullPath((Join-Path $perfRootFull $RunId)).TrimEnd('\')
$prefix = $perfRootFull + '\'
if (-not $runDir.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase) -or
    $runDir.Equals($perfRootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "run directory escapes perf root: $runDir"
}
if ($runDir.ToLowerInvariant().Contains('danbooru')) { throw 'refusing real library path' }

function Assert-NoReparse([string]$path) {
  $current = [System.IO.Path]::GetFullPath($path)
  while ($current) {
    if (Test-Path -LiteralPath $current) {
      $item = Get-Item -LiteralPath $current -Force
      if (([int]$item.Attributes -band 0x400) -ne 0) {
        throw "perf path contains a reparse point: $current"
      }
    }
    $parent = [System.IO.Path]::GetDirectoryName($current)
    if ([string]::IsNullOrEmpty($parent) -or $parent -eq $current) { break }
    $current = $parent
  }
}

Assert-NoReparse $perfRootFull
$parent = Split-Path -Parent $runDir
Assert-NoReparse $parent
if (Test-Path $runDir) {
  if (-not $Reuse) { throw "cold run directory already exists: $runDir" }
  if (-not (Test-Path (Join-Path $runDir '.perf-isolated'))) { throw 'reuse requires ownership marker' }
  $oldConfig = Get-Content (Join-Path $runDir 'run-config.json') -Raw -ErrorAction SilentlyContinue | ConvertFrom-Json
  if (-not $oldConfig -or $oldConfig.layer -ne $Layer -or [int]$oldConfig.count -ne $Count) {
    throw 'reuse config does not match requested layer/count'
  }
} else {
  New-Item -ItemType Directory -Path $runDir | Out-Null
  Set-Content -Path (Join-Path $runDir '.perf-isolated') -Value "run_id=$RunId" -Encoding ascii
}
Assert-NoReparse $runDir
$temp = Join-Path $runDir 'temp'
New-Item -ItemType Directory -Force $temp | Out-Null

# --- budgets ---------------------------------------------------------------
$disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='D:'"
$volumeBytes = [int64]$disk.Size
$freeBytes = [int64]$disk.FreeSpace
$reserveBytes = [int64][math]::Max(40GB, [double]$volumeBytes * 0.05)
$qaCapBytes = [int64]120GB
$existingBytes = [int64]((Get-ChildItem $perfRootFull -Recurse -File -ErrorAction SilentlyContinue |
  Measure-Object Length -Sum).Sum)
if ($existingBytes -lt 0 -or $null -eq $existingBytes) { $existingBytes = 0 }
$os = Get-CimInstance Win32_OperatingSystem
$freeMemGb = [math]::Round($os.FreePhysicalMemory * 1KB / 1GB, 1)
$perImage = if ($Layer -eq 'H' -or $Layer -eq 'X') { [double]$Width * $Height * 2.2 }
  elseif ($Layer -eq 'boundary' -or $Layer -eq 'native') { 1GB }
  else { 2048 }
$estimatedBytes = [int64]($Count * $perImage)
$projectedBytes = $existingBytes + $estimatedBytes
$freeAfterBytes = $freeBytes - $estimatedBytes
$maxSeconds = [int64]$MaxMinutes * 60
$maxPrivateBytes = [int64]($TestMaxPrivateGb * 1GB)
if ($freeAfterBytes -lt $reserveBytes) {
  $msg = "resource gate failed: projected free $([math]::Round($freeAfterBytes/1GB,2)) GB < reserve $([math]::Round($reserveBytes/1GB,2)) GB; estimated $([math]::Round($estimatedBytes/1GB,2)) GB, free $([math]::Round($freeBytes/1GB,2)) GB"
  Set-Content (Join-Path $runDir 'RUN-BLOCKED.txt') $msg
  throw $msg
}
if ($projectedBytes -gt $qaCapBytes) {
  $msg = "resource gate failed: QA total $([math]::Round($projectedBytes/1GB,2)) GB > cap $([math]::Round($qaCapBytes/1GB,2)) GB (existing $([math]::Round($existingBytes/1GB,2)) GB + estimated $([math]::Round($estimatedBytes/1GB,2)) GB)"
  Set-Content (Join-Path $runDir 'RUN-BLOCKED.txt') $msg
  throw $msg
}
if ($freeMemGb -lt 8) {
  $msg = "resource gate failed: free physical memory ${freeMemGb} GB < 8 GB"
  Set-Content (Join-Path $runDir 'RUN-BLOCKED.txt') $msg
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
$envInfo = [ordered]@{
  run_id = $RunId; layer = $Layer; count = $Count; width = $Width; height = $Height
  git_head = $git; git_dirty = ($status.Length -gt 0); rustc = $rustVersion; cargo = $cargo
  os = $os.Caption; cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
  logical_cpus = $env:NUMBER_OF_PROCESSORS; free_memory_gb_at_start = $freeMemGb
  disk_free_gb_at_start = [math]::Round($freeBytes/1GB, 2)
  volume_gb = [math]::Round($volumeBytes/1GB, 2)
  reserve_gb = [math]::Round($reserveBytes/1GB, 2)
  qa_cap_gb = [math]::Round($qaCapBytes/1GB, 2)
  existing_qa_bytes = $existingBytes; estimated_data_bytes = $estimatedBytes
  temp_root = $temp; start = (Get-Date).ToString('o')
}
$envInfo | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $runDir 'environment.json')
[ordered]@{
  layer=$Layer; count=$Count; width=$Width; height=$Height
  seed='fixed-mix-0x9e3779b97f4a7c15'; test_root=$temp
  max_seconds=$maxSeconds; reserve_bytes=$reserveBytes; qa_cap_bytes=$qaCapBytes
  max_private_bytes=$maxPrivateBytes
  test_stop_after_seconds=$TestStopAfterSeconds
  test_build_sleep_seconds=$TestBuildSleepSeconds
  existing_qa_bytes=$existingBytes; estimated_data_bytes=$estimatedBytes
  cold_run=(-not $Reuse); force_failure=[bool]$ForceFailure; force_crash=[bool]$ForceCrash
} | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $runDir 'run-config.json')

$test = switch ($Layer) {
  'H' { 'library::perf_tests::perf_h_scan_cascade' }
  'M' { 'library::perf_tests::perf_m_numeric_scale' }
  'M-sql' { 'library::perf_tests::perf_m_sql_paging' }
  'X' { 'library::perf_tests::perf_x_dense' }
  'native' { 'duplicates::cascade_tests::perf_boundary_native_limits' }
  'boundary' {
    if ($Count -eq 24) { 'library::perf_tests::perf_boundary_24mp' }
    elseif ($Count -eq 36) { 'library::perf_tests::perf_boundary_over32mp' }
    else { 'library::perf_tests::perf_boundary_16mp' }
  }
}
$forceLine = if ($ForceFailure) { 'set ILLUTAG_PERF_FORCE_FAIL=1' } else { 'set ILLUTAG_PERF_FORCE_FAIL=' }
$crashLine = if ($ForceCrash) { 'set ILLUTAG_PERF_FORCE_CRASH=1' } else { 'set ILLUTAG_PERF_FORCE_CRASH=' }
$buildSleepLine = if ($TestBuildSleepSeconds -gt 0) {
  "powershell -NoProfile -Command Start-Sleep -Seconds $TestBuildSleepSeconds"
} else { 'rem no build sleep' }
$out = Join-Path $runDir 'runner.out'
$err = Join-Path $runDir 'runner.err'
$exitCodeFile = Join-Path $runDir 'exitcode.txt'
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
set ILLUTAG_PERF_STOP_FILE=$runDir\STOP
set ILLUTAG_TEST_ROOT=$temp
set TEMP=$temp
set TMP=$temp
$forceLine
$crashLine
$buildSleepLine
"$cargo" test --release --manifest-path "$repo\src-tauri\Cargo.toml" --lib $test -- --ignored --exact --nocapture --test-threads=1 > "$out" 2> "$err"
set RC=%ERRORLEVEL%
echo %RC% > "$exitCodeFile"
exit /b %RC%
"@ | Set-Content -Encoding ascii $cmdPath

$res = Join-Path $runDir 'resources.jsonl'
$stopFile = Join-Path $runDir 'STOP'
Remove-Item $res, $stopFile, $out, $err, $exitCodeFile -ErrorAction SilentlyContinue
$started = Get-Date
$proc = Start-Process -FilePath 'cmd.exe' -ArgumentList @('/c', $cmdPath) -WorkingDirectory $repo -NoNewWindow -PassThru
$peakWs = 0L; $peakPrivate = 0L; $last = Get-Date
$ownedPid = 0; $ownedStart = $null; $ownedName = ''
$stopReason = $null; $unknownExit = $false; $residualPids = @()
$knownPids = New-Object 'System.Collections.Generic.HashSet[int]'
$knownStarts = @{}
$knownPids.Add([int]$proc.Id) | Out-Null
try { $knownStarts[[int]$proc.Id] = (Get-Process -Id $proc.Id -ErrorAction Stop).StartTime } catch {}

function Get-QaBytes {
  return [int64]((Get-ChildItem $perfRootFull -Recurse -File -ErrorAction SilentlyContinue |
    Measure-Object Length -Sum).Sum)
}

function Get-DescendantPids([int]$rootPid) {
  $all = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Select-Object ProcessId,ParentProcessId)
  $byParent = @{}
  foreach ($row in $all) {
    $parent = [int]$row.ParentProcessId
    if (-not $byParent.ContainsKey($parent)) { $byParent[$parent] = @() }
    $byParent[$parent] += [int]$row.ProcessId
  }
  $result = New-Object System.Collections.Generic.List[int]
  $queue = New-Object System.Collections.Generic.Queue[int]
  $queue.Enqueue($rootPid)
  while ($queue.Count -gt 0) {
    $current = $queue.Dequeue()
    if ($byParent.ContainsKey($current)) {
      foreach ($child in $byParent[$current]) {
        if (-not $result.Contains($child)) { $result.Add($child); $queue.Enqueue($child) }
      }
    }
  }
  return $result
}

function Track-Descendants {
  foreach ($id in @(Get-DescendantPids $proc.Id)) {
    if (-not $knownPids.Contains([int]$id)) {
      try {
        $p = Get-Process -Id $id -ErrorAction Stop
        $knownPids.Add([int]$id) | Out-Null
        $knownStarts[[int]$id] = $p.StartTime
      } catch {}
    }
  }
}

function Stop-OwnedTree {
  Track-Descendants
  $pids = @($knownPids)
  if ($ownedPid -gt 0) { $pids += $ownedPid }
  foreach ($id in ($pids | Sort-Object -Descending -Unique)) {
    try {
      $p = Get-Process -Id $id -ErrorAction Stop
      if ($knownStarts.ContainsKey([int]$id) -and $p.StartTime -eq $knownStarts[[int]$id]) {
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
      }
    } catch {}
  }
  & cmd.exe /c "taskkill /PID $($proc.Id) /T /F >nul 2>&1" | Out-Null
}

function Request-Stop([string]$reason) {
  if (-not $script:stopReason) { $script:stopReason = $reason }
  Set-Content $stopFile $script:stopReason -Encoding ascii
}

$qaUsed = $existingBytes
$lastQaCheck = $started
while (-not $proc.HasExited) {
  $now = Get-Date
  Track-Descendants
  if (($now - $started).TotalSeconds -gt $maxSeconds) { Request-Stop 'wall-time limit' }
  if ($TestStopAfterSeconds -gt 0 -and ($now - $started).TotalSeconds -gt $TestStopAfterSeconds) {
    Request-Stop 'test stop threshold'
  }
  $freeNow = [int64](Get-PSDrive D).Free
  if ($freeNow -lt $reserveBytes) { Request-Stop 'disk reserve threshold' }
  if (($now - $lastQaCheck).TotalSeconds -ge 5) {
    $qaUsed = Get-QaBytes
    $lastQaCheck = $now
    $freeMemNow = [math]::Round((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory * 1KB / 1GB, 1)
    if ($qaUsed -gt $qaCapBytes) { Request-Stop 'QA actual-usage cap' }
    if ($freeMemNow -lt 8) { Request-Stop 'free physical memory < 8 GiB' }
  }

  $pidFile = Join-Path $runDir 'test.pid'
  if (Test-Path $pidFile) {
    $testPid = [int](Get-Content $pidFile -ErrorAction SilentlyContinue | Select-Object -First 1)
    if ($testPid -gt 0) {
      try {
        $p = Get-Process -Id $testPid -ErrorAction Stop
        if ($ownedPid -eq 0) {
          $ownedPid = $testPid; $ownedStart = $p.StartTime; $ownedName = $p.ProcessName
        }
        $ws = $p.WorkingSet64; $priv = $p.PrivateMemorySize64
        if ($ws -gt $peakWs) { $peakWs = $ws }
        if ($priv -gt $peakPrivate) { $peakPrivate = $priv }
        if ($priv -gt $maxPrivateBytes) { Request-Stop 'private bytes threshold' }
        if (((Get-Date) - $last).TotalMilliseconds -ge 1000) {
          [pscustomobject]@{
            ts=(Get-Date).ToString('o'); pid=$testPid; working_set=$ws
            private_bytes=$priv; cpu_seconds=$p.CPU; free_bytes=$freeNow; qa_used_bytes=$qaUsed
          } | ConvertTo-Json -Compress | Add-Content $res
          $last = Get-Date
        }
      } catch {}
    }
  }
  if ($stopReason) {
    Start-Sleep -Seconds 10
    Stop-OwnedTree
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline) {
      Track-Descendants
      $alive = @()
      foreach ($id in $knownPids) {
        try {
          $p = Get-Process -Id $id -ErrorAction Stop
          if ($knownStarts.ContainsKey([int]$id) -and $p.StartTime -eq $knownStarts[[int]$id]) {
            $alive += $id
          }
        } catch {}
      }
      if ($alive.Count -eq 0) { break }
      Start-Sleep -Milliseconds 200
    }
    Track-Descendants
    $residualPids = @()
    foreach ($id in $knownPids) {
      try {
        $p = Get-Process -Id $id -ErrorAction Stop
        if ($knownStarts.ContainsKey([int]$id) -and $p.StartTime -eq $knownStarts[[int]$id]) {
          $residualPids += $id
        }
      } catch {}
    }
    $residualPids = @($residualPids | Sort-Object -Unique)
    break
  }
  Start-Sleep -Milliseconds 100
}
if (-not $proc.HasExited) {
  $proc.WaitForExit()
}
$proc.WaitForExit()
$proc.Refresh()
$exit = $null
if (Test-Path $exitCodeFile) {
  $rawExit = (Get-Content $exitCodeFile -ErrorAction SilentlyContinue | Select-Object -First 1)
  $parsed = 0
  if ([int]::TryParse("$rawExit".Trim(), [ref]$parsed)) { $exit = $parsed }
}
if ($null -eq $exit) { $exit = 255; $unknownExit = $true }
if ($stopReason) { $exit = 124 }
$elapsed = (Get-Date) - $started

$jsonFiles = @(Get-ChildItem $runDir -Filter '*.json' -Recurse |
  Where-Object { $_.Name -notin @('environment.json','run-config.json','summary.json') })
$manifest = Get-ChildItem $runDir -Filter 'manifest.jsonl' -Recurse | Select-Object -First 1
$manifestHash = if ($manifest) { (Get-FileHash $manifest.FullName -Algorithm SHA256).Hash } else { $null }
$usedBytes = [int64]((Get-ChildItem $runDir -Recurse -File | Measure-Object Length -Sum).Sum)
$freeEnd = [int64](Get-PSDrive D).Free
$failure = $null
if ($stopReason -and $residualPids.Count -gt 0) {
  $failure = "stopped with residual PIDs: $($residualPids -join ',') ($stopReason)"
}
elseif ($stopReason) { $failure = "stopped: $stopReason" }
elseif ($unknownExit) { $failure = 'unknown child exit code' }
elseif ($exit -ne 0) { $failure = "child exit code $exit" }
elseif ($jsonFiles.Count -eq 0) { $failure = 'required result JSON is missing' }
$passed = ($null -eq $failure)
$qaEnd = Get-QaBytes
$summary = [ordered]@{
  run_id = $RunId; layer = $Layer; count = $Count; resolution = "$Width x $Height"
  exit_code = $exit; passed = $passed; failure_reason = $failure
  stopped = ($null -ne $stopReason -and $residualPids.Count -eq 0)
  stop_reason = $stopReason; residual_pids = $residualPids
  elapsed_seconds = [math]::Round($elapsed.TotalSeconds, 3)
  peak_working_set_bytes = $peakWs; peak_private_bytes = $peakPrivate
  peak_sampling_note = 'sampled every <=100ms/1s; value is a lower bound, not OS peak'
  manifest_sha256 = $manifestHash; run_bytes = $usedBytes
  qa_bytes_at_end = $qaEnd
  disk_free_gb_at_end = [math]::Round($freeEnd/1GB, 2)
  budget = [ordered]@{ max_seconds=$maxSeconds; reserve_gb=[math]::Round($reserveBytes/1GB,2); qa_cap_gb=[math]::Round($qaCapBytes/1GB,2); max_private_bytes=$maxPrivateBytes; qa_sampling_seconds=5 }
  result_files = @($jsonFiles | ForEach-Object { $_.FullName })
  completed_at = (Get-Date).ToString('o')
}
$summary | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $runDir 'summary.json')
$md = @"
# $RunId

- layer: $Layer
- count: $Count
- resolution: $Width x $Height
- passed: $passed
- exit: $exit
- failure: $failure
- stop: $stopReason
- elapsed: $([math]::Round($elapsed.TotalSeconds, 3)) s
- peak working set: $peakWs bytes
- peak private bytes: $peakPrivate bytes
- manifest SHA-256: $manifestHash
- data bytes: $usedBytes
- budget: max $maxSeconds s, reserve $([math]::Round($reserveBytes/1GB,2)) GB, QA cap $([math]::Round($qaCapBytes/1GB,2)) GB
- git: $git $(if ($status.Length -gt 0) { '(dirty)' } else { '(clean)' })

## Result files
$((@($jsonFiles | ForEach-Object { "- $($_.FullName)" })) -join "`n")

## Runner output tail
``````
$((Get-Content $out -Tail 40 -ErrorAction SilentlyContinue) -join "`n")
``````
"@
$md | Set-Content (Join-Path $runDir 'summary.md')
if (-not $passed) {
  throw "perf run failed: $failure (summary: $runDir\summary.json)"
}
Write-Output "RUN=$runDir EXIT=$exit PEAK=$peakWs ELAPSED=$([math]::Round($elapsed.TotalSeconds,1))s"
