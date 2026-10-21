// No Tauri process, production database, real images or visible browser windows.
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
const previousTemp = process.env.TEMP, previousTmp = process.env.TMP;
process.env.TEMP = output; process.env.TMP = output;
const root = fileURLToPath(new URL('../../', import.meta.url));
let server, browser;
const checks = [];
try {
  server = await createServer({ root, server: { host: '127.0.0.1', port: 0, open: false } });
  await server.listen();
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
  browser = await chromium.launch({ channel: 'msedge', headless: true, downloadsPath: output });
  for (const scope of ['all', 'random']) {
    const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
    try {
      await context.route('**/*', route => {
        const url = new URL(route.request().url());
        return ['127.0.0.1', 'localhost'].includes(url.hostname) || ['data:', 'blob:'].includes(url.protocol)
          ? route.continue() : route.abort();
      });
      const page = await context.newPage();
      const errors = [];
      page.on('pageerror', e => errors.push(e.message));
      await page.goto(origin + '/?preview=1');
      await page.waitForSelector('.app');
      // Test-only adapters use the existing in-memory SVG preview collection.
      await page.evaluate(async () => {
        const { api } = await import('/src/shared/ipc.ts');
        const { previewInvoke } = await import('/src/shared/preview.ts');
        const fixture = await previewInvoke('query_gallery', { query: { scope: 'all', offset: 0, limit: 3 } });
        window.__tagFilterQA = { galleryQueries: [], tagQueries: [] };
        const originalQuery = api.query;
        api.query = async q => { window.__tagFilterQA.galleryQueries.push(JSON.parse(JSON.stringify(q))); return originalQuery(q); };
        api.tagCovers = async tags => tags.map(tag => ({ tag, image: fixture.images[0], pinned: false, x: 50, y: 50, zoom: 1 }));
        api.tagPreview = async (tags, offset = 0, limit = 18) => {
          window.__tagFilterQA.tagQueries.push([...tags]);
          const rows = fixture.images.slice(0, tags.includes('outdoors') ? 2 : 3);
          return { images: rows.slice(offset, offset + limit), totalImageCount: rows.length, offset, limit, revision: 1 };
        };
      });
      if (scope === 'random') await page.locator('.sidebar').getByRole('button', { name: '随机', exact: true }).click();
      await page.locator('.sidebar').getByRole('button', { name: '标签', exact: true }).click();
      await page.locator('.tag-card-main').first().click();
      await page.waitForFunction(() => document.querySelectorAll('.tag-preview-masonry img').length === 3);
      const queriesBefore = await page.evaluate(() => window.__tagFilterQA.galleryQueries.length);
      async function addFromViewer() {
        await page.getByRole('button', { name: /查看图片：/ }).first().click();
        const viewer = page.getByRole('dialog', { name: '大图查看器', exact: true });
        await viewer.waitFor();
        const info = viewer.getByRole('button', { name: '信息', exact: true });
        if (await info.getAttribute('aria-pressed') !== 'true') await info.click();
        await viewer.getByRole('button', { name: '户外', exact: true }).click();
        await viewer.waitFor({ state: 'detached' });
      }
      await addFromViewer();
      await page.waitForFunction(() => document.querySelectorAll('.tag-preview-masonry img').length === 2);
      assert.ok(await page.locator('.tag-workbench').isVisible());
      assert.equal(await page.locator('.tag-query-chip').count(), 2);
      const after = await page.evaluate(() => window.__tagFilterQA);
      assert.equal(after.galleryQueries.length, queriesBefore);
      assert.deepEqual(after.tagQueries.at(-1), ['landscape', 'outdoors']);
      // Clicking an existing filter adds nothing and must not toggle it off.
      await addFromViewer();
      assert.equal(await page.locator('.tag-query-chip').count(), 2);
      assert.equal((await page.evaluate(() => window.__tagFilterQA.tagQueries.length)), after.tagQueries.length);
      assert.ok(await page.locator('.tag-workbench').isVisible());
      if (scope === 'random') await page.screenshot({ path: path.join(output, 'tag-filter-preserved.png') });
      checks.push(`Viewer adds a tag to the existing intersection after ${scope} browsing; no gallery navigation/query, no duplicate toggle`);
      // Ordinary gallery viewers keep their existing scope and search behavior.
      await page.locator('.sidebar').getByRole('button', { name: scope === 'all' ? '全部' : '随机', exact: true }).click();
      await page.locator('.image-tile').first().click();
      const viewer = page.getByRole('dialog', { name: '大图查看器', exact: true });
      await viewer.waitFor();
      if (await viewer.getByRole('button', { name: '信息', exact: true }).getAttribute('aria-pressed') !== 'true')
        await viewer.getByRole('button', { name: '信息', exact: true }).click();
      await viewer.getByRole('button', { name: '户外', exact: true }).click();
      await viewer.waitFor({ state: 'detached' });
      await page.waitForFunction(() => window.__tagFilterQA.galleryQueries.at(-1)?.tags?.includes('outdoors'));
      assert.equal((await page.evaluate(() => window.__tagFilterQA.galleryQueries.at(-1))).scope, scope);
      assert.equal(await page.locator('.tag-workbench').count(), 0);
      assert.deepEqual(errors, []);
      checks.push(`Normal ${scope} gallery viewer still adds its tag within that gallery scope`);
    } finally { await context.close(); }
  }
  console.log(JSON.stringify({ passed: true, headless: true, syntheticOnly: true, productionLaunched: false, checks }));
} finally {
  await browser?.close();
  await server?.close();
  if (previousTemp === undefined) delete process.env.TEMP; else process.env.TEMP = previousTemp;
  if (previousTmp === undefined) delete process.env.TMP; else process.env.TMP = previousTmp;
}
