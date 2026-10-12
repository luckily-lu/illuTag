import { chromium } from 'playwright'
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import net from 'node:net'
import WebSocket from 'ws'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
export const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

// Validate the nearest existing parent as well, so a junction cannot redirect
// validation writes into the production installation.
export function isolatedPath(value, label = 'path') {
  if (typeof value !== 'string' || !path.isAbsolute(value)) throw Error(`Explicit absolute isolated ${label} required`)
  const resolved = path.resolve(value)
  const inside = p => p.split(/[\\/]/).some(part => part.toLowerCase() === 'illutag-refactor-work')
  if (!inside(resolved)) throw Error(`${label} must be inside illuTag-refactor-work`)
  let ancestor = resolved
  while (!fs.existsSync(ancestor)) {
    const parent = path.dirname(ancestor)
    if (parent === ancestor) throw Error(`No existing parent for ${label}`)
    ancestor = parent
  }
  if (!inside(fs.realpathSync(ancestor))) throw Error(`${label} resolves outside the isolated workspace`)
  return resolved
}
async function bounded(promise, milliseconds, label) {
  let timer
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(Error(`${label} timed out`)), milliseconds)
    })])
  } finally { clearTimeout(timer) }
}
async function requireFreePort(port) {
  if (!Number.isInteger(port) || port < 1024 || port > 65535) throw Error('Invalid validation debugging port')
  const server = net.createServer()
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolve) })
  await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()))
}
export async function connectDebugging(port) {
  const version = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) }).then(response => response.json())
  const endpoint = new URL(version.webSocketDebuggerUrl)
  if (endpoint.protocol !== 'ws:' || !['127.0.0.1', 'localhost'].includes(endpoint.hostname) || Number(endpoint.port) !== port)
    throw Error('Unexpected validation debugging endpoint')
  const socket = new WebSocket(endpoint, { handshakeTimeout: 1000 })
  let closed = false
  const transport = {
    send(message) { socket.send(JSON.stringify(message)) },
    close() { if (closed) return; closed = true; socket.terminate(); transport.onclose?.() },
  }
  socket.on('error', () => {})
  socket.on('close', () => { if (!closed) { closed = true; transport.onclose?.() } })
  socket.on('message', data => { transport.onmessage?.(JSON.parse(data.toString())) })
  try {
    await bounded(new Promise((resolve, reject) => { socket.once('open', resolve); socket.once('error', reject) }), 1500, 'Debugging socket open')
    // This public transport interface lets us release the owned socket even if
    // WebView2 fails to finish Playwright's ordinary connection shutdown.
    const browser = await chromium.connectOverCDP(transport, { timeout: 5000 })
    return { browser, transport }
  } catch (error) { transport.close(); throw error }
}
export async function launch(executable, root, port = 9781) {
  executable = isolatedPath(executable, 'program')
  root = isolatedPath(root, 'data root')
  if (!fs.statSync(executable).isFile()) throw Error('Validation program does not exist')
  await requireFreePort(port)
  for (const name of ['data', 'profile', 'assets', 'logs', 'pycache']) {
    const directory = isolatedPath(path.join(root, name), name)
    fs.mkdirSync(directory, { recursive: true })
  }
  const logPath = path.join(root, 'logs', `session-${Date.now()}.log`)
  const out = fs.openSync(logPath, 'w'), started = performance.now()
  const child = spawn(executable, [], { cwd: path.dirname(executable), windowsHide: true, stdio: ['ignore', out, out], env: {
    ...process.env,
    ILLUTAG_DATA_ROOT: path.join(root, 'data'), ILLUTAG_PROFILE_ROOT: path.join(root, 'profile'),
    ILLUTAG_ASSET_ROOT: path.join(root, 'assets'), ILLUTAG_LOG_ROOT: path.join(root, 'logs'),
    ILLUTAG_RESOURCE_ROOT: path.dirname(executable), PYTHONPYCACHEPREFIX: path.join(root, 'pycache'),
    WEBVIEW2_USER_DATA_FOLDER: path.join(root, 'profile'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-allow-origins=*`,
  } })
  let spawnError, browser, transport, page, closing
  child.once('error', error => { spawnError = error })
  const alive = () => child.exitCode === null && child.signalCode === null && !spawnError
  const invoke = (command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args })
  function close() {
    if (closing) return closing
    closing = (async () => {
      try {
        if (page && alive()) await bounded(invoke('window_close_command'), 2000, 'Normal window close').catch(() => {})
        const deadline = performance.now() + 15000
        while (alive() && performance.now() < deadline) await sleep(100)
        // This handle belongs only to the process launched above.
        if (alive()) {
          child.kill()
          const killDeadline = performance.now() + 3000
          while (alive() && performance.now() < killDeadline) await sleep(100)
        }
        if (browser) {
          const browserClosing = browser.close()
          // Disconnect only this CDP transport. Do not kill WebView2 processes
          // or shut down the shared Playwright connection between warm runs.
          transport?.close()
          await bounded(browserClosing, 5000, 'Debugging connection close').catch(() => {})
        } else transport?.close()
        if (alive()) throw Error(`Validation process ${child.pid} did not exit`)
      } finally { fs.closeSync(out) }
    })()
    return closing
  }
  try {
    for (let i = 0; i < 160; i++) {
      if (spawnError) throw spawnError
      if (!alive()) throw Error('Application exited before CDP connected')
      try { const connected = await connectDebugging(port); browser = connected.browser; transport = connected.transport; break }
      catch { await sleep(100) }
    }
    if (!browser) throw Error('Independent WebView2 debugging endpoint unavailable')
    page = browser.contexts().flatMap(context => context.pages()).find(p => p.url().includes('tauri')) ?? browser.contexts()[0]?.pages()[0]
    if (!page) throw Error('Independent WebView2 page unavailable')
    await page.waitForFunction(() => window.__TAURI_INTERNALS__?.invoke, { timeout: 30000 })
    return { child, browser, page, invoke, started, close, logPath, root, executable }
  } catch (error) {
    await close().catch(closeError => console.error(`Validation cleanup failed: ${closeError.message}`))
    throw error
  }
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [executable, root] = process.argv.slice(2), session = await launch(executable, root)
  try {
    await session.page.waitForSelector('[data-gallery-image-id]', { timeout: 120000 })
    console.log(JSON.stringify({ url: session.page.url(), data: await session.invoke('data_directory_info_command'), firstScreenMs: performance.now() - session.started }))
    await session.page.screenshot({ path: path.join(root, 'logs', 'recovery-rehearsal.png') })
    await sleep(2000)
  } finally { await session.close() }
}
