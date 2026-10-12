import { connectDebugging, sleep } from './native-session.mjs'
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import net from 'node:net'
import { createHash } from 'node:crypto'
import assert from 'node:assert/strict'

// This is a deployment check, not a preview launcher. It can only open the
// explicitly authorized installation and performs read-only application calls.
const root = 'D:\\Ai\\illuTag-danbooru-tag-query'
const executable = path.join(root, 'illutag.exe')
const [reportPath] = process.argv.slice(2)
assert.ok(reportPath && path.isAbsolute(reportPath), 'An absolute report destination is required')
const manifest = JSON.parse(fs.readFileSync(path.join(root, 'release-manifest.json'), 'utf8'))
assert.equal(manifest.version, '1.1.0')
assert.equal(manifest.schemaVersion, 2)
const digest = createHash('sha256').update(fs.readFileSync(executable)).digest('hex')
assert.equal(digest, manifest.files.find(file => file.path === 'illutag.exe').sha256)
const normalize = value => path.resolve(value.replace(/^\\\\\?\\/, '')).toLowerCase()
const config = JSON.parse(fs.readFileSync(path.join(root, 'illutag.portable.json'), 'utf8'))
assert.equal(normalize(config.dataDir), normalize(root), 'Production must use its existing root database')
const port = 9789, probe = net.createServer()
await new Promise((resolve, reject) => { probe.once('error', reject); probe.listen(port, '127.0.0.1', resolve) })
await new Promise(resolve => probe.close(resolve))
const env = { ...process.env }
for (const key of Object.keys(env)) if (key.startsWith('ILLUTAG_') || key.startsWith('WEBVIEW2_')) delete env[key]
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${port} --remote-allow-origins=*`
const child = spawn(executable, [], { cwd: root, windowsHide: true, stdio: 'ignore', env })
let browser, transport, page, outcome, failure, spawnError
child.once('error', error => { spawnError = error })
const checks = [], startedAt = new Date().toISOString()
const invoke = (command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args })
async function bounded(promise, milliseconds, label) {
  let timer
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(Error(`${label} timed out`)), milliseconds) })]) }
  finally { clearTimeout(timer) }
}
const alive = () => child.exitCode === null && child.signalCode === null && !spawnError
try {
  for (let attempt = 0; attempt < 160; attempt++) {
    if (spawnError) throw spawnError
    assert.equal(child.exitCode, null, 'Production application exited before connecting')
    try { ({ browser, transport } = await connectDebugging(port)); break }
    catch { await sleep(100) }
  }
  assert.ok(browser, 'Production WebView2 endpoint unavailable')
  page = browser.contexts().flatMap(context => context.pages()).find(item => item.url().includes('tauri'))
  assert.ok(page)
  await page.waitForFunction(() => window.__TAURI_INTERNALS__?.invoke)
  const data = await invoke('data_directory_info_command')
  assert.equal(normalize(data.dataDir), normalize(root))
  assert.equal(data.usingFallback, false)
  const bootstrap = await invoke('get_bootstrap')
  assert.equal(bootstrap.page.totalImageCount, 809214)
  assert.equal(bootstrap.page.images.length, 5000)
  assert.equal(bootstrap.folders.length, 15)
  checks.push('existing root database and bounded first page loaded')
  const query = { scope: 'all', folderId: null, unclassifiedOnlyParentFolderId: null, tags: [], englishQuery: '', fileName: '', naturalLanguage: '', offset: 805000, limit: 5000, randomSeed: null }
  const last = await invoke('query_gallery', { query })
  assert.equal(last.images.length, 4214)
  checks.push('final page beyond 805000 loaded')
  const filtered = await invoke('query_gallery', { query: { ...query, offset: 0, tags: ['1girl'] } })
  assert.ok(filtered.images.length > 0)
  const detail = await invoke('get_image_detail', { imageId: bootstrap.page.images[0].id })
  assert.ok(detail.image)
  checks.push('tag filter and on-demand image detail available')
  const jobs = await invoke('list_jobs')
  assert.equal(jobs.filter(job => ['queued', 'running', 'paused', 'cancelling'].includes(job.state)).length, 0)
  checks.push('startup did not launch background work')
  await page.waitForSelector('[data-gallery-image-id]', { timeout: 120000 })
  const visual = await page.evaluate(() => ({ theme: getComputedStyle(document.documentElement).getPropertyValue('--bg').trim(), overflow: document.documentElement.scrollWidth > innerWidth, width: innerWidth, height: innerHeight, scale: devicePixelRatio }))
  assert.equal(visual.theme, '#fafafa')
  assert.equal(visual.overflow, false)
  checks.push('frameless gallery embedded in production executable')
  outcome = { passed: true, startedAt, root, executable, sha256: digest, sourceCommit: manifest.sourceCommit, visibleImages: bootstrap.page.totalImageCount, firstPageCount: bootstrap.page.images.length, lastPageCount: last.images.length, sourceCount: bootstrap.folders.length, jobsAtStartup: jobs.length, visual, checks }
} catch (error) {
  failure = error
  outcome = { passed: false, startedAt, root, checks, error: error.stack ?? String(error) }
} finally {
  let forced = false
  try {
    if (page && alive()) await bounded(invoke('window_close_command'), 2000, 'Normal production close').catch(() => {})
    for (let attempt = 0; attempt < 150 && alive(); attempt++) await sleep(100)
    if (alive()) {
      forced = true
      child.kill()
      for (let attempt = 0; attempt < 30 && alive(); attempt++) await sleep(100)
    }
    assert.equal(alive(), false, 'Production smoke process did not exit')
    const closing = browser?.close().catch(() => {})
    transport?.close()
    if (closing) await bounded(closing, 5000, 'Production debugging disconnect')
    assert.equal(browser?.isConnected() ?? false, false, 'Debugging connection remained open')
    assert.equal(forced, false, 'Production window required forced termination')
    assert.equal(child.exitCode, 0, 'Production application did not close normally')
  } catch (error) {
    transport?.close()
    failure ??= error
    outcome ??= { startedAt, root, checks }
    outcome.cleanupError = error.stack ?? String(error)
  }
  outcome.passed = !failure
  outcome.completedAt = new Date().toISOString()
  outcome.close = { forced, exitCode: child.exitCode, signalCode: child.signalCode, disconnected: !(browser?.isConnected() ?? false) }
  fs.writeFileSync(reportPath, JSON.stringify(outcome, null, 2))
}
if (failure) throw failure
console.log(JSON.stringify({ passed: true, visibleImages: outcome.visibleImages, checks, close: outcome.close }))
