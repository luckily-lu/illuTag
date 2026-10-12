#Requires -Version 7.4
param([switch]$Execute)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$LiveRoot = 'D:\Ai\illuTag-danbooru-tag-query'
$StageRoot = 'D:\Ai\illuTag-refactor-work\deployment-stage\illuTag-danbooru-tag-query'
$RollbackRoot = 'D:\Ai\illuTag-danbooru-tag-query.rollback-20261012-1.0.7'
$PackageRoot = 'D:\Ai\illuTag-refactor-work\release-stable\illuTag-1.1.0'
$LegacyExeSha256 = 'aee1814a8eafc011219ca26228004066ccdf0119ae01c21971dfe3091c7fed4f'
$DisabledExeName = 'illutag.exe.rollback-disabled'

function Get-NormalizedPath([string]$Path) {
    if (!$Path) { throw 'Path must not be empty' }
    if ($Path.StartsWith('\\?\')) { $Path = $Path.Substring(4) }
    if (![IO.Path]::IsPathFullyQualified($Path)) { throw "Absolute path required: $Path" }
    return [IO.Path]::GetFullPath($Path).TrimEnd('\')
}

function Assert-Path([string]$Path, [string]$ExpectedParent, [switch]$MustExist, [switch]$MustBeAbsent) {
    $absolute = Get-NormalizedPath $Path
    if ([IO.Path]::GetPathRoot($absolute) -ine 'D:\') { throw "Deployment paths must stay on D: $absolute" }
    $parent = Split-Path -Parent $absolute
    if ($parent -ine (Get-NormalizedPath $ExpectedParent)) { throw "Unexpected parent directory: $absolute" }
    $exists = Test-Path -LiteralPath $absolute
    if ($MustExist -and !$exists) { throw "Required directory does not exist: $absolute" }
    if ($MustBeAbsent -and $exists) { throw "Refusing to overwrite existing directory: $absolute" }
    if ($exists -and !(Test-Path -LiteralPath $absolute -PathType Container)) { throw "Directory required: $absolute" }
    $ancestor = if ($exists) { $absolute } else { $parent }
    while ($ancestor) {
        $item = Get-Item -LiteralPath $ancestor -Force
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Directory links are not allowed: $ancestor" }
        $ancestor = Split-Path -Parent $ancestor
    }
    if ($exists) {
        $links = @(Get-ChildItem -LiteralPath $absolute -Recurse -Force -Attributes ReparsePoint)
        if ($links.Count) { throw "Directory contains a link: $($links[0].FullName)" }
        $resolved = Get-NormalizedPath ((Resolve-Path -LiteralPath $absolute).ProviderPath)
        if ($resolved -ine $absolute) { throw "Resolved directory differs from expected path: $absolute" }
    }
    return $absolute
}

function Assert-NoInstallationProcesses([string[]]$Roots) {
    $processes = @(Get-CimInstance Win32_Process)
    foreach ($process in $processes) {
        $path = [string]$process.ExecutablePath
        if (!$path) { continue }
        foreach ($root in $Roots) {
            $normalizedRoot = Get-NormalizedPath $root
            if ($path.Equals($normalizedRoot, [StringComparison]::OrdinalIgnoreCase) -or
                $path.StartsWith($normalizedRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
                throw "Close this installation before switching: PID $($process.ProcessId), $path"
            }
        }
    }
    $productionProfile = Join-Path $env:LOCALAPPDATA 'com.sunriseworks.illutag\EBWebView'
    foreach ($process in $processes) {
        if ($process.Name -ieq 'msedgewebview2.exe' -and ([string]$process.CommandLine).Contains($productionProfile, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Production WebView is still running: PID $($process.ProcessId). Close illuTag normally."
        }
    }
}

function Assert-FileHash([string]$Path, [string]$Sha256, [long]$ExpectedBytes = -1) {
    if (!(Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Missing file: $Path" }
    if ($Sha256 -notmatch '^[a-fA-F0-9]{64}$') { throw "Invalid SHA-256 for $Path" }
    if ($ExpectedBytes -ge 0 -and (Get-Item -LiteralPath $Path).Length -ne $ExpectedBytes) { throw "File length differs: $Path" }
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ine $Sha256) { throw "SHA-256 differs: $Path" }
}

function Get-ManifestFilePath([string]$Root, [string]$Relative) {
    if (!$Relative -or [IO.Path]::IsPathRooted($Relative)) { throw 'Manifest paths must be relative' }
    $rootPath = Get-NormalizedPath $Root
    $path = [IO.Path]::GetFullPath((Join-Path $rootPath $Relative))
    if (!$path.StartsWith($rootPath + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Manifest path escapes its directory' }
    return $path
}

function Assert-StablePackage([string]$Root, [string]$ReferenceRoot) {
    $releasePath = Join-Path $ReferenceRoot 'release-manifest.json'
    $release = Get-Content -LiteralPath $releasePath -Raw | ConvertFrom-Json
    if ($release.version -ne '1.1.0' -or $release.schemaVersion -ne 2 -or $release.sourceCommit -notmatch '^[a-f0-9]{40}$') { throw 'Expected stable v1.1.0/schema 2 release manifest' }
    Assert-FileHash (Join-Path $Root 'release-manifest.json') ((Get-FileHash -LiteralPath $releasePath -Algorithm SHA256).Hash)
    foreach ($file in $release.files) {
        $reference = Get-ManifestFilePath $ReferenceRoot $file.path
        Assert-FileHash $reference $file.sha256 $file.bytes
        # Deployment intentionally preserves the live absolute portable dataDir.
        if ($file.path -ne 'illutag.portable.json') {
            Assert-FileHash (Get-ManifestFilePath $Root $file.path) $file.sha256 $file.bytes
        }
    }
    foreach ($required in @('illutag.exe','model-manifest.json','scripts/ort_runtime.py','scripts/wd_tagger_service.py','scripts/chinese_clip_image_service.py','scripts/chinese_clip_text_service.py')) {
        if (!(@($release.files.path) -contains $required)) { throw "Release manifest omits $required" }
    }
    $models = Get-Content -LiteralPath (Join-Path $Root 'model-manifest.json') -Raw | ConvertFrom-Json
    if ($models.appVersion -ne '1.1.0') { throw 'Model manifest version differs from stable release' }
    foreach ($file in $models.files) { Assert-FileHash (Get-ManifestFilePath $Root $file.path) $file.sha256 $file.bytes }
    foreach ($required in @('wd-swinv2-tagger-v3/model.onnx','wd-swinv2-tagger-v3/selected_tags.csv','wd-swinv2-tagger-v3/selected_tags_full_translation.csv','wd-swinv2-tagger-v3/tagger_config.json','wd-swinv2-tagger-v3/tag_groups.json','model/chinese-clip-vit-base-patch16/onnx/chinese_clip_text_encoder.onnx','model/chinese-clip-vit-base-patch16/onnx/chinese_clip_image_encoder.onnx','model/chinese-clip-vit-base-patch16/vocab.txt','model/chinese-clip-vit-base-patch16/preprocessor_config.json')) {
        if (!(@($models.files.path) -contains $required)) { throw "Model manifest omits $required" }
    }
    Assert-FileHash (Join-Path $Root 'runtime\python\python.exe') ((Get-FileHash -LiteralPath (Join-Path $ReferenceRoot 'runtime\python\python.exe') -Algorithm SHA256).Hash)
    $config = Get-Content -LiteralPath (Join-Path $Root 'wd-swinv2-tagger-v3\tagger_config.json') -Raw | ConvertFrom-Json
    if ($config.general_threshold -ne 0.2 -or $config.character_threshold -ne 0.2) { throw 'DTQ thresholds differ from the approved 0.2/0.2 configuration' }
    return $release.sourceCommit
}

function Assert-PortableDataRoot([string]$Root) {
    $config = Get-Content -LiteralPath (Join-Path $Root 'illutag.portable.json') -Raw | ConvertFrom-Json
    if ((Get-NormalizedPath ([string]$config.dataDir)) -ine $LiveRoot) { throw "Portable dataDir must preserve the production root: $Root" }
}

function Get-DatabaseInfo([string]$Root, [string]$PythonExe, [int]$ExpectedSchema) {
    $database = Join-Path $Root 'illutag.sqlite'
    if (!(Test-Path -LiteralPath $database -PathType Leaf)) { throw "Missing root library database: $database" }
    $wal = $database + '-wal'
    if ((Test-Path -LiteralPath $wal -PathType Leaf) -and (Get-Item -LiteralPath $wal).Length -gt 0) { throw "Uncheckpointed WAL found; close/checkpoint the library before deployment: $wal" }
    $script = @'
import json, sqlite3, sys
from pathlib import Path
p = Path(sys.argv[1]).resolve()
with sqlite3.connect(p.as_uri() + '?mode=ro&immutable=1', uri=True) as db:
    version = db.execute('PRAGMA user_version').fetchone()[0]
    images = db.execute('SELECT COUNT(*) FROM images').fetchone()[0]
print(json.dumps({'schemaVersion': version, 'imageCount': images, 'bytes': p.stat().st_size}))
'@
    $raw = & $PythonExe -B -c $script $database
    if ($LASTEXITCODE -ne 0) { throw "Read-only database check failed: $database" }
    $info = ($raw -join "`n") | ConvertFrom-Json
    if ($info.schemaVersion -ne $ExpectedSchema) { throw "Database schema differs from expected $ExpectedSchema : $database" }
    return $info
}

function Invoke-DeploymentSwitch([string]$Live, [string]$Next, [string]$Rollback, [string]$DisabledName) {
    $oldMoved = $false
    $newMoved = $false
    $oldExeDisabled = $false
    try {
        [IO.Directory]::Move($Live, $Rollback)
        $oldMoved = $true
        [IO.Directory]::Move($Next, $Live)
        $newMoved = $true
        [IO.File]::Move((Join-Path $Rollback 'illutag.exe'), (Join-Path $Rollback $DisabledName))
        $oldExeDisabled = $true
    } catch {
        $originalError = $_.Exception.Message
        $recoveryErrors = [Collections.Generic.List[string]]::new()
        if ($oldExeDisabled) {
            try { [IO.File]::Move((Join-Path $Rollback $DisabledName), (Join-Path $Rollback 'illutag.exe')) } catch { $recoveryErrors.Add($_.Exception.Message) }
        }
        if ($newMoved) {
            try { [IO.Directory]::Move($Live, $Next) } catch { $recoveryErrors.Add($_.Exception.Message) }
        }
        if ($oldMoved) {
            try { [IO.Directory]::Move($Rollback, $Live) } catch { $recoveryErrors.Add($_.Exception.Message) }
        }
        if ($recoveryErrors.Count) { throw "Deployment failed and recovery needs attention. Keep all directories and do not launch either EXE. Cause: $originalError; recovery: $($recoveryErrors -join ' | ')" }
        throw "Deployment failed; original production directory restored: $originalError"
    }
}

# Validation is the default. Directory mutations require the explicit -Execute flag.
$null = Assert-Path $LiveRoot 'D:\Ai' -MustExist
$null = Assert-Path $StageRoot 'D:\Ai\illuTag-refactor-work\deployment-stage' -MustExist
$null = Assert-Path $RollbackRoot 'D:\Ai' -MustBeAbsent
$null = Assert-Path $PackageRoot 'D:\Ai\illuTag-refactor-work\release-stable' -MustExist
Assert-NoInstallationProcesses @($LiveRoot,$StageRoot,$RollbackRoot,$PackageRoot)
if (Test-Path -LiteralPath (Join-Path $LiveRoot $DisabledExeName)) { throw 'Live directory already contains a disabled legacy EXE; resolve manually' }
Assert-FileHash (Join-Path $LiveRoot 'illutag.exe') $LegacyExeSha256
$commit = Assert-StablePackage $StageRoot $PackageRoot
Assert-PortableDataRoot $LiveRoot
Assert-PortableDataRoot $StageRoot
$python = Join-Path $StageRoot 'runtime\python\python.exe'
$oldDb = Get-DatabaseInfo $LiveRoot $python 1
$newDb = Get-DatabaseInfo $StageRoot $python 2
if ($newDb.imageCount -ne $oldDb.imageCount) { throw 'Stage image count differs from the production snapshot' }
$oldWindow = Join-Path $LiveRoot 'window-state.json'
if (Test-Path -LiteralPath $oldWindow -PathType Leaf) { Assert-FileHash (Join-Path $StageRoot 'window-state.json') ((Get-FileHash -LiteralPath $oldWindow -Algorithm SHA256).Hash) }
if (!$Execute) {
    Write-Output ([ordered]@{validated=$true;executed=$false;live=$LiveRoot;stage=$StageRoot;rollback=$RollbackRoot;sourceCommit=$commit;imageCount=$newDb.imageCount;schemaVersion=2} | ConvertTo-Json)
    Write-Output 'Preflight passed. Run with -Execute to perform the directory switch. The script never launches illuTag.'
    exit 0
}
# Recheck immediately before the two same-volume moves.
Assert-NoInstallationProcesses @($LiveRoot,$StageRoot,$RollbackRoot,$PackageRoot)
$null = Assert-Path $RollbackRoot 'D:\Ai' -MustBeAbsent
Invoke-DeploymentSwitch $LiveRoot $StageRoot $RollbackRoot $DisabledExeName
Write-Output ([ordered]@{validated=$true;executed=$true;live=$LiveRoot;rollback=$RollbackRoot;oldExeDisabled=(Join-Path $RollbackRoot $DisabledExeName);sourceCommit=$commit;imageCount=$newDb.imageCount;schemaVersion=2} | ConvertTo-Json)
Write-Output 'Deployment switched. Original program, database and resources are retained together. No EXE was launched.'
