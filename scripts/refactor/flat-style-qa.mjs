import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import fs from 'node:fs';

// This suite uses the browser preview and synthetic SVGs only. The additional
// tag folders are injected into the served preview module, never written to it.
const output = 'output/playwright';
const baseURL = process.env.ILLUTAG_PREVIEW_URL || 'http://127.0.0.1:1421/?preview=1';
assert.equal(new URL(baseURL).searchParams.get('preview'), '1');
fs.mkdirSync(output, { recursive: true });
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const context = await browser.newContext({ viewport: { width: 1280, height: 840 } });
const page = await context.newPage();
const checks = [], snapshots = [], stateChecks = [], scrollbars = [], errors = [];
const floating = '.soft-dialog,.menu-popover,.select-popover,.search-suggestions,.drag-ghost';
page.on('pageerror', error => errors.push(error.message));

await context.route('**/src/shared/preview.ts*', async route => {
  const response = await route.fetch();
  const source = await response.text();
  const marker = 'else if (command === "list_tag_management_state_command")';
  const start = source.indexOf(marker), end = source.indexOf('else if', start + marker.length);
  assert.ok(start >= 0 && end > start, 'preview tag fixture hook changed');
  const fragment = source.slice(start, end);
  assert.match(fragment, /folders:\s*\[\]/, 'expected empty preview tag folders');
  const fixture = JSON.stringify([
    { id: 101, name: '场景参考', sortOrder: 0, tags: ['风景', '建筑'] },
    { id: 102, name: '配色参考', sortOrder: 1, tags: ['中性色'] },
  ]);
  const withTagFolders = source.slice(0, start) + fragment.replace(/folders:\s*\[\]/, `folders: ${fixture}`) + source.slice(end);
  // The in-memory preview returns the same jobs array by reference. Real IPC
  // deserializes fresh objects; clone this response so Vue sees the refresh.
  const jobs = /else if \(command === "list_jobs"\)\s*result = records;/;
  assert.match(withTagFolders, jobs, 'preview jobs fixture hook changed');
  await route.fulfill({ response, body: withTagFolders.replace(jobs, 'else if (command === "list_jobs") result = structuredClone(records);') });
});

const rgba = value => {
  const channels = value.match(/[\d.]+/g)?.map(Number);
  assert.ok(channels && channels.length >= 3, `unparsed color: ${value}`);
  return [...channels.slice(0, 3), channels[3] ?? 1];
};
const neutral = value => {
  const [r, g, b] = rgba(value);
  return Math.max(r, g, b) - Math.min(r, g, b) <= 8;
};
const luminance = value => rgba(value).slice(0, 3).map(c => c / 255).map(c => c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4).reduce((sum, c, index) => sum + c * [.2126, .7152, .0722][index], 0);
const contrast = (a, b) => (Math.max(luminance(a), luminance(b)) + .05) / (Math.min(luminance(a), luminance(b)) + .05);
const settle = async locator => {
  const duration = await locator.evaluate(n => Math.max(0, ...getComputedStyle(n).transitionDuration.split(',').map(v => parseFloat(v) * (v.trim().endsWith('ms') ? 1 : 1000))));
  await page.waitForTimeout(Math.min(500, duration + 30));
};
const ancestorShadowAudit = async (locator, label) => {
  const value = await locator.evaluate((node, allowed) => {
    const shadows = [];
    for (let n = node; n; n = n.parentElement) {
      const shadow = getComputedStyle(n).boxShadow;
      if (shadow !== 'none' && !n.matches(allowed)) shadows.push({ name: `${n.tagName}.${n.className}`, shadow });
    }
    return shadows;
  }, floating);
  assert.deepEqual(value, [], `${label}: control ancestor retains a state shadow`);
};
const overflow = async label => {
  const geometry = await page.evaluate(() => ({ viewport: innerWidth, document: document.documentElement.scrollWidth }));
  assert.ok(geometry.document <= geometry.viewport, `${label}: horizontal document overflow ${JSON.stringify(geometry)}`);
};
const screenshot = async name => {
  const path = `${output}/flat-${name}.png`;
  await page.screenshot({ path });
  snapshots.push(path);
};

async function shadowAudit(label) {
  const result = await page.evaluate(allowed => {
    const shadows = [], controls = [], surfaces = [];
    for (const n of document.querySelectorAll('*')) {
      const rect = n.getBoundingClientRect(), style = getComputedStyle(n);
      if (!rect.width || !rect.height || style.visibility === 'hidden' || style.display === 'none') continue;
      const name = `${n.tagName.toLowerCase()}.${[...n.classList].join('.')} ${n.getAttribute('aria-label') || n.textContent?.trim().slice(0, 35) || ''}`;
      const parts = ['', '::before', '::after'];
      if (n.matches('progress')) parts.push('::-webkit-progress-bar', '::-webkit-progress-value');
      for (const part of parts) {
        const computed = part ? getComputedStyle(n, part) : style;
        if (part && ['::before', '::after'].includes(part) && ['none', 'normal'].includes(computed.content)) continue;
        if (computed.boxShadow !== 'none') shadows.push({ name: name + part, shadow: computed.boxShadow, allowed: !part && n.matches(allowed), control: n.matches('button,input,select,textarea') });
      }
      if (n.matches('button,input,select,textarea')) controls.push(name);
      if (n.matches('.sidebar,.panel,.job-row,.viewer-info,.soft-segment,.segmented,.tag-table tbody tr')) surfaces.push(name);
    }
    return { shadows, controls: controls.length, surfaces: surfaces.length };
  }, floating);
  for (const item of result.shadows) {
    assert.equal(item.control, false, `${label}: control has a shadow: ${JSON.stringify(item)}`);
    assert.equal(item.allowed, true, `${label}: ordinary surface has a shadow: ${JSON.stringify(item)}`);
    assert.equal(item.shadow.includes('inset'), false, `${label}: inset shadow: ${JSON.stringify(item)}`);
    // Split commas outside color functions, then verify one dark downward shadow.
    const layers = item.shadow.split(/,(?![^()]*\))/);
    assert.equal(layers.length, 1, `${label}: double/light shadow: ${JSON.stringify(item)}`);
    const color = item.shadow.match(/rgba?\([^)]*\)/)?.[0];
    assert.ok(color && neutral(color), `${label}: non-neutral floating shadow: ${JSON.stringify(item)}`);
    assert.ok(rgba(color)[3] <= .2 && Math.max(...rgba(color).slice(0, 3)) <= 64, `${label}: floating shadow is too strong/light: ${JSON.stringify(item)}`);
    const dimensions = item.shadow.replace(color, '').match(/-?[\d.]+px/g)?.map(Number.parseFloat);
    assert.ok(dimensions?.length >= 3 && Math.abs(dimensions[0]) <= 8 && dimensions[1] > 0 && dimensions[1] <= 12 && dimensions[2] <= 32 && (dimensions[3] ?? 0) <= 0, `${label}: floating shadow direction/size: ${JSON.stringify(item)}`);
  }
  checks.push({ label, ...result });
}

async function scrollbarAudit(label) {
  const values = await page.evaluate(() => [...document.querySelectorAll('*')].filter(n => {
    const s = getComputedStyle(n), r = n.getBoundingClientRect();
    return r.width && r.height && /auto|scroll/.test(s.overflowX + s.overflowY);
  }).map(n => {
    let ancestor = n, surface = 'rgb(250, 250, 250)';
    while (ancestor) {
      const background = getComputedStyle(ancestor).backgroundColor;
      if (background !== 'rgba(0, 0, 0, 0)') { surface = background; break; }
      ancestor = ancestor.parentElement;
    }
    return { name: `${n.tagName.toLowerCase()}.${[...n.classList].join('.')}`, width: getComputedStyle(n, '::-webkit-scrollbar').width, thumb: getComputedStyle(n, '::-webkit-scrollbar-thumb').backgroundColor, track: getComputedStyle(n, '::-webkit-scrollbar-track').backgroundColor, surface, arrow: getComputedStyle(n, '::-webkit-scrollbar-button').display };
  }));
  for (const value of values) {
    assert.equal(value.width, '12px', `${label}: scrollbar width ${JSON.stringify(value)}`);
    assert.equal(value.arrow, 'none', `${label}: native arrow ${JSON.stringify(value)}`);
    assert.ok(neutral(value.thumb) && neutral(value.track), `${label}: tinted scrollbar ${JSON.stringify(value)}`);
    assert.ok(rgba(value.thumb)[3] > 0 && contrast(value.thumb, rgba(value.track)[3] > 0 ? value.track : value.surface) >= 1.5, `${label}: scrollbar thumb is indistinct ${JSON.stringify(value)}`);
  }
  scrollbars.push({ label, values });
}

async function primaryAudit(label) {
  const values = await page.locator('.soft-button.primary').evaluateAll(nodes => nodes.filter(n => n.getClientRects().length).map(n => ({ name: n.textContent.trim(), background: getComputedStyle(n).backgroundColor, color: getComputedStyle(n).color, shadow: getComputedStyle(n).boxShadow })));
  for (const value of values) {
    assert.ok(neutral(value.background) && Math.max(...rgba(value.background).slice(0, 3)) <= 64 && rgba(value.background)[3] === 1, `${label}: primary is not solid charcoal ${JSON.stringify(value)}`);
    assert.ok(contrast(value.background, value.color) >= 4.5, `${label}: primary label contrast ${JSON.stringify(value)}`);
    assert.equal(value.shadow, 'none');
  }
  const secondary = await page.locator('.soft-button:not(.primary),.text-button').evaluateAll(nodes => nodes.filter(n => n.getClientRects().length).map(n => ({ name: n.textContent.trim(), background: getComputedStyle(n).backgroundColor })));
  for (const value of secondary) assert.ok(rgba(value.background)[3] === 0 || neutral(value.background) && Math.min(...rgba(value.background).slice(0, 3)) >= 100, `${label}: secondary action competes with primary ${JSON.stringify(value)}`);
  checks.push({ label: `${label}: primary hierarchy`, values, secondary });
}

async function sideLineAudit(locator, label) {
  await locator.waitFor();
  const value = await locator.evaluate(n => {
    const s = getComputedStyle(n), p = getComputedStyle(n, '::before');
    return { borderWidth: parseFloat(s.borderLeftWidth), borderStyle: s.borderLeftStyle, borderColor: s.borderLeftColor, background: s.backgroundColor, before: { content: p.content, width: parseFloat(p.width), left: p.left, background: p.backgroundColor } };
  });
  const border = value.borderWidth >= 2 && value.borderWidth <= 4 && value.borderStyle === 'solid' && rgba(value.borderColor)[3] > 0 && neutral(value.borderColor);
  const pseudo = !['none', 'normal'].includes(value.before.content) && value.before.width >= 2 && value.before.width <= 4 && value.before.left === '0px' && rgba(value.before.background)[3] > 0 && neutral(value.before.background);
  assert.ok(border || pseudo, `${label}: missing neutral selected side line ${JSON.stringify(value)}`);
  assert.equal(rgba(value.background)[3], 0, `${label}: selected navigation should be frameless ${JSON.stringify(value)}`);
  checks.push({ label, value });
}

async function underlineAudit(label) {
  const value = await page.locator('.soft-segment button.is-active').evaluate(n => {
    const s = getComputedStyle(n), p = getComputedStyle(n, '::after');
    return { text: n.textContent.trim(), width: parseFloat(s.borderBottomWidth), style: s.borderBottomStyle, color: s.borderBottomColor, background: s.backgroundColor, after: { content: p.content, height: parseFloat(p.height), bottom: p.bottom, background: p.backgroundColor } };
  });
  const border = value.width >= 2 && value.style === 'solid' && rgba(value.color)[3] > 0 && neutral(value.color);
  const pseudo = !['none', 'normal'].includes(value.after.content) && value.after.height >= 2 && value.after.bottom === '0px' && rgba(value.after.background)[3] > 0 && neutral(value.after.background);
  assert.ok(border || pseudo, `${label}: missing selected search underline ${JSON.stringify(value)}`);
  assert.equal(rgba(value.background)[3], 0, `${label}: search mode has a raised/filled background`);
  checks.push({ label, value });
}

async function controlStates(root, label) {
  // One representative of every tag/class/role/context combination exercises
  // all CSS variants without repeating the same 48 thumbnail controls.
  await page.locator('[data-flat-qa-control]').evaluateAll(nodes => nodes.forEach(n => n.removeAttribute('data-flat-qa-control')));
  const candidates = await page.locator(root).evaluateAll(roots => {
    const seen = new Set(), entries = [];
    for (const root of roots) for (const n of root.querySelectorAll('button,input,select,textarea')) {
      const r = n.getBoundingClientRect();
      if (!r.width || !r.height || n.disabled || getComputedStyle(n).visibility === 'hidden') continue;
      const parents = [n.parentElement, n.parentElement?.parentElement].map(p => p?.className || '').join('/');
      const key = [n.tagName, n.className, n.getAttribute('type'), n.getAttribute('role'), n.getAttribute('aria-pressed'), n.getAttribute('aria-expanded'), parents].join('|');
      if (seen.has(key)) continue;
      seen.add(key);
      const id = `flat-control-${entries.length}`;
      n.setAttribute('data-flat-qa-control', id);
      entries.push({ id, key, name: n.getAttribute('aria-label') || n.textContent.trim().slice(0, 35), button: n.matches('button'), disabledVariant: n.matches('.soft-button,.soft-select,input:not([type="checkbox"])') });
    }
    return entries;
  });
  for (const entry of candidates) {
    const control = page.locator(`[data-flat-qa-control="${entry.id}"]`);
    if (!await control.isVisible()) continue;
    await control.scrollIntoViewIfNeeded();
    const danger = await control.evaluate(n => n.classList.contains('danger'));
    if (danger) {
      await page.mouse.move(1, 1); await settle(control);
      assert.equal(await control.evaluate(n => getComputedStyle(n).color), 'rgb(158, 52, 52)', `${label}: danger loses its default red ${entry.key}`);
    }
    await control.hover(); await settle(control);
    const hover = await control.evaluate(n => ({ shadow: getComputedStyle(n).boxShadow, transform: getComputedStyle(n).transform, color: getComputedStyle(n).color }));
    assert.equal(hover.shadow, 'none', `${label}: hover shadow ${entry.key}`);
    assert.equal(hover.transform, 'none', `${label}: raised hover transform ${entry.key}`);
    if (danger) assert.equal(hover.color, 'rgb(158, 52, 52)', `${label}: danger loses its hover red ${entry.key}`);
    await ancestorShadowAudit(control, `${label}: hover ${entry.key}`);
    const box = await control.boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down(); await settle(control);
    assert.equal(await control.evaluate(n => getComputedStyle(n).boxShadow), 'none', `${label}: active shadow ${entry.key}`);
    await ancestorShadowAudit(control, `${label}: active ${entry.key}`);
    // Release outside the control so style testing never invokes its action.
    // Stay inside a dialog: releasing on its backdrop would close it.
    const heading = page.locator('.soft-dialog h2').first();
    const release = await heading.isVisible() ? await heading.boundingBox() : null;
    await page.mouse.move(release ? release.x + release.width / 2 : 1, release ? release.y + release.height / 2 : 1); await page.mouse.up();
    // F2 establishes keyboard modality and has no application handler. Tab
    // closes popups, while ArrowDown opens a select when its trigger has focus.
    await page.keyboard.press('F2'); await control.focus(); await settle(control);
    const focus = await control.evaluate(n => ({ visible: n.matches(':focus-visible'), shadow: getComputedStyle(n).boxShadow, width: parseFloat(getComputedStyle(n).outlineWidth), style: getComputedStyle(n).outlineStyle, color: getComputedStyle(n).outlineColor }));
    assert.equal(focus.visible, true, `${label}: keyboard focus-visible ${entry.key}`);
    assert.equal(focus.shadow, 'none', `${label}: focus shadow ${entry.key}`);
    assert.ok(focus.width >= 2 && focus.style === 'solid' && neutral(focus.color) && rgba(focus.color)[3] > 0, `${label}: missing visible neutral focus outline ${entry.key}: ${JSON.stringify(focus)}`);
    await ancestorShadowAudit(control, `${label}: focus ${entry.key}`);
    if (entry.disabledVariant) {
      // Only the DOM disabled flag is changed. This covers the CSS state without
      // starting native work or depending on an instantaneous preview request.
      await control.evaluate(n => { n.blur(); n.disabled = true; });
      await page.mouse.move(1, 1); await settle(control);
      const before = await control.evaluate(n => ({ background: getComputedStyle(n).backgroundColor, color: getComputedStyle(n).color, shadow: getComputedStyle(n).boxShadow, opacity: getComputedStyle(n).opacity }));
      await control.hover({ force: true }); await settle(control);
      const after = await control.evaluate(n => ({ background: getComputedStyle(n).backgroundColor, color: getComputedStyle(n).color, shadow: getComputedStyle(n).boxShadow, opacity: getComputedStyle(n).opacity, disabled: n.matches(':disabled') }));
      assert.equal(after.disabled, true);
      assert.equal(after.shadow, 'none', `${label}: disabled shadow ${entry.key}`);
      assert.deepEqual({ background: after.background, color: after.color, shadow: after.shadow }, { background: before.background, color: before.color, shadow: before.shadow }, `${label}: disabled hover changes appearance ${entry.key}`);
      if (entry.button) assert.ok(Number(after.opacity) <= .65, `${label}: disabled button not visually distinct ${entry.key}`);
      await control.evaluate(n => { n.disabled = false; });
    }
    stateChecks.push({ label, ...entry, states: ['hover', 'active', 'keyboard-focus', ...(entry.disabledVariant ? ['disabled', 'disabled-hover'] : [])] });
  }
  await page.mouse.move(1, 1);
  await shadowAudit(`${label}: after state checks`);
}

async function inspect(label, { root = '.app', states = false, shot = true } = {}) {
  await overflow(label); await shadowAudit(label); await primaryAudit(label); await scrollbarAudit(label);
  if (shot) await screenshot(label);
  if (states) await controlStates(root, label);
  console.log(`Flat style QA: ${label}`);
}
async function navigate(label) {
  const target = page.locator('.sidebar').getByRole('button', { name: label, exact: true });
  if (!await target.isVisible()) await page.getByRole('button', { name: '展开导航', exact: true }).click();
  await target.click();
  await page.waitForTimeout(160);
}

let passed = false, failure;
try {
  await page.goto(baseURL); await page.waitForSelector('[data-gallery-image-id]');
  const tokens = await page.evaluate(() => {
    const s = getComputedStyle(document.documentElement);
    return Object.fromEntries(['--bg', '--surface', '--accent', '--muted', '--border'].map(name => [name, s.getPropertyValue(name).trim()]));
  });
  assert.deepEqual(tokens, { '--bg': '#fafafa', '--surface': '#ffffff', '--accent': '#252525', '--muted': '#666966', '--border': '#dedfdd' });
  assert.equal(await page.locator('.search-submit.primary').count(), 1, 'gallery search must be a primary action');
  await sideLineAudit(page.locator('.sidebar .nav-row.active'), 'left navigation side line');
  await inspect('gallery', { states: true });
  for (const label of ['标签', '文件名', '自然语言']) {
    await page.locator('.soft-segment').getByRole('button', { name: label, exact: true }).click();
    await underlineAudit(`search mode ${label}`); await shadowAudit(`search mode ${label}`);
  }
  await page.locator('.soft-segment').getByRole('button', { name: '标签', exact: true }).click();
  await page.getByRole('textbox', { name: '标签搜索' }).fill('landscape');
  await page.waitForSelector('.search-suggestions');
  await inspect('search-suggestions', { root: '.search-suggestions', states: true });
  await page.locator('.search-suggestions button').first().click();
  await page.getByRole('button', { name: '清空条件', exact: true }).click();
  await page.locator('.folder-name').filter({ hasText: '风景与空间' }).click();
  await sideLineAudit(page.locator('.folder-row.active'), 'folder navigation side line');
  await navigate('全部');

  const folderMenu = page.getByRole('button', { name: '文件夹操作：风景与空间', exact: true });
  await folderMenu.click(); await page.waitForSelector('.action-menu');
  await inspect('folder-menu', { root: '.action-menu', states: true });
  await page.keyboard.press('Escape');
  await folderMenu.click(); await page.getByRole('menuitem', { name: '自动分类规则', exact: true }).click();
  await page.waitForSelector('.soft-dialog');
  await page.getByRole('button', { name: '添加条件', exact: true }).click();
  await inspect('rule-dialog', { root: '.soft-dialog', states: true });
  await page.getByRole('combobox', { name: '条件来源', exact: true }).click();
  await page.waitForSelector('.select-popover');
  await inspect('dialog-select', { root: '.select-popover', states: true });
  await page.keyboard.press('Escape'); await page.keyboard.press('Escape');

  await page.getByRole('button', { name: '查看 画面_1.png', exact: true }).click();
  await inspect('viewer', { root: '.viewer', states: true });
  await page.getByRole('button', { name: '信息', exact: true }).click();
  await page.waitForSelector('.tag-table');
  await inspect('viewer-info', { root: '.viewer', states: true });
  await page.locator('.viewer-info').evaluate(n => {
    const section = n.querySelector('.info-tag-section');
    n.scrollTop += section.getBoundingClientRect().top - n.getBoundingClientRect().top - 18;
  });
  await screenshot('viewer-tags');
  await page.getByRole('combobox', { name: '标签类型', exact: true }).click();
  await page.waitForSelector('.select-popover');
  await inspect('viewer-select', { root: '.select-popover', states: true });
  await page.keyboard.press('Escape'); await page.keyboard.press('Escape');

  await navigate('标签'); await page.waitForSelector('.tag-manager aside .nav-row.active');
  await sideLineAudit(page.locator('.tag-manager aside .nav-row.active'), 'tag folder navigation side line');
  await inspect('tags', { states: true });
  await page.getByRole('button', { name: '标签字典', exact: true }).click();
  await page.waitForSelector('.detail-tag'); await inspect('dictionary', { states: true });
  await page.getByRole('combobox', { name: '字典分组', exact: true }).click();
  await inspect('dictionary-select', { root: '.select-popover', states: true });
  await page.keyboard.press('Escape');

  await navigate('任务中心');
  await page.getByRole('button', { name: '一键处理', exact: true }).click();
  await page.waitForSelector('.job-row'); await inspect('tasks', { states: true });
  await navigate('设置');
  for (const [label, name] of [['外观与窗口', 'appearance'], ['图库与存储', 'storage'], ['备份与导出', 'backup']]) {
    await page.getByRole('button', { name: label, exact: true }).click();
    await inspect(`settings-${name}`, { states: true });
  }

  await page.setViewportSize({ width: 500, height: 500 });
  for (const [label, name] of [['全部', 'gallery'], ['标签', 'tags'], ['任务中心', 'tasks'], ['设置', 'settings']]) {
    await navigate(label); await inspect(`${name}-500`);
    if (label === '设置') for (const section of ['图库与存储', '备份与导出']) {
      await page.getByRole('button', { name: section, exact: true }).click(); await inspect(`settings-${section}-500`);
    }
  }
  await navigate('全部');
  await page.getByRole('button', { name: '展开导航', exact: true }).click();
  await inspect('navigation-500');
  await page.getByRole('button', { name: '新建虚拟文件夹', exact: true }).click();
  await page.waitForSelector('.soft-dialog'); await inspect('dialog-500', { root: '.soft-dialog' });
  await page.keyboard.press('Escape');
  if (await page.locator('.drawer-scrim').count()) await page.keyboard.press('Escape');
  await page.getByRole('button', { name: '查看 画面_1.png', exact: true }).click();
  if (!await page.locator('.viewer-info').count()) await page.getByRole('button', { name: '信息', exact: true }).click();
  await inspect('viewer-info-500', { root: '.viewer' });
  const panel = await page.locator('.viewer-info').evaluate(n => ({ width: n.clientWidth, scrollWidth: n.scrollWidth }));
  assert.ok(panel.scrollWidth <= panel.width, `500px information panel overflow: ${JSON.stringify(panel)}`);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: '选择', exact: true }).click();
  await page.getByRole('button', { name: '选择当前已加载结果', exact: true }).click();
  await page.getByRole('button', { name: '更多操作', exact: true }).click();
  await inspect('batch-dialog-500', { root: '.soft-dialog' });
  await page.keyboard.press('Escape'); await page.keyboard.press('Escape');

  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.setViewportSize({ width: 1280, height: 840 });
  const motion = await page.evaluate(() => [...document.querySelectorAll('button,input,textarea,.image-tile,.titlebar')].filter(n => n.getClientRects().length).map(n => ({ name: `${n.tagName}.${n.className}`, transition: getComputedStyle(n).transitionDuration, animation: getComputedStyle(n).animationDuration })));
  for (const value of motion) for (const kind of ['transition', 'animation']) assert.ok(value[kind].split(',').every(duration => parseFloat(duration) <= .001), `reduced motion ${kind}: ${JSON.stringify(value)}`);
  await inspect('reduced-motion', { states: true });
  checks.push({ label: 'reduced motion durations', motion });
  assert.deepEqual(errors, [], 'unexpected browser errors');
  passed = true;
  console.log(JSON.stringify({ passed, snapshots: snapshots.length, audits: checks.length, controlVariants: stateChecks.length, checks: ['flat surfaces and controls', 'single downward floating shadows', 'charcoal primary hierarchy', 'navigation side lines', 'search underlines', 'neutral scrollbars', 'hover/active/disabled/focus', '500px overflow', 'reduced motion'] }));
} catch (error) {
  failure = { message: error.message, stack: error.stack };
  await screenshot('failure');
  throw error;
} finally {
  fs.writeFileSync(`${output}/flat-style-qa.json`, JSON.stringify({ passed, baseURL, checks, stateChecks, scrollbars, snapshots, errors, failure }, null, 2));
  await browser.close();
}
