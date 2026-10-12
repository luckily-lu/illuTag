param(
  [string]$Source
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (!$Source) { $Source = Join-Path $repoRoot 'src-tauri\icons\app-icon-master.png' }
$Source = (Resolve-Path -LiteralPath $Source).Path
$iconRoot = Join-Path $repoRoot 'src-tauri\icons'
Push-Location $repoRoot
try {
  & npm.cmd run tauri -- icon $Source --output $iconRoot
  if ($LASTEXITCODE -ne 0) { throw 'Icon generation failed' }
} finally { Pop-Location }

# Tauri embeds the first ICO entry as the Windows window icon. Put the largest
# frame first so the runtime does not enlarge a 32px frame on high-DPI displays.
# Reorder directory entries only: preserve every encoded image and its offset.
$icoPath = Join-Path $iconRoot 'icon.ico'
$bytes = [IO.File]::ReadAllBytes($icoPath)
if ($bytes.Length -lt 6 -or [BitConverter]::ToUInt16($bytes, 2) -ne 1) {
  throw 'Invalid ICO header'
}
$count = [BitConverter]::ToUInt16($bytes, 4)
if ($count -lt 1 -or $bytes.Length -lt (6 + 16 * $count)) {
  throw 'Invalid ICO directory'
}
$entries = for ($i = 0; $i -lt $count; $i++) {
  $entry = New-Object byte[] 16
  [Array]::Copy($bytes, 6 + 16 * $i, $entry, 0, 16)
  $width = if ($entry[0] -eq 0) { 256 } else { [int]$entry[0] }
  [pscustomobject]@{ Width = $width; Bytes = $entry }
}
$ordered = @($entries | Sort-Object Width -Descending)
for ($i = 0; $i -lt $count; $i++) {
  [Array]::Copy($ordered[$i].Bytes, 0, $bytes, 6 + 16 * $i, 16)
}
[IO.File]::WriteAllBytes($icoPath, $bytes)
Write-Output ('Windows ICO frames: ' + (($ordered | ForEach-Object { $_.Width }) -join ', '))
