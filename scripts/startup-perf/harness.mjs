// illuTag startup measurement harness.
// Launches the exe, timestamps Rust stderr, and captures the frontend
// [startup-prof] console logs over the WebView2 DevTools protocol (CDP).
//
// Usage:  node harness.mjs <label> <durationMs>
// Env:    ILLUTAG_EXE, ILLUTAG_CWD, ILLUTAG_OUT
// Requires: Node >= 22 (global WebSocket + fetch).

import { spawn, execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'

const EXE = process.env.ILLUTAG_EXE ?? 'D:\\Ai\\illuTag-danbooru-tag-query\\illutag.exe'
const CWD = process.env.ILLUTAG_CWD ?? 'D:\\Ai\\illuTag-danbooru-tag-query'
const OUTDIR = process.env.ILLUTAG_OUT ?? path.join(process.env.TEMP ?? '.', 'illutag-startup-perf')
fs.mkdirSync(OUTDIR, { recursive: true })

const label = process.argv[2] ?? 'run'
const durationMs = Number(process.argv[3] ?? 30000)
const PORT = Number(process.env.ILLUTAG_CDP_PORT ?? 9222)

const t0 = Date.now()
const hr0 = process.hrtime.bigint()
const elapsed = () => Number(process.hrtime.bigint() - hr0) / 1e6

const stderrFile = path.join(OUTDIR, `${label}.stderr.log`)
const stdoutFile = path.join(OUTDIR, `${label}.stdout.log`)
const consoleFile = path.join(OUTDIR, `${label}.console.jsonl`)
const summaryFile = path.join(OUTDIR, `${label}.summary.json`)

const stderrStream = fs.createWriteStream(stderrFile)
const stdoutStream = fs.createWriteStream(stdoutFile)
const consoleStream = fs.createWriteStream(consoleFile)

const child = spawn(EXE, [], {
  cwd: CWD,
  env: {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT} --remote-allow-origins=*`,
  },
})
const pid = child.pid

let stderrBuf = ''
child.stderr.on('data', (d) => {
  stderrBuf += d.toString('utf8')
  let idx
  while ((idx = stderrBuf.indexOf('\n')) >= 0) {
    const line = stderrBuf.slice(0, idx)
    stderrBuf = stderrBuf.slice(idx + 1)
    stderrStream.write(`[+${elapsed().toFixed(1)}ms] ${line}\n`)
  }
})
let stdoutBuf = ''
child.stdout.on('data', (d) => {
  stdoutBuf += d.toString('utf8')
  let idx
  while ((idx = stdoutBuf.indexOf('\n')) >= 0) {
    const line = stdoutBuf.slice(0, idx)
    stdoutBuf = stdoutBuf.slice(idx + 1)
    stdoutStream.write(`[+${elapsed().toFixed(1)}ms] ${line}\n`)
  }
})

const events = []
function rec(obj) {
  const row = { t_ms: Number(elapsed().toFixed(1)), ...obj }
  events.push(row)
  consoleStream.write(JSON.stringify(row) + '\n')
}

async function waitForTarget(timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://127.0.0.1:${PORT}/json/list`)
      const list = await res.json()
      const page = list.find((t) => t.type === 'page' && !String(t.url).startsWith('devtools://'))
      if (page?.webSocketDebuggerUrl) return page
    } catch {}
    await new Promise((r) => setTimeout(r, 50))
  }
  return null
}

let ws = null
let msgId = 0
const pending = new Map()
function send(method, params = {}) {
  if (!ws || ws.readyState !== 1) return
  const id = ++msgId
  ws.send(JSON.stringify({ id, method, params }))
  return new Promise((resolve) => pending.set(id, resolve))
}

async function cdpEval(expression) {
  const p = send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: false })
  if (!p) return null
  const res = await p
  return res?.result?.result?.value ?? null
}

const target = await waitForTarget()
if (!target) {
  rec({ event: 'cdp_target_not_found' })
} else {
  rec({ event: 'cdp_target_found', url: target.url })
  ws = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve) => {
    ws.onopen = resolve
    ws.onerror = () => resolve()
  })
  ws.onmessage = (m) => {
    let msg
    try {
      msg = JSON.parse(m.data)
    } catch {
      return
    }
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg)
      pending.delete(msg.id)
      return
    }
    const method = msg.method
    if (!method) return
    if (method === 'Runtime.consoleAPICalled') {
      const text = (msg.params.args ?? [])
        .map((a) => a.value ?? a.description ?? a.unserializableValue ?? '')
        .join(' ')
      rec({ event: 'console', level: msg.params.type, text })
    } else if (method === 'Runtime.exceptionThrown') {
      rec({ event: 'exception', text: msg.params.exceptionDetails?.text ?? '' })
    } else if (method === 'Log.entryAdded') {
      rec({ event: 'log', level: msg.params.entry.level, text: msg.params.entry.text })
    } else if (method === 'Page.loadEventFired') {
      rec({ event: 'loadEventFired' })
    } else if (method === 'Page.lifecycleEvent') {
      rec({ event: 'lifecycle', name: msg.params.name })
    }
  }
  await send('Runtime.enable')
  await send('Log.enable')
  await send('Page.enable')
  await send('Page.setLifecycleEventsEnabled', { enabled: true })

  const snapshots = []
  const snapDeadline = Date.now() + durationMs
  while (Date.now() < snapDeadline) {
    const snap = await cdpEval(`(() => {
      const paints = performance.getEntriesByType('paint').map(e => ({ name: e.name, startTime: e.startTime }));
      const nav = performance.getEntriesByType('navigation')[0];
      return {
        perf_now: performance.now(),
        paint: paints,
        domContentLoaded: nav ? nav.domContentLoadedEventEnd : null,
        loadEventEnd: nav ? nav.loadEventEnd : null,
        heap_used: performance.memory ? performance.memory.usedJSHeapSize : null,
        imgs: document.querySelectorAll('img').length,
        masonry_items: document.querySelectorAll('.masonry__item').length,
      };
    })()`)
    if (snap) snapshots.push({ t_ms: Number(elapsed().toFixed(1)), ...snap })
    await new Promise((r) => setTimeout(r, 1000))
  }
  fs.writeFileSync(path.join(OUTDIR, `${label}.snapshots.json`), JSON.stringify(snapshots, null, 1))
}

let mem = null
try {
  const out = execFileSync('powershell', ['-NoProfile', '-Command',
    `$p=Get-Process -Id ${pid} -ErrorAction SilentlyContinue; if($p){[pscustomobject]@{ws=$p.WorkingSet64;peak=$p.PeakWorkingSet64;cpu=$p.TotalProcessorTime.TotalSeconds}|ConvertTo-Json -Compress}`],
    { encoding: 'utf8' })
  mem = JSON.parse(out.trim())
} catch (e) {
  mem = { error: String(e) }
}

const summary = {
  label,
  pid,
  t0_epoch_ms: t0,
  duration_ms: Number(elapsed().toFixed(0)),
  mem,
  stderr_stages: parseStages(fs.readFileSync(stderrFile, 'utf8')),
  console_startup_prof: events.filter((e) => e.event === 'console' && String(e.text).includes('startup-prof')),
}
fs.writeFileSync(summaryFile, JSON.stringify(summary, null, 1))
console.log(JSON.stringify(summary, null, 1))

try {
  child.kill('SIGKILL')
} catch {}
await new Promise((r) => setTimeout(r, 1500))

function parseStages(text) {
  const out = []
  for (const line of text.split('\n')) {
    const m = line.match(/\[\+([\d.]+)ms\]\s*(.*)/)
    if (m) out.push({ t_ms: Number(m[1]), line: m[2] })
  }
  return out
}
