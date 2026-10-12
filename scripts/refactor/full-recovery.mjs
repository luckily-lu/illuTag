import { launch, isolatedPath } from './native-session.mjs'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import fs from 'node:fs'
import path from 'node:path'
import assert from 'node:assert/strict'

const exec = promisify(execFile)
const [programArgument, workspaceArgument, existingPackage] = process.argv.slice(2)
const executable = isolatedPath(programArgument, 'program'), workspace = isolatedPath(workspaceArgument, 'workspace')
const origin = isolatedPath(path.join(workspace, 'validation/candidate'), 'source copy')
const target = isolatedPath(path.join(workspace, 'validation/full-restored'), 'restore target')
const destination = isolatedPath(path.join(workspace, 'validation/full-backups'), 'backup destination')
const reportPath = path.join(workspace, 'validation/full-recovery.json'), startedAt = new Date().toISOString()
const expected = { images: 809230, image_clip_embeddings: 809229, image_auto_tags: 41254917 }
let packageDir, manifest, restored, session
const checks = [], sessions = []
function validateManifest(value) {
  assert.equal(value.format, 'illutag-restore-package')
  assert.equal(value.packageVersion, 1)
  assert.equal(value.schemaVersion, 2)
  assert.equal(value.imageCount, expected.images)
  for (const [table, count] of Object.entries(expected)) assert.equal(value.counts[table], count, table)
  assert.equal(value.externalOriginalsIncluded, false)
  assert.ok(value.files.some(file => file.path === 'illutag.sqlite' && file.bytes > 29_000_000_000))
}
async function open(root) {
  session = await launch(executable, root, 9783)
  sessions.push({ pid: session.child.pid, root, logPath: session.logPath })
  // Integrity checks run in the backend. Keep the idle maintenance window from
  // being accidentally closed during a long full-library restore.
  await session.invoke('window_minimize_command')
}
async function readRestoredDatabase() {
  const python = isolatedPath(path.join(path.dirname(executable), 'runtime/python/python.exe'), 'bundled Python')
  const script = `import sqlite3,json,sys,pathlib
database=pathlib.Path(sys.argv[1]).resolve()
with sqlite3.connect(database.as_uri()+'?mode=ro',uri=True) as conn:
    names=[r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    counts={name:conn.execute('SELECT COUNT(*) FROM "'+name.replace('"','""')+'"').fetchone()[0] for name in names}
    result={'counts':counts,'schemaVersion':conn.execute('PRAGMA user_version').fetchone()[0],'visibleImages':conn.execute('SELECT COUNT(*) FROM images WHERE COALESCE(trashed,0)=0').fetchone()[0],'bytes':database.stat().st_size}
print(json.dumps(result))`
  const { stdout } = await exec(python, ['-B', '-X', 'utf8', '-c', script, path.join(target, 'data/illutag.sqlite')], { windowsHide: true, maxBuffer: 8 * 1024 * 1024 })
  return JSON.parse(stdout)
}
try {
  if (existingPackage) {
    packageDir = isolatedPath(existingPackage, 'existing recovery package')
    manifest = JSON.parse(fs.readFileSync(path.join(packageDir, 'manifest.json'), 'utf8'))
    validateManifest(manifest)
    console.log('Using existing package; restore itself performs complete hash, SQLite and reference validation', new Date().toISOString())
  } else {
    fs.mkdirSync(destination, { recursive: true })
    await open(origin)
    try {
      assert.equal((await session.invoke('get_bootstrap')).page.totalImageCount, 809214)
      console.log('Creating complete 809,230-image restore package', new Date().toISOString())
      packageDir = await session.invoke('create_restore_package', {
        destination,
        frontendSettings: await session.page.evaluate(() => ({ localStorage: Object.fromEntries(Object.keys(localStorage).filter(key => key.startsWith('illutag.')).map(key => [key, localStorage.getItem(key)])) })),
      })
      manifest = JSON.parse(fs.readFileSync(path.join(packageDir, 'manifest.json'), 'utf8'))
      validateManifest(manifest)
    } finally { await session.close(); session = undefined }
  }
  await open(target)
  try {
    console.log('Starting validated restore into independent target', new Date().toISOString())
    // No preliminary inspect: restore validates the complete package before any
    // switch, validates its staged copy and retains the previous database.
    const result = await session.invoke('restore_package', { packageDir, pathMappings: [] })
    assert.deepEqual(result.counts, manifest.counts)
    assert.equal(result.schemaVersion, manifest.schemaVersion)
    checks.push('restore command completed complete validation and switched independent database')
    await session.page.evaluate(values => {
      for (const [key, value] of Object.entries(values)) if (key.startsWith('illutag.') && typeof value === 'string') localStorage.setItem(key, value)
    }, manifest.frontendSettings?.localStorage ?? {})
    console.log('Restored independent full-library copy', new Date().toISOString())
  } finally { await session.close(); session = undefined }
  console.log('Reading target table counts through read-only SQLite', new Date().toISOString())
  restored = await readRestoredDatabase()
  assert.deepEqual(restored.counts, manifest.counts, 'Every restored table must match, including vectors and historical references')
  assert.equal(restored.schemaVersion, manifest.schemaVersion)
  assert.equal(restored.visibleImages, 809214)
  checks.push('read-only target SQL counts match every manifest table and schema version')
  await open(target)
  let lastPageCount, jobs, restoredSettings
  try {
    const bootstrap = await session.invoke('get_bootstrap')
    assert.equal(bootstrap.page.totalImageCount, restored.visibleImages)
    assert.equal(bootstrap.page.images.length, 5000)
    jobs = await session.invoke('list_jobs')
    assert.equal(jobs.filter(job => ['queued', 'running', 'paused', 'cancelling'].includes(job.state)).length, 0)
    const lastOffset = Math.floor((restored.visibleImages - 1) / 5000) * 5000
    const last = await session.invoke('query_gallery', { query: { scope: 'all', folderId: null, unclassifiedOnlyParentFolderId: null, tags: [], englishQuery: '', fileName: '', naturalLanguage: '', offset: lastOffset, limit: 5000, randomSeed: null } })
    lastPageCount = last.images.length
    assert.equal(lastPageCount, restored.visibleImages - lastOffset)
    assert.equal(lastPageCount, 4214)
    const settings = Object.fromEntries(Object.entries(manifest.frontendSettings?.localStorage ?? {}).filter(([key, value]) => key.startsWith('illutag.') && typeof value === 'string'))
    restoredSettings = await session.page.evaluate(keys => Object.fromEntries(keys.map(key => [key, localStorage.getItem(key)])), Object.keys(settings))
    assert.deepEqual(restoredSettings, settings)
    checks.push('restart loads first and final page, preserves preferences and starts no background jobs')
  } finally { await session.close(); session = undefined }
  fs.writeFileSync(reportPath, JSON.stringify({ passed: true, startedAt, completedAt: new Date().toISOString(), executable, packageDir, restoredRoot: target, counts: restored.counts, schemaVersion: restored.schemaVersion, databaseBytes: restored.bytes, visibleImages: restored.visibleImages, lastPageCount, jobs, restoredSettings, externalOriginalsIncluded: false, checks, sessions }, null, 2))
  console.log('Full recovery passed: target table counts, preferences and final page verified')
} catch (error) {
  if (session) await session.close().catch(() => {})
  fs.mkdirSync(path.dirname(reportPath), { recursive: true })
  fs.writeFileSync(reportPath, JSON.stringify({ passed: false, startedAt, failedAt: new Date().toISOString(), executable, packageDir, restoredRoot: target, error: error.stack ?? String(error), checks, sessions }, null, 2))
  throw error
}
