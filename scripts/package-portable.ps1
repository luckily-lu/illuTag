param(
  [Parameter(Mandatory=$true)][string]$ResourceRoot,
  [string]$OutputRoot,
  [string]$ReleaseExe,
  [switch]$Build,
  [switch]$SkipZip
)
$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$metadata = Get-Content -LiteralPath (Join-Path $repoRoot 'package.json') -Raw | ConvertFrom-Json
$version = $metadata.version
if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?$') { throw 'Invalid package version' }
$ResourceRoot = (Resolve-Path -LiteralPath $ResourceRoot).Path
if (!$OutputRoot) { $OutputRoot = Join-Path $repoRoot 'release\portable' }
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
if (!$ReleaseExe) {
  $targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repoRoot 'src-tauri\target' }
  $ReleaseExe = Join-Path $targetRoot 'release\illutag.exe'
}
if ($Build) {
  Push-Location $repoRoot
  try {
    & npm run tauri build -- --no-bundle
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
  } finally { Pop-Location }
}
$ReleaseExe = (Resolve-Path -LiteralPath $ReleaseExe).Path
$required = @(
  'runtime\python\python.exe',
  'wd-swinv2-tagger-v3\model.onnx',
  'wd-swinv2-tagger-v3\selected_tags.csv',
  'wd-swinv2-tagger-v3\selected_tags_full_translation.csv',
  'wd-swinv2-tagger-v3\tagger_config.json',
  'wd-swinv2-tagger-v3\tag_groups.json',
  'model\chinese-clip-vit-base-patch16\onnx\chinese_clip_text_encoder.onnx',
  'model\chinese-clip-vit-base-patch16\onnx\chinese_clip_image_encoder.onnx',
  'model\chinese-clip-vit-base-patch16\vocab.txt',
  'model\chinese-clip-vit-base-patch16\preprocessor_config.json'
)
foreach ($relative in $required) {
  if (!(Test-Path -LiteralPath (Join-Path $ResourceRoot $relative) -PathType Leaf)) { throw "Missing resource: $relative" }
}
$portableAppDir = Join-Path $OutputRoot "illuTag-$version"
$zipPath = Join-Path $OutputRoot "illuTag-portable-$version.zip"
# Never delete an existing release directory or overwrite another artifact.
if ((Test-Path -LiteralPath $portableAppDir) -or (Test-Path -LiteralPath $zipPath)) { throw 'Output already exists; choose a new output directory' }
New-Item -ItemType Directory -Path $portableAppDir -Force | Out-Null
Copy-Item -LiteralPath $ReleaseExe -Destination (Join-Path $portableAppDir 'illutag.exe')
Copy-Item -LiteralPath (Join-Path $repoRoot 'node_modules\@lucide\vue\LICENSE') -Destination (Join-Path $portableAppDir 'LICENSE-Lucide.txt')
Copy-Item -LiteralPath (Join-Path $ResourceRoot 'runtime') -Destination (Join-Path $portableAppDir 'runtime') -Recurse
foreach ($relative in $required | Where-Object { !$_.StartsWith('runtime\') }) {
  $destination = Join-Path $portableAppDir $relative
  New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
  Copy-Item -LiteralPath (Join-Path $ResourceRoot $relative) -Destination $destination
}
$scriptNames = @('wd_tagger_service.py','wd_tagger_test.py','chinese_clip_text_service.py','chinese_clip_image_service.py','ort_runtime.py','storage_optimize.py','storage_cutover.py')
New-Item -ItemType Directory -Path (Join-Path $portableAppDir 'scripts') | Out-Null
foreach ($name in $scriptNames) { Copy-Item -LiteralPath (Join-Path $repoRoot "src-tauri\scripts\$name") -Destination (Join-Path $portableAppDir "scripts\$name") }
foreach ($name in @('compact_schema.sql','compact_indexes.sql')) { Copy-Item -LiteralPath (Join-Path $repoRoot "src-tauri\src\library\$name") -Destination (Join-Path $portableAppDir "scripts\$name") }
Set-Content -LiteralPath (Join-Path $portableAppDir 'illutag.portable.json') -Value '{"dataDir":"data"}' -Encoding utf8NoBOM
$records = foreach ($relative in $required | Where-Object { !$_.StartsWith('runtime\') }) {
  $file = Get-Item -LiteralPath (Join-Path $portableAppDir $relative)
  @{path=$relative.Replace('\','/');bytes=$file.Length;sha256=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
}
@{appVersion=$version;chineseClip='onnx_v1';wdTagger='wd-swinv2-tagger-v3';files=@($records)} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $portableAppDir 'model-manifest.json') -Encoding utf8NoBOM
$releaseRecords = foreach ($relative in @('illutag.exe','illutag.portable.json','model-manifest.json','LICENSE-Lucide.txt') + (($scriptNames + @('compact_schema.sql','compact_indexes.sql')) | ForEach-Object {"scripts\$_"})) {
  $file = Get-Item -LiteralPath (Join-Path $portableAppDir $relative)
  @{path=$relative.Replace('\','/');bytes=$file.Length;sha256=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
}
@{version=$version;schemaVersion=3;sourceCommit=(& git -C $repoRoot rev-parse HEAD);files=@($releaseRecords)} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $portableAppDir 'release-manifest.json') -Encoding utf8NoBOM
@"
illuTag $version portable
Keep all folders together. Start illutag.exe; data is stored in the sibling data directory.
This package contains no user database or source images. Restore a complete backup into an independent directory for evaluation.
Complete backups exclude external source originals. Models are supplied in this program package.
Schema 3 databases must never be opened with the old executable. Roll back the executable, scripts, configuration and matching original database as one saved set.
"@ | Set-Content -LiteralPath (Join-Path $portableAppDir 'README-portable.txt') -Encoding utf8NoBOM
if (!$SkipZip) {
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  [IO.Compression.ZipFile]::CreateFromDirectory($portableAppDir,$zipPath,[IO.Compression.CompressionLevel]::Optimal,$true)
}
Write-Output "Portable program: $portableAppDir"
if (!$SkipZip) { Write-Output "Archive: $zipPath" }
