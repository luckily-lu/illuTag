// Tiny, fresh, synthetic Windows recycle sample; never accepts production data.
import { launch, isolatedPath, sleep } from './native-session.mjs'
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'

const [exeArg, rootArg, resourceArg, mode = 'controls'] = process.argv.slice(2)
const exe = isolatedPath(exeArg, 'candidate'), root = isolatedPath(rootArg, 'fresh QA root')
assert.ok(!fs.existsSync(root), 'Fresh synthetic root required')
assert.ok(['baseline', 'controls'].includes(mode))
assert.ok(resourceArg && path.isAbsolute(resourceArg))
const python = path.join(fs.realpathSync(resourceArg), 'runtime/python/python.exe')
const report = { passed: false, syntheticOnly: true, mode, checks: [], exeSha256: createHash('sha256').update(fs.readFileSync(exe)).digest('hex') }
let session, failure
function py(code) {
  return JSON.parse(execFileSync(python, ['-B', '-c', code, root], {
    windowsHide: true, env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' }, timeout: 10000,
  }).toString())
}
async function until(read, accept, ms = 30000) {
  const end = performance.now() + ms
  while (performance.now() < end) {
    const value = await read()
    if (accept(value)) return value
    await sleep(15)
  }
  throw Error('Synthetic recycle check timed out')
}
const query = fileName => ({ scope: 'trash', folderId: null, unclassifiedOnlyParentFolderId: null, tags: [], englishQuery: '', fileName, naturalLanguage: '', offset: 0, limit: 1, randomSeed: 0 })
const job = id => session.invoke('list_jobs').then(rows => rows.find(r => r.id === id))
async function finish(id) {
  const row = await until(() => job(id), r => ['completed', 'partial', 'cancelled', 'failed', 'interrupted'].includes(r?.state))
  assert.ok(['completed', 'cancelled'].includes(row.state), JSON.stringify(row))
  return row
}
async function apply(selectionId) {
  return session.invoke('apply_recycle_action', { request: { selectionId, action: 'systemTrash', confirmed: true } })
}
async function selection(fileName) {
  return session.invoke('prepare_recycle_selection', { request: { query: query(fileName), imageIds: [], selectionId: null, excludedIds: [] } })
}
const seed = String.raw`
from PIL import Image
import pathlib,sqlite3,sys,json
p=pathlib.Path(sys.argv[1]).resolve(); assert 'illutag-refactor-work' in [s.lower() for s in p.parts]
assets=p/'assets'; c=sqlite3.connect(p/'data'/'illutag.sqlite',timeout=10)
c.execute('pragma foreign_keys=on')
c.execute('insert into folders(id,path,added_at) values(1,?,0)',(str(assets),))
im=Image.new('RGB',(16,16),(65,134,177))
for prefix,count in [('bench',128),('control',512)]:
 for n in range(count):
  f=assets/f'{prefix}-{n:04}.png'; im.save(f); s=f.stat()
  c.execute("insert into images(id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id,trashed) values(?,?,?,'png',16,16,?,?,0,1,1)",(str(f),str(f),f.name,s.st_size,s.st_mtime_ns//1000000))
c.commit(); c.close()
print(json.dumps({'count':640,'bytes':sum(f.stat().st_size for f in assets.iterdir())}))
`
const inspect = String.raw`
import pathlib,sqlite3,sys,json
p=pathlib.Path(sys.argv[1]).resolve(); assert 'illutag-refactor-work' in [s.lower() for s in p.parts]
c=sqlite3.connect((p/'data'/'illutag.sqlite').as_uri()+'?mode=ro',uri=True)
print(json.dumps({'rows':c.execute('select count(*) from images').fetchone()[0],'files':len(list((p/'assets').iterdir())),'schema':c.execute('pragma user_version').fetchone()[0]})); c.close()
`
try {
  session = await launch(exe, root, 9798, { resourceRoot: resourceArg })
  await session.page.waitForSelector('.app')
  assert.equal((await session.invoke('get_bootstrap')).page.totalImageCount, 0)
  assert.equal((await session.invoke('list_jobs')).length, 0)
  report.fixture = py(seed)
  const sample = await selection('bench-')
  assert.equal(sample.count, 128)
  const began = performance.now(), first = await apply(sample.selectionId)
  const done = await finish(first.id)
  assert.equal(done.state, 'completed')
  assert.equal(done.processed, 128)
  report.recycle128Ms = Math.round(performance.now() - began)
  assert.deepEqual(py(inspect), { rows: 512, files: 512, schema: 3 })
  report.checks.push('128 real Windows recycle moves match database removals')
  if (mode === 'controls') {
    const sample = await selection('control-')
    const task = await apply(sample.selectionId)
    await until(() => job(task.id), r => r?.state === 'running')
    const began = performance.now()
    await session.invoke('control_job', { id: task.id, action: 'pause' })
    report.pauseRequestMs = Math.round(performance.now() - began)
    assert.equal((await job(task.id)).state, 'paused')
    // Allow only the shell batch already in flight to settle.
    await sleep(400)
    const paused = await session.invoke('recycle_operation_status', { operationId: sample.selectionId })
    await sleep(250)
    const still = await session.invoke('recycle_operation_status', { operationId: sample.selectionId })
    assert.equal(still.processed, paused.processed)
    assert.ok(still.processed < 512)
    await session.invoke('control_job', { id: task.id, action: 'resume' })
    await until(() => session.invoke('recycle_operation_status', { operationId: sample.selectionId }), s => s.processed > paused.processed)
    const stop = performance.now()
    await session.invoke('control_job', { id: task.id, action: 'cancel' })
    const cancelled = await finish(task.id)
    assert.equal(cancelled.state, 'cancelled')
    report.stopMs = Math.round(performance.now() - stop)
    const partial = await session.invoke('recycle_operation_status', { operationId: sample.selectionId })
    assert.ok(partial.succeeded > 0 && partial.succeeded < 512)
    assert.deepEqual(py(inspect), { rows: 512-partial.succeeded, files: 512-partial.succeeded, schema: 3 })
    const resumed = await apply(sample.selectionId)
    assert.equal(resumed.request.recycleOperationId, sample.selectionId)
    assert.equal((await finish(resumed.id)).state, 'completed')
    assert.deepEqual(py(inspect), { rows: 0, files: 0, schema: 3 })
    report.checks.push('Pause stays stable; resume advances; stop preserves completed moves; same batch continues without duplicate moves')
  }
  report.passed = true
} catch (error) {
  failure = error
  report.error = error.message
} finally {
  if (session) {
    await session.close()
    const stopped = session.getDiagnostics()
    report.session = stopped
    if (stopped.killed || stopped.exitCode !== 0) { report.passed = false; failure ||= Error('QA did not close normally') }
  }
  fs.writeFileSync(path.join(root, 'logs/recycle-performance-qa.json'), JSON.stringify(report, null, 2))
  console.log(JSON.stringify(report))
}
if (failure) throw failure
