param(
  [Parameter(Mandatory=$true)][string]$CandidatePath,
  [Parameter(Mandatory=$true)][string]$ValidationReport,
  [Parameter(Mandatory=$true)][string]$BackupRoot,
  [switch]$Execute
)
$ErrorActionPreference='Stop'
$taskLive='D:\Ai\illuTag-danbooru-tag-query'
$taskFence='D:\Ai\illuTag-refactor-work'
$taskRepo=Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$taskCandidate=(Resolve-Path -LiteralPath $CandidatePath).Path
$taskReport=(Resolve-Path -LiteralPath $ValidationReport).Path
$taskBackup=[IO.Path]::GetFullPath($BackupRoot)
foreach($taskPath in @($taskCandidate,$taskReport,$taskBackup)){
  if(!$taskPath.StartsWith($taskFence+'\',[StringComparison]::OrdinalIgnoreCase)){
    throw "Release paths must stay inside $taskFence"
  }
}
if(Test-Path -LiteralPath $taskBackup){throw 'Backup destination must be new'}
$taskValidation=Get-Content -LiteralPath $taskReport -Raw | ConvertFrom-Json
$taskHash=(Get-FileHash -LiteralPath $taskCandidate -Algorithm SHA256).Hash.ToLowerInvariant()
if(!$taskValidation.passed -or !$taskValidation.syntheticOnly -or $taskValidation.exeSha256 -ne $taskHash){
  throw 'Candidate does not match a passed synthetic native acceptance report'
}
$taskVersion=(Get-Content -LiteralPath (Join-Path $taskRepo 'package.json') -Raw | ConvertFrom-Json).version
if($taskVersion -ne '1.6.0'){throw 'This update is restricted to version 1.6.0'}
$taskPortable=Get-Content -LiteralPath (Join-Path $taskLive 'illutag.portable.json') -Raw | ConvertFrom-Json
$taskData=[IO.Path]::GetFullPath(([string]$taskPortable.dataDir).Replace('\\?\',''))
if($taskData -ine $taskLive){throw 'Unexpected production data root'}
$taskOldManifest=Get-Content -LiteralPath (Join-Path $taskLive 'release-manifest.json') -Raw | ConvertFrom-Json
$taskOldExe=Join-Path $taskLive 'illutag.exe'
$taskOldHash=(Get-FileHash -LiteralPath $taskOldExe -Algorithm SHA256).Hash.ToLowerInvariant()
if($taskOldManifest.version -ne '1.5.2' -or $taskOldManifest.schemaVersion -ne 3 -or
   $taskOldHash -ne ($taskOldManifest.files | Where-Object path -eq 'illutag.exe').sha256){
  throw 'Installed release is not the expected unchanged 1.5.2/schema 3'
}
if(!$Execute){
  @{validated=$true;version=$taskVersion;candidateSha256=$taskHash;backup=$taskBackup}|ConvertTo-Json -Compress
  exit 0
}
$taskRunning=@(Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -ieq $taskOldExe})
if($taskRunning.Count){throw 'Close the installed illuTag normally before applying the update'}
$taskLease=[IO.File]::Open((Join-Path $taskLive 'illutag.session.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
$taskStage=Join-Path $taskLive ('illutag.exe.update-'+$taskVersion)
$taskBackupNames=@('release-manifest.json','model-manifest.json','deployment-manifest.json','README-portable.txt')
$taskMovedOld=$false
try{
  if(Test-Path -LiteralPath $taskStage){throw 'Unexpected preexisting update staging file'}
  New-Item -ItemType Directory -Path $taskBackup | Out-Null
  foreach($taskName in $taskBackupNames){
    $taskFile=Join-Path $taskLive $taskName
    if(Test-Path -LiteralPath $taskFile){Copy-Item -LiteralPath $taskFile -Destination (Join-Path $taskBackup $taskName)}
  }
  foreach($taskJournalName in @('duplicate-management','recycle-management')){
  foreach($taskSuffix in @('','-wal','-shm')){
    $taskJournal=Join-Path $taskLive ('operations\'+$taskJournalName+'.sqlite'+$taskSuffix)
    if(Test-Path -LiteralPath $taskJournal){Copy-Item -LiteralPath $taskJournal -Destination (Join-Path $taskBackup ($taskJournalName+'.sqlite'+$taskSuffix))}
  }
  }
  $taskDatabase=Get-Item -LiteralPath (Join-Path $taskLive 'illutag.sqlite')
  $taskDatabaseIdentity=@{path=$taskDatabase.FullName;bytes=$taskDatabase.Length;lastWriteUtc=$taskDatabase.LastWriteTimeUtc.ToString('o')}
  Copy-Item -LiteralPath $taskCandidate -Destination $taskStage
  if((Get-FileHash -LiteralPath $taskStage -Algorithm SHA256).Hash.ToLowerInvariant() -ne $taskHash){throw 'Staged EXE differs'}
  [IO.File]::Move($taskOldExe,(Join-Path $taskBackup 'illutag.exe.backup'))
  $taskMovedOld=$true
  [IO.File]::Move($taskStage,$taskOldExe)
  $taskModels=Get-Content -LiteralPath (Join-Path $taskLive 'model-manifest.json') -Raw | ConvertFrom-Json
  $taskModels.appVersion=$taskVersion
  $taskModels | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $taskLive 'model-manifest.json') -Encoding utf8NoBOM
  $taskOldManifest.version=$taskVersion
  $taskOldManifest | Add-Member -NotePropertyName workingTreeModified -NotePropertyValue $true -Force
  foreach($taskFile in $taskOldManifest.files){
    if($taskFile.path -in @('illutag.exe','model-manifest.json')){
      $taskSource=Get-Item -LiteralPath (Join-Path $taskLive $taskFile.path)
      $taskFile.bytes=$taskSource.Length
      $taskFile.sha256=(Get-FileHash -LiteralPath $taskSource.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
  }
  $taskOldManifest | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $taskLive 'release-manifest.json') -Encoding utf8NoBOM
  $taskDeployment=@{version=$taskVersion;schemaVersion=3;deployedAt=[DateTimeOffset]::UtcNow.ToString('o');exeSha256=$taskHash;sourceCommit=(& git -C $taskRepo rev-parse HEAD);workingTreeModified=$true;validationReport=$taskReport;backupRoot=$taskBackup;database=$taskDatabaseIdentity;originalFilesPreserved=$true;bulkExecution='Starts only when the user clicks a global action';modelRuntimeReused=$true}
  $taskDeployment | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $taskLive 'deployment-manifest.json') -Encoding utf8NoBOM
  @"
illuTag $taskVersion portable
Start illutag.exe. Existing absolute data directory and runtime/model resources are preserved.
Duplicate search has global exact/random and near/highest-resolution actions. Both move records to the application recycle bin and support bulk undo; original files remain in place.
Recycle bin has dedicated restore and cleanup actions. Source-file cleanup only starts after explicit confirmation.
Rollback program: $taskBackup\illutag.exe.backup
Native acceptance: $taskReport
"@ | Set-Content -LiteralPath (Join-Path $taskLive 'README-portable.txt') -Encoding utf8NoBOM
  $taskAfter=Get-Item -LiteralPath (Join-Path $taskLive 'illutag.sqlite')
  if($taskAfter.Length -ne $taskDatabase.Length -or $taskAfter.LastWriteTimeUtc -ne $taskDatabase.LastWriteTimeUtc){throw 'Unexpected database modification during program-only replacement'}
  @{deployed=$true;version=$taskVersion;exeSha256=$taskHash;backupRoot=$taskBackup}|ConvertTo-Json -Compress
}catch{
  if($taskMovedOld){
    if(Test-Path -LiteralPath $taskOldExe){[IO.File]::Move($taskOldExe,(Join-Path $taskBackup ('illutag-'+$taskVersion+'-failed.exe.backup')))}
    [IO.File]::Move((Join-Path $taskBackup 'illutag.exe.backup'),$taskOldExe)
    foreach($taskName in $taskBackupNames){
      $taskSaved=Join-Path $taskBackup $taskName
      if(Test-Path -LiteralPath $taskSaved){Copy-Item -LiteralPath $taskSaved -Destination (Join-Path $taskLive $taskName) -Force}
    }
  }
  throw
}finally{$taskLease.Dispose()}
