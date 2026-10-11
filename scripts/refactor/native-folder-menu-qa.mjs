import { launch } from "./native-session.mjs";
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";

const [executable, root] = process.argv.slice(2);
const session = await launch(executable, root, 9782);
const { page } = session,
  checks = [];
async function ensureNavigation() {
  if (!(await page.locator(".folder-menu-trigger").first().isVisible()))
    await page.getByRole("button", { name: "展开导航", exact: true }).click();
}
try {
  await page.waitForSelector("[data-gallery-image-id]");
  await ensureNavigation();
  const first = page.locator(".folder-menu-trigger").first();
  const second = page.locator(".folder-menu-trigger").nth(1);
  assert.ok((await page.locator(".folder-menu-trigger").count()) >= 2);
  await first.click();
  await page.waitForSelector(".action-menu");
  if (await page.locator(".drawer-scrim").count())
    await page.getByRole("button", { name: "关闭导航", exact: true }).click();
  else await page.locator(".search-input-wrap input").click();
  assert.equal(await page.locator(".action-menu").count(), 0);
  checks.push("one outside click closes menu in WebView2");
  await ensureNavigation();
  await first.click();
  await second.click();
  assert.equal(await page.locator(".action-menu").count(), 1);
  assert.equal(await first.getAttribute("aria-expanded"), "false");
  checks.push("one click switches folder menus");
  await page.keyboard.press("Escape");
  assert.equal(await page.locator(".action-menu").count(), 0);
  assert.equal(
    await second.evaluate((n) => n === document.activeElement),
    true,
  );
  checks.push("Escape closes menu and restores focus");
  for (const label of [
    "新建子文件夹",
    "重命名",
    "自动分类规则",
    "删除文件夹",
  ]) {
    await second.click();
    await page.getByRole("menuitem", { name: label, exact: true }).click();
    await page.waitForSelector(".soft-dialog");
    assert.equal(await page.locator(".action-menu").count(), 0);
    await page.keyboard.press("Escape");
    assert.equal(await page.locator(".action-menu").count(), 0);
    checks.push(`${label} closes menu before dialog`);
  }
  await second.click();
  await page
    .getByRole("menuitem", { name: "仅本级待分类", exact: true })
    .click();
  assert.equal(await page.locator(".action-menu").count(), 0);
  checks.push("scope action closes menu");
  await ensureNavigation();
  await page.locator(".sidebar > .nav-row").first().click();
  await ensureNavigation();
  await second.click();
  await page.screenshot({
    path: path.join(root, "logs", "native-folder-menu.png"),
  });
  await page.keyboard.press("Escape");
  if (await page.locator(".drawer-scrim").count())
    await page.keyboard.press("Escape");
  fs.writeFileSync(
    path.join(root, "logs", "native-folder-menu-qa.json"),
    JSON.stringify({ passed: true, checks }, null, 2),
  );
  console.log(JSON.stringify({ passed: true, checks }));
} finally {
  await session.close();
}
