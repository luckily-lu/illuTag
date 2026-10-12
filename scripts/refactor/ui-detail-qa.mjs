import { chromium } from 'playwright';
import fs from 'node:fs';
import assert from 'node:assert/strict';
const output = 'output/playwright';
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const context = await browser.newContext({ viewport: { width: 1280, height: 840 } });
const page = await context.newPage(), checks = [];
const overflow = async () => {
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
};
try {
  await page.goto('http://127.0.0.1:1421/?preview=1');
  await page.waitForSelector('[data-gallery-image-id]');
  assert.equal(await page.locator('.nav-icon svg').count(), 8);
  const input = page.getByRole('textbox', { name: '标签搜索' });
  await input.fill('landscape'); await page.waitForSelector('.search-suggestions');
  await page.locator('.search-suggestions button').first().click();
  await page.waitForFunction(() => document.querySelector('.active-filters').textContent.includes('风景'));
  assert.equal(await page.locator('.tag-chip').getAttribute('title'), 'landscape');
  await page.screenshot({ path: `${output}/ui-gallery-updated.png` });
  await page.getByRole('button', { name: '清空条件' }).click();
  await page.getByRole('button', { name: '查看 画面_1.png', exact: true }).click();
  await page.getByRole('button', { name: '信息', exact: true }).click();
  await page.waitForSelector('.tag-table');
  assert.equal(await page.locator('.tag-table thead th').count(), 6);
  assert.equal(await page.locator('.tag-table tbody tr').count(), 17);
  await page.locator('.viewer-info').evaluate(n => n.scrollTop = 600);
  await page.screenshot({ path: `${output}/ui-viewer-tags-updated.png` });
  const type = page.getByRole('combobox', { name: '标签类型' });
  await type.click(); await page.waitForSelector('.select-popover');
  await page.keyboard.press('ArrowDown'); await page.keyboard.press('Enter');
  assert.match(await type.textContent(), /补充模型标签/);
  await type.click(); await page.keyboard.press('Escape');
  assert.equal(await page.locator('.select-popover').count(), 0);
  assert.equal(await page.locator('.viewer').count(), 1);
  await page.getByRole('textbox', { name: '新增标签' }).focus();
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('.viewer').count(), 0);
  await page.getByRole('button', { name: '查看 画面_1.png', exact: true }).click();
  for (const width of [500, 768, 1024, 1280, 1920]) {
    await page.setViewportSize({ width, height: width === 500 ? 500 : 840 });
    await page.waitForTimeout(350);
    await overflow();
    const panel = await page.locator('.viewer-info').evaluate(n => ({ width: n.clientWidth, scrollWidth: n.scrollWidth }));
    assert.ok(panel.scrollWidth <= panel.width, `tag panel overflows at ${width}px`);
    assert.equal(await page.getByRole('button', { name: '关闭查看器' }).isVisible(), true);
    if (width === 500 || width === 1920) await page.screenshot({ path: `${output}/ui-viewer-${width}-updated.png` });
    checks.push(`viewer table at ${width}px`);
  }
  await page.keyboard.press('Escape');
  checks.push('SVG navigation', 'Chinese chip preserves English query ID', 'aligned bilingual tag table', 'custom select keyboard / Escape layering', 'Escape in text field');
  for (const width of [500, 768, 1024, 1280, 1920]) {
    await page.setViewportSize({ width, height: 840 });
    for (const label of ['标签', '任务中心', '设置']) {
      const navigation = page.locator('.sidebar').getByRole('button', { name: label, exact: true });
      if (!await navigation.isVisible()) await page.getByRole('button', { name: '展开导航', exact: true }).click();
      await navigation.click(); await overflow();
      if (label === '标签') {
        await page.getByRole('button', { name: '标签字典', exact: true }).click();
        const group = page.getByRole('combobox', { name: '字典分组' });
        await group.click(); await overflow(); await page.keyboard.press('Escape');
      }
      if (label === '设置') {
        for (const section of ['外观与窗口', '图库与存储', '备份与导出']) { await page.getByRole('button', { name: section, exact: true }).click(); await overflow(); }
      }
      if (width === 1280) {
        // Capture the settled control state after navigation.
        await page.waitForTimeout(350);
        await page.screenshot({ path: `${output}/ui-${label}-updated.png` });
      }
    }
    checks.push(`all pages at ${width}px`);
  }
  assert.equal(await page.locator('select').count(), 0);
  const scrollbar = await page.locator('.page-content').evaluate(n => ({ width: getComputedStyle(n, '::-webkit-scrollbar').width, thumb: getComputedStyle(n, '::-webkit-scrollbar-thumb').backgroundColor, arrow: getComputedStyle(n, '::-webkit-scrollbar-button').display }));
  assert.equal(scrollbar.width, '12px'); assert.equal(scrollbar.arrow, 'none');
  assert.ok(['rgb(184, 186, 182)', 'rgb(146, 150, 143)'].includes(scrollbar.thumb), 'scrollbar thumb uses the neutral default or hover color');
  checks.push('global scrollbars and hidden native arrows', 'all native selects replaced');
  fs.writeFileSync(`${output}/ui-detail-qa.json`, JSON.stringify({ passed: true, checks, scrollbar }, null, 2));
  console.log(JSON.stringify({ passed: true, checks }));
} finally { await browser.close(); }
