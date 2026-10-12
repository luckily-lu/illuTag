import fs from 'node:fs'
import path from 'node:path'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import assert from 'node:assert/strict'

const root = 'D:\\Ai\\illuTag-danbooru-tag-query'
const executable = path.join(root, 'illutag.exe')
const manifest = JSON.parse(fs.readFileSync(path.join(root, 'release-manifest.json'), 'utf8'))
assert.equal(manifest.version, '1.1.0')
assert.equal(manifest.schemaVersion, 2)
assert.equal(createHash('sha256').update(fs.readFileSync(executable)).digest('hex'), manifest.files.find(file => file.path === 'illutag.exe').sha256)
const env = { ...process.env }
for (const key of Object.keys(env)) if (key.startsWith('ILLUTAG_') || key.startsWith('WEBVIEW2_') || key === 'PYTHONPYCACHEPREFIX') delete env[key]
const child = spawn(executable, [], { cwd: root, windowsHide: true, detached: true, stdio: 'ignore', env })
await new Promise((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject) })
child.unref()
console.log(JSON.stringify({ launched: true, pid: child.pid, executable, mode: 'normal production, no testing or debugging overrides' }))
