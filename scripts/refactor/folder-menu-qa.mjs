import { chromium } from "playwright";
import fs from "node:fs";
import assert from "node:assert/strict";
const browser = await chromium.launch({ channel: "chrome", headless: true });
const page = await browser.newPage({ viewport: { width: 1280, height: 840 } }),
  checks = [];
try {
  await page.goto("http://127.0.0.1:1421/?preview=1");
  await page.waitForSelector(".image-tile");
  const first = page.getByRole("button", {
      name: "文件夹操作：风景与空间",
      exact: true,
    }),
    second = page.getByRole("button", {
      name: "文件夹操作：人物与故事",
      exact: true,
    });
  await first.click();
  await page.waitForSelector(".action-menu");
  await page.getByRole("textbox", { name: "标签搜索" }).click();
  assert.equal(await page.locator(".action-menu").count(), 0);
  checks.push("one outside click closes menu");
  await first.click();
  await second.click();
  assert.equal(await page.locator(".action-menu").count(), 1);
  assert.equal(await second.getAttribute("aria-expanded"), "true");
  assert.equal(await first.getAttribute("aria-expanded"), "false");
  checks.push("switching folders closes previous menu");
  await page.keyboard.press("Escape");
  assert.equal(await page.locator(".action-menu").count(), 0);
  assert.equal(
    await second.evaluate((n) => n === document.activeElement),
    true,
  );
  checks.push("Escape restores trigger focus");
  await first.click();
  await first.click();
  assert.equal(await page.locator(".action-menu").count(), 0);
  checks.push("trigger toggles menu closed with one click");
  await first.click();
  await page.keyboard.press("Tab");
  assert.equal(await page.locator(".action-menu").count(), 0);
  checks.push("Tab closes menu");
  await first.focus();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await page.waitForSelector(".soft-dialog");
  assert.equal(await page.locator(".action-menu").count(), 0);
  assert.match(await page.locator(".soft-dialog h2").textContent(), /重命名/);
  await page.keyboard.press("Escape");
  assert.equal(await page.locator(".action-menu").count(), 0);
  checks.push("choosing an action closes menu before dialog");
  for (const label of [
    "新建子文件夹",
    "重命名",
    "自动分类规则",
    "删除文件夹",
  ]) {
    await first.click();
    await page.getByRole("menuitem", { name: label, exact: true }).click();
    await page.waitForSelector(".soft-dialog");
    assert.equal(await page.locator(".action-menu").count(), 0);
    await page.keyboard.press("Escape");
    assert.equal(await page.locator(".action-menu").count(), 0);
    checks.push(`${label} closes menu before dialog`);
  }
  await first.click();
  await page
    .getByRole("menuitem", { name: "仅本级待分类", exact: true })
    .click();
  assert.equal(await page.locator(".action-menu").count(), 0);
  checks.push("scope action closes menu");
  for (const width of [500, 768, 1071, 1280, 1920]) {
    await page.setViewportSize({ width, height: 500 });
    if (!(await first.isVisible()))
      await page.getByRole("button", { name: "展开导航", exact: true }).click();
    await second.click();
    await page.waitForSelector(".action-menu");
    const bounds = await page.locator(".action-menu").evaluate((n) => {
      const r = n.getBoundingClientRect();
      return {
        left: r.left,
        right: r.right,
        top: r.top,
        bottom: r.bottom,
        width: innerWidth,
        height: innerHeight,
      };
    });
    assert.ok(
      bounds.left >= 0 &&
        bounds.right <= bounds.width &&
        bounds.top >= 0 &&
        bounds.bottom <= bounds.height,
      JSON.stringify(bounds),
    );
    if (width === 1280)
      await page.screenshot({
        path: "output/playwright/ui-folder-menu-fixed.png",
      });
    await page.keyboard.press("Escape");
    if (await page.locator(".drawer-scrim").count())
      await page.keyboard.press("Escape");
    checks.push(`menu fits ${width}×500px`);
  }
  fs.writeFileSync(
    "output/playwright/folder-menu-qa.json",
    JSON.stringify({ passed: true, checks }, null, 2),
  );
  console.log(JSON.stringify({ passed: true, checks }));
} finally {
  await browser.close();
}
