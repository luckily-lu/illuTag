// Synthetic SVGs, headless Edge only. Never launches Tauri or reads library files.
import { createServer } from "vite";
import { chromium } from "playwright";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const output = path.resolve(process.argv[2] || "");
assert.ok(
  process.argv[2] &&
    path.isAbsolute(process.argv[2]) &&
    output
      .toLowerCase()
      .startsWith("d:\\ai\\illutag-refactor-work\\workbench-1.12.0\\"),
);
let ancestor = output;
while (!fs.existsSync(ancestor)) ancestor = path.dirname(ancestor);
assert.ok(
  fs
    .realpathSync(ancestor)
    .toLowerCase()
    .startsWith("d:\\ai\\illutag-refactor-work"),
);
fs.mkdirSync(output, { recursive: true });
const oldTemp = process.env.TEMP,
  oldTmp = process.env.TMP;
process.env.TEMP = output;
process.env.TMP = output;
let server, browser;
const checked = [];
try {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    server: { host: "127.0.0.1", port: 0, open: false },
  });
  await server.listen();
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
  browser = await chromium.launch({
    channel: "msedge",
    headless: true,
    downloadsPath: output,
  });
  for (const width of [1430, 768, 500]) {
    const context = await browser.newContext({
      viewport: { width: 1430, height: 840 },
    });
    await context.route("**/*", (r) => {
      const u = new URL(r.request().url());
      return ["127.0.0.1", "localhost"].includes(u.hostname) ||
        ["data:", "blob:"].includes(u.protocol)
        ? r.continue()
        : r.abort();
    });
    const page = await context.newPage(),
      errors = [];
    page.on("pageerror", (e) => errors.push(String(e)));
    async function navigate(name) {
      await page.locator(".sidebar").waitFor({ state: "attached" });
      const button = page
        .locator(".sidebar")
        .getByRole("button", { name, exact: true });
      if (!(await button.isVisible()))
        await page.locator('[aria-label="展开导航"]:visible').first().click();
      await button.click();
    }
    async function noOverflow() {
      assert.ok(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
        "page must fit viewport",
      );
    }
    try {
      await page.goto(origin + "/?preview=1&tasks=ready");
      await page.locator(".image-open").first().waitFor();
      assert.deepEqual(
        (
          await page.locator(".sidebar > .nav-row .nav-label").allTextContents()
        ).slice(0, 6),
        ["全部", "随机", "标签", "收藏", "待分类", "回收站"],
      );
      await navigate("标签");
      await page.locator(".tag-card-main").first().waitFor();
      await page
        .getByRole("button", { name: "收藏标签：风景", exact: true })
        .click();
      assert.equal(
        await page.locator(".tag-query-chip").count(),
        0,
        "bookmark must not select a condition",
      );
      await page
        .locator(".tag-category-nav")
        .getByRole("button", { name: "收藏", exact: true })
        .click();
      await page.locator(".tag-card-main").first().click();
      await page.waitForFunction(
        () =>
          document.querySelectorAll(".tag-preview-masonry img").length >= 18,
      );
      await page.setViewportSize({ width, height: 840 });
      if (
        !(await page
          .getByRole("button", { name: /查看图片：/ })
          .first()
          .isVisible())
      )
        await page.locator(".tag-panel-tabs button").last().click();
      await noOverflow();
      await page.screenshot({ path: path.join(output, `tags-${width}.png`) });
      await page
        .getByRole("button", { name: /查看图片：/ })
        .first()
        .click();
      const viewer = page.getByRole("dialog", {
        name: "大图查看器",
        exact: true,
      });
      await viewer.waitFor();
      const filename = await viewer.locator(".viewer-file-name").textContent();
      await viewer.getByRole("button", { name: "信息", exact: true }).click();
      const input = viewer.getByRole("textbox", {
        name: "新增标签",
        exact: true,
      });
      await input.fill("input-guard");
      await input.press("Delete");
      assert.equal(
        await viewer.locator(".viewer-file-name").textContent(),
        filename,
        "Delete in an input must not recycle the image",
      );
      await page.keyboard.press("Escape");
      await viewer.locator(".viewer-stage").click({ position: { x: 8, y: 8 } });
      await page.keyboard.press("Delete");
      await page.waitForFunction(
        (name) =>
          document.querySelector(".viewer-file-name")?.textContent !== name,
        filename,
      );
      assert.equal(
        await page.locator(".tag-query-chip").count(),
        1,
        "deletion must retain the tag intersection",
      );
      const nextFilename = await viewer
        .locator(".viewer-file-name")
        .textContent();
      await page.keyboard.press("Control+z");
      await page
        .getByRole("button", { name: `查看图片：${filename}`, exact: true })
        .waitFor({ state: "attached" });
      assert.equal(
        await viewer.locator(".viewer-file-name").textContent(),
        nextFilename,
        "undo must restore the result without replacing the currently viewed image",
      );
      await page.evaluate(() =>
        window.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Delete",
            repeat: true,
            bubbles: true,
          }),
        ),
      );
      assert.equal(
        await viewer.locator(".viewer-file-name").textContent(),
        nextFilename,
        "held Delete must not delete another image",
      );
      await noOverflow();
      await page.screenshot({ path: path.join(output, `viewer-${width}.png`) });
      await page.keyboard.press("Escape");
      await viewer.waitFor({ state: "detached" });
      await navigate("任务中心");
      await page.locator(".task-history-row").first().waitFor();
      assert.equal(await page.locator(".task-queue-row").count(), 2);
      await page
        .locator(".task-current")
        .getByRole("button", { name: "暂停", exact: true })
        .click();
      await page
        .getByText(
          "当前任务已暂停，等待队列也会等待。继续或停止此任务后再执行后续任务。",
          { exact: true },
        )
        .waitFor();
      await page
        .locator(".task-current")
        .getByRole("button", { name: "继续", exact: true })
        .click();
      await page
        .locator(".task-queue-row")
        .first()
        .getByRole("button", { name: "取消排队", exact: true })
        .click();
      await page.waitForFunction(
        () => document.querySelectorAll(".task-queue-row").length === 1,
      );
      await page.locator(".task-heading").scrollIntoViewIfNeeded();
      await noOverflow();
      await page.screenshot({ path: path.join(output, `tasks-${width}.png`) });
      await page.locator(".task-history-more").click();
      await page.waitForFunction(
        () => document.querySelectorAll(".task-history-row").length === 40,
      );
      await page
        .getByRole("combobox", { name: "筛选任务历史", exact: true })
        .click();
      await page.getByRole("option", { name: "已完成", exact: true }).click();
      await page.waitForFunction(() =>
        [...document.querySelectorAll(".task-history-state")].every(
          (e) => e.textContent === "已完成",
        ),
      );
      await page
        .getByRole("button", { name: "清除记录…", exact: true })
        .click();
      const clear = page.getByRole("dialog", {
        name: "清除任务历史记录",
        exact: true,
      });
      await clear.waitFor();
      assert.ok(
        await clear.getByText("此操作无法撤销。", { exact: true }).isVisible(),
      );
      await clear
        .getByRole("button", { name: "清除记录", exact: true })
        .click();
      await clear.waitFor({ state: "detached" });
      await page
        .getByText("暂无符合条件的任务记录。", { exact: true })
        .waitFor();
      assert.equal(await page.locator(".task-queue-row").count(), 1);
      assert.ok(await page.locator(".task-current h3").isVisible());
      await page.goto(origin + "/?preview=1&tasks=save");
      await navigate("任务中心");
      await page
        .getByRole("button", { name: "重试保存", exact: true })
        .waitFor();
      await noOverflow();
      await page.screenshot({
        path: path.join(output, `tasks-save-${width}.png`),
      });
      await page.getByRole("button", { name: "重试保存", exact: true }).click();
      await page
        .getByRole("button", { name: "重试保存", exact: true })
        .waitFor({ state: "detached" });
      assert.equal(
        await page
          .locator(
            '.task-history-row[data-job-id="task-000"] .task-history-state',
          )
          .textContent(),
        "已完成",
      );
      assert.deepEqual(errors, []);
      checked.push({
        width,
        deleteUndo: true,
        bookmarks: true,
        taskQueue: true,
        history: true,
      });
    } catch (e) {
      await page.screenshot({
        path: path.join(output, `failure-${width}.png`),
      });
      throw e;
    } finally {
      await context.close();
    }
  }
  console.log(JSON.stringify({ checked }));
} finally {
  await browser?.close();
  await server?.close();
  process.env.TEMP = oldTemp;
  process.env.TMP = oldTmp;
}
