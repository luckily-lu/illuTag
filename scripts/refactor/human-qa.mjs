import { launch, sleep } from './native-session.mjs'
import fs from 'node:fs'
import path from 'node:path'

const [executable, root] = process.argv.slice(2)
const session = await launch(executable, root, 9785)
await session.page.waitForSelector('[data-gallery-image-id]', { timeout: 30000 })
console.log(JSON.stringify({ pid: session.child.pid, port: 9785, root, ready: true }))
let stop = false
process.stdin.setEncoding('utf8')
process.stdin.on('data', value => { if (value.trim() === 'close') stop = true })
try {
  while (!stop && session.child.exitCode === null) {
    const viewport = await session.page.evaluate(() => ({ width: innerWidth, height: innerHeight, scale: devicePixelRatio })).catch(() => null)
    if (viewport) fs.writeFileSync(path.join(root, 'logs', 'human-viewport.json'), JSON.stringify({ ...viewport, observedAt: new Date().toISOString() }, null, 2))
    await sleep(1000)
  }
} finally {
  await session.close()
}
