// Headless Edge, isolated SVG preview data only. Never starts the installed EXE.
import { createServer } from 'vite';
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const output = path.resolve(process.argv[2] || '');
assert.ok(process.argv[2] && path.isAbsolute(process.argv[2]));
assert.ok(output.split(/[\\/]/).some(s => s.toLowerCase() === 'illutag-refactor-work'));
let ancestor = output;
while (!fs.existsSync(ancestor)) ancestor = path.dirname(ancestor);
assert.ok(fs.realpathSync(ancestor).split(/[\\/]/).some(s => s.toLowerCase() === 'illutag-refactor-work'));
fs.mkdirSync(output, { recursive: true });
const oldTemp = process.env.TEMP, oldTmp = process.env.TMP;
process.env.TEMP = output; process.env.TMP = output;
let server, browser;
const checks = [];
try {
  server = await createServer({ root: fileURLToPath(new URL('../../', import.meta.url)), server: { host: '127.0.0.1', port: 0, open: false } });
  await server.listen();
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
  browser = await chromium.launch({ channel: 'msedge', headless: true, downloadsPath: output });
  for (const width of [1430, 768, 500]) {
    const context = await browser.newContext({ viewport: { width: 1430, height: 840 } });
    try {
      await context.route('**/*', route => {
        const url = new URL(route.request().url());
        return ['127.0.0.1', 'localhost'].includes(url.hostname) || ['data:', 'blob:'].includes(url.protocol) ? route.continue() : route.abort();
      });
      const page = await context.newPage();
      const errors = [];
      page.on('pageerror', e => errors.push(e.message));
      await page.goto(origin + '/?preview=1&duplicates=ready');
      await page.waitForSelector('.app');
      await page.evaluate(async () => {
        const { api } = await import('/src/shared/ipc.ts');
        window.__duplicateQA = [];
        const start = api.startJob;
        api.startJob = async request => { window.__duplicateQA.push(JSON.parse(JSON.stringify(request))); return start(request); };
      });
      await page.locator('.sidebar').getByRole('button', { name: '重复检索', exact: true }).click();
      await page.getByText('外观近似 · 候选尚未复核局部细节', { exact: false }).waitFor();
      await page.setViewportSize({ width, height: 840 });
      await page.locator('.duplicate-scan-settings > summary').click();
      const mode = page.getByRole('combobox', { name: '近似检索模式' });
      assert.match(await mode.innerText(), /同图副本/);
      await mode.click();
      await page.getByRole('option', { name: '外观近似（原有算法）', exact: true }).click();
      assert.equal(await page.evaluate(() => window.__duplicateQA.length), 0);
      await mode.click();
      await page.getByRole('option', { name: '同图副本（细节复核）', exact: true }).click();
      await page.getByRole('button', { name: '应用设置并重新检索', exact: true }).click();
      await page.getByText('同图副本 · 已复核局部细节', { exact: false }).waitFor();
      const requests = await page.evaluate(() => window.__duplicateQA);
      assert.equal(requests.length, 1);
      assert.equal(requests[0].duplicateOptions.verificationMode, 'verified');
      assert.equal(requests[0].duplicateOptions.regroupOnly, true);
      assert.ok(await page.getByRole('button', { name: '复核并整理近似副本', exact: true }).isVisible());
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
      assert.equal(overflow, false);
      assert.deepEqual(errors, []);
      if (width === 1430) await page.screenshot({ path: path.join(output, 'refinement-synthetic.png') });
      checks.push(`${width}px: mode selection is manual, existing result stays labelled until submission, typed request and completed state, no horizontal overflow`);
    } finally { await context.close(); }
  }
  console.log(JSON.stringify({ passed: true, headless: true, syntheticOnly: true, productionLaunched: false, checks }));
} finally {
  await browser?.close();
  await server?.close();
  if (oldTemp === undefined) delete process.env.TEMP; else process.env.TEMP = oldTemp;
  if (oldTmp === undefined) delete process.env.TMP; else process.env.TMP = oldTmp;
}
