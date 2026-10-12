#Requires -Version 7.4
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Load only function declarations; never run either script's production preflight
# or directory-switch entry point. Fixtures remain in the isolated validation tree.
$checks = [Collections.Generic.List[string]]::new()
foreach ($name in @('apply-deployment.ps1', 'rollback-deployment.ps1')) {
    $tokens = $null
    $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $name), [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw "PowerShell syntax error: $name" }
    foreach ($definition in $ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst]}, $true)) {
        . ([scriptblock]::Create($definition.Extent.Text))
    }
}
$checks.Add('both deployment scripts parse without errors')
$fixtureRoot = 'D:\Ai\illuTag-refactor-work\validation\deployment-script-qa-' + [guid]::NewGuid().ToString('N')
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
if ([IO.Path]::GetFullPath($fixtureRoot) -notlike 'D:\Ai\illuTag-refactor-work\validation\deployment-script-qa-*') { throw 'Fixture path escaped isolation' }

function Assert-Condition([bool]$Value, [string]$Message) { if (!$Value) { throw $Message } }
function Assert-Fails([scriptblock]$Action, [string]$ExpectedText) {
    try { & $Action } catch {
        if (!$_.Exception.Message.Contains($ExpectedText)) { throw "Unexpected error: $($_.Exception.Message)" }
        return
    }
    throw "Expected failure containing: $ExpectedText"
}
function New-Case([string]$Name, [switch]$WithNext) {
    $root = Join-Path $fixtureRoot $Name
    $live = Join-Path $root 'live'
    $next = Join-Path $root 'next'
    $rollback = Join-Path $root 'rollback'
    $after = Join-Path $root 'after'
    New-Item -ItemType Directory -Path $live -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $live 'illutag.exe'), 'original-exe')
    [IO.File]::WriteAllText((Join-Path $live 'original-data.txt'), 'original-library')
    if ($WithNext) {
        New-Item -ItemType Directory -Path $next | Out-Null
        [IO.File]::WriteAllText((Join-Path $next 'illutag.exe'), 'new-exe')
        [IO.File]::WriteAllText((Join-Path $next 'new-data.txt'), 'new-library')
    }
    return @{root=$root;live=$live;next=$next;rollback=$rollback;after=$after}
}

$disabled = 'illutag.exe.rollback-disabled'
$afterDisabled = 'illutag.exe.after-rollback-disabled'
$case = New-Case 'success' -WithNext
$null = Assert-Path $case.live $case.root -MustExist
Assert-Condition ((Get-NormalizedPath '\\?\D:\Ai\illuTag-danbooru-tag-query') -eq 'D:\Ai\illuTag-danbooru-tag-query') 'Extended production portable path failed normalization'
Assert-Fails { Assert-Path $case.live 'D:\Ai' -MustExist } 'Unexpected parent'
Assert-Fails { Assert-Path $case.live $case.root -MustBeAbsent } 'overwrite'
Assert-Fails { Get-NormalizedPath 'D:relative' } 'Absolute path'
$checks.Add('absolute same-volume paths, parent bounds, existing-output refusal and extended portable path')
Assert-Fails { Get-ManifestFilePath $case.live '..\..\outside.json' } 'escapes'
Assert-Fails { Get-ManifestFilePath $case.live 'D:\outside.json' } 'relative'
$checks.Add('manifest traversal and rooted filenames rejected')

Invoke-DeploymentSwitch $case.live $case.next $case.rollback $disabled
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.live 'new-data.txt')) 'New directory did not become live'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.rollback 'original-data.txt')) 'Original data was not preserved'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.rollback $disabled)) 'Original EXE was not disabled'
Assert-Condition (!(Test-Path -LiteralPath (Join-Path $case.rollback 'illutag.exe'))) 'Original EXE still launchable in rollback archive'
$checks.Add('successful switch preserves old directory and disables its EXE without altering bytes')
Invoke-RollbackSwitch $case.live $case.rollback $case.after $disabled
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.live 'original-data.txt')) 'Original directory was not restored'
Assert-Condition ([IO.File]::ReadAllText((Join-Path $case.live 'illutag.exe')) -eq 'original-exe') 'Original EXE bytes changed'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.after 'new-data.txt')) 'New library was not retained after rollback'
Assert-Condition (!(Test-Path -LiteralPath (Join-Path $case.after 'illutag.exe'))) 'New EXE is still launchable in after archive'
Assert-Condition ([IO.File]::ReadAllText((Join-Path $case.after $afterDisabled)) -eq 'new-exe') 'Archived new EXE bytes changed'
$checks.Add('successful rollback restores original EXE, retains all new data and disables archived new EXE')

$case = New-Case 'second-rename-failure'
Assert-Fails { Invoke-DeploymentSwitch $case.live $case.next $case.rollback $disabled } 'original production directory restored'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.live 'original-data.txt')) 'Second-rename failure did not restore original directory'
Assert-Condition (!(Test-Path -LiteralPath $case.rollback)) 'Unexpected rollback directory remained after recovery'
$checks.Add('failure of second deployment rename restores original live directory')

$case = New-Case 'disable-exe-failure' -WithNext
[IO.File]::WriteAllText((Join-Path $case.live $disabled), 'occupied-disabled-name')
Assert-Fails { Invoke-DeploymentSwitch $case.live $case.next $case.rollback $disabled } 'original production directory restored'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.live 'original-data.txt')) 'EXE disable failure did not restore original directory'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.next 'new-data.txt')) 'EXE disable failure did not return prepared directory'
$checks.Add('failure to disable old EXE reverses both directory moves')

$case = New-Case 'rollback-rename-failure' -WithNext
Invoke-DeploymentSwitch $case.live $case.next $case.rollback $disabled
Assert-Fails { Invoke-RollbackSwitch $case.live (Join-Path $case.root 'missing-rollback') $case.after $disabled } 'new production directory restored'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.live 'new-data.txt')) 'Rollback rename failure did not preserve new live directory'
Assert-Condition ([IO.File]::ReadAllText((Join-Path $case.live 'illutag.exe')) -eq 'new-exe') 'New EXE name was not restored after old-directory move failure'
Assert-Condition (!(Test-Path -LiteralPath (Join-Path $case.live $afterDisabled))) 'New EXE remained disabled after failed rollback recovery'
$checks.Add('failure of old-directory rollback move restores new live data and re-enables its EXE')

$case = New-Case 'after-exe-disable-failure' -WithNext
Invoke-DeploymentSwitch $case.live $case.next $case.rollback $disabled
[IO.File]::WriteAllText((Join-Path $case.live $afterDisabled), 'occupied-after-disabled-name')
Assert-Fails { Invoke-RollbackSwitch $case.live $case.rollback $case.after $disabled } 'new production directory restored'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.live 'new-data.txt')) 'Failure disabling after EXE lost new live data'
Assert-Condition ([IO.File]::ReadAllText((Join-Path $case.live 'illutag.exe')) -eq 'new-exe') 'Failure disabling after EXE lost current EXE'
Assert-Condition ([IO.File]::ReadAllText((Join-Path $case.live $afterDisabled)) -eq 'occupied-after-disabled-name') 'Existing disabled-name bytes were overwritten'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.rollback $disabled)) 'Original rollback EXE changed before after sealing succeeded'
Assert-Condition (!(Test-Path -LiteralPath $case.after)) 'Failed after sealing left directory stranded'
$checks.Add('failure to disable archived new EXE restores its live directory before moving the old archive')

$case = New-Case 'rollback-exe-failure' -WithNext
Invoke-DeploymentSwitch $case.live $case.next $case.rollback $disabled
[IO.File]::WriteAllText((Join-Path $case.rollback 'illutag.exe'), 'occupied-exe-name')
Assert-Fails { Invoke-RollbackSwitch $case.live $case.rollback $case.after $disabled } 'new production directory restored'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.live 'new-data.txt')) 'Rollback EXE enable failure lost new live data'
Assert-Condition (Test-Path -LiteralPath (Join-Path $case.rollback $disabled)) 'Rollback archive disabled EXE was not retained'
Assert-Condition ([IO.File]::ReadAllText((Join-Path $case.live 'illutag.exe')) -eq 'new-exe') 'New EXE was not re-enabled after legacy EXE failure'
Assert-Condition (!(Test-Path -LiteralPath (Join-Path $case.live $afterDisabled))) 'New EXE remains disabled after legacy EXE failure recovery'
$checks.Add('failure to re-enable original EXE reverses rollback moves and re-enables the current EXE')

$python = 'D:\Ai\illuTag-refactor-work\release-final\illuTag-1.1.0-rc.1\runtime\python\python.exe'
$dbRoot = Join-Path $fixtureRoot 'database'
New-Item -ItemType Directory -Path $dbRoot | Out-Null
$database = Join-Path $dbRoot 'illutag.sqlite'
$code = @'
import sqlite3, sys
with sqlite3.connect(sys.argv[1]) as db:
    db.executescript('CREATE TABLE images(id TEXT PRIMARY KEY); INSERT INTO images VALUES ("one"),("two"); PRAGMA user_version=2;')
'@
& $python -B -c $code $database
if ($LASTEXITCODE -ne 0) { throw 'Database fixture creation failed' }
$beforeHash = (Get-FileHash -LiteralPath $database -Algorithm SHA256).Hash
$info = Get-DatabaseInfo $dbRoot $python 2
Assert-Condition ($info.imageCount -eq 2) 'Database image count differs'
Assert-Condition ((Get-FileHash -LiteralPath $database -Algorithm SHA256).Hash -eq $beforeHash) 'Read-only inspection altered fixture database'
Assert-Condition (!(Test-Path -LiteralPath ($database + '-wal')) -and !(Test-Path -LiteralPath ($database + '-shm'))) 'Read-only inspection created WAL or SHM'
Assert-Fails { Get-DatabaseInfo $dbRoot $python 1 } 'schema differs'
[IO.File]::WriteAllText(($database + '-wal'), 'pending-wal')
Assert-Fails { Get-DatabaseInfo $dbRoot $python 2 } 'Uncheckpointed WAL'
$checks.Add('immutable schema/count inspection leaves database unchanged and refuses pending WAL')

$result = [ordered]@{passed=$true;checks=$checks;fixtureRoot=$fixtureRoot;productionTouched=$false;exeLaunched=$false;directoriesDeleted=$false}
$result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $fixtureRoot 'deployment-script-qa.json') -Encoding utf8NoBOM
$result | ConvertTo-Json -Depth 4
