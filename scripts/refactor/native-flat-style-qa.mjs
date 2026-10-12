import { launch } from "./native-session.mjs";
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";

// Native visual verification uses an explicitly isolated existing fixture.
// It does not launch jobs, import sources or alter image metadata.
const [executable, root] = process.argv.slice(2);
const session = await launch(executable, root, 9782);
const { page } = session;
const checks = [],
  screenshots = [];
const floating =
  ".soft-dialog,.menu-popover,.select-popover,.search-suggestions,.drag-ghost";

async function audit(label) {
  const result = await page.evaluate((allowed) => {
    const shadows = [],
      backgrounds = [];
    for (const node of document.querySelectorAll("*")) {
      const rect = node.getBoundingClientRect();
      if (!rect.width || !rect.height) continue;
      const style = getComputedStyle(node);
      if (style.visibility === "hidden" || style.display === "none") continue;
      const name = `${node.tagName}.${node.className}`;
      if (style.boxShadow !== "none")
        shadows.push({
          name,
          shadow: style.boxShadow,
          allowed: node.matches(allowed),
          control: node.matches("button,input,select,textarea"),
        });
      if (node.matches(".soft-button.primary"))
        backgrounds.push({
          background: style.backgroundColor,
          color: style.color,
        });
    }
    return {
      shadows,
      backgrounds,
      bg: getComputedStyle(document.documentElement)
        .getPropertyValue("--bg")
        .trim(),
      overflow: document.documentElement.scrollWidth > innerWidth,
      width: innerWidth,
      height: innerHeight,
      scale: devicePixelRatio,
    };
  }, floating);
  assert.equal(
    result.bg,
    "#fafafa",
    `${label}: release did not embed new theme`,
  );
  assert.equal(result.overflow, false, `${label}: horizontal overflow`);
  for (const shadow of result.shadows) {
    assert.equal(
      shadow.control,
      false,
      `${label}: control shadow ${shadow.name}`,
    );
    assert.equal(
      shadow.allowed,
      true,
      `${label}: ordinary surface shadow ${shadow.name}`,
    );
    assert.equal(shadow.shadow.includes("inset"), false);
    assert.equal(shadow.shadow.split(/,(?![^()]*\))/).length, 1);
  }
  for (const primary of result.backgrounds) {
    assert.equal(primary.background, "rgb(37, 37, 37)");
    assert.equal(primary.color, "rgb(255, 255, 255)");
  }
  checks.push({ label, ...result });
  const screenshot = path.join(root, "logs", `native-flat-${label}.png`);
  await page.screenshot({ path: screenshot });
  screenshots.push(screenshot);
}

async function navigate(label) {
  const button = page
    .locator(".sidebar")
    .getByRole("button", { name: label, exact: true });
  if (!(await button.isVisible()))
    await page.getByRole("button", { name: "展开导航", exact: true }).click();
  await button.click();
}

try {
  await page.waitForSelector("[data-gallery-image-id]");
  await navigate("全部");
  await page.mouse.move(1, 1);
  await audit("gallery");
  const folderMenu = page.locator(".folder-menu-trigger").first();
  if (!(await folderMenu.isVisible()))
    await page.getByRole("button", { name: "展开导航", exact: true }).click();
  await folderMenu.click();
  await page.waitForSelector(".action-menu");
  await audit("folder-menu");
  const danger = page.getByRole("menuitem", {
    name: "删除文件夹",
    exact: true,
  });
  assert.equal(
    await danger.evaluate((n) => getComputedStyle(n).color),
    "rgb(158, 52, 52)",
  );
  await danger.hover();
  assert.equal(
    await danger.evaluate((n) => getComputedStyle(n).color),
    "rgb(158, 52, 52)",
  );
  await page.keyboard.press("Escape");
  if (await page.locator(".drawer-scrim").count())
    await page.keyboard.press("Escape");

  await page.locator("[data-gallery-image-id]").first().click();
  if (!(await page.locator(".viewer-info").count()))
    await page.getByRole("button", { name: "信息", exact: true }).click();
  await page.waitForSelector(".tag-table");
  await audit("viewer-info");
  await page.getByRole("combobox", { name: "标签类型", exact: true }).click();
  await page.waitForSelector(".select-popover");
  await audit("viewer-select");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");

  await navigate("标签");
  await audit("tags");
  await page.getByRole("button", { name: "标签字典", exact: true }).click();
  await audit("dictionary");
  await navigate("任务中心");
  await page.waitForSelector(".task-center");
  await audit("tasks");
  await navigate("设置");
  for (const [label, name] of [
    ["外观与窗口", "appearance"],
    ["图库与存储", "storage"],
    ["备份与导出", "backup"],
  ]) {
    await page.getByRole("button", { name: label, exact: true }).click();
    await audit(`settings-${name}`);
  }
  await navigate("全部");
  await page.getByRole("button", { name: "沉浸浏览", exact: true }).click();
  await page.getByRole("button", { name: "展开导航", exact: true }).click();
  const opacity = await page
    .locator(".sidebar")
    .evaluate((n) => ({
      background: getComputedStyle(n).backgroundColor,
      opacity: getComputedStyle(n).opacity,
    }));
  assert.equal(opacity.background, "rgb(250, 250, 250)");
  assert.equal(opacity.opacity, "1");
  await audit("immersive-drawer");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "退出沉浸", exact: true }).click();
  const report = { passed: true, executable, root, checks, screenshots };
  fs.writeFileSync(
    path.join(root, "logs", "native-flat-style-qa.json"),
    JSON.stringify(report, null, 2),
  );
  console.log(
    JSON.stringify({
      passed: true,
      pages: checks.map((item) => item.label),
      screenshots,
    }),
  );
} finally {
  await session.close();
}
