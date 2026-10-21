// Headless only. Generated SVG fixtures, no native application or library files.
import { createServer } from "vite";
import { chromium } from "playwright";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const output = path.resolve(process.argv[2] || "");
assert.ok(process.argv[2] && path.isAbsolute(process.argv[2]));
assert.ok(
  output
    .toLowerCase()
    .startsWith("d:\\ai\\illutag-refactor-work\\evidence-1.11.0\\"),
);
let parent = output;
while (!fs.existsSync(parent)) parent = path.dirname(parent);
assert.ok(
  fs
    .realpathSync(parent)
    .toLowerCase()
    .startsWith("d:\\ai\\illutag-refactor-work"),
);
fs.mkdirSync(output, { recursive: true });
const previousTemp = process.env.TEMP,
  previousTmp = process.env.TMP;
process.env.TEMP = output;
process.env.TMP = output;
let server, browser;
const passed = [];
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
    await context.route("**/*", (route) => {
      const url = new URL(route.request().url());
      return ["127.0.0.1", "localhost"].includes(url.hostname) ||
        ["data:", "blob:"].includes(url.protocol)
        ? route.continue()
        : route.abort();
    });
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (e) => errors.push(String(e)));
    try {
      await page.goto(origin + "/?preview=1&duplicates=ready");
      await page.getByRole("button", { name: "重复检索", exact: true }).click();
      await page.getByText("已保存旧版检索结果", { exact: true }).waitFor();
      await page.evaluate(async () => {
        const { api } = await import("/src/shared/ipc.ts");
        window.__evidenceStarts = [];
        let pending = null;
        const start = api.startJob,
          jobs = api.jobs,
          control = api.controlJob;
        api.startJob = async (request) => {
          if (request.kind !== "duplicates") return start(request);
          window.__evidenceStarts.push(request);
          pending = {
            id: "evidence-test",
            request,
            state: "running",
            stage: "验证候选原图（比较关系）",
            processed: 7,
            total: 0,
            failures: 0,
            errors: [],
            updatedAt: Date.now(),
          };
          return pending;
        };
        api.jobs = async () => (pending ? [pending] : jobs());
        api.controlJob = async (id, action) => {
          if (id !== pending?.id) return control(id, action);
          if (action === "pause") pending = { ...pending, state: "paused" };
          else if (action === "resume") {
            await start(pending.request);
            pending = null;
          } else if (action === "cancel") {
            pending = { ...pending, state: "cancelled" };
          }
        };
      });
      await page
        .getByRole("button", { name: "使用新版验证", exact: true })
        .click();
      await page
        .locator(".workflow-progress")
        .getByRole("button", { name: "暂停", exact: true })
        .waitFor();
      await page
        .locator(".workflow-progress")
        .getByRole("button", { name: "暂停", exact: true })
        .click();
      await page
        .locator(".workflow-progress")
        .getByRole("button", { name: "继续", exact: true })
        .waitFor();
      await page
        .locator(".workflow-progress")
        .getByRole("button", { name: "继续", exact: true })
        .click();
      await page.locator('[data-category="recycle"]').waitFor();
      assert.equal(await page.locator(".workflow-result-row").count(), 2);
      assert.equal(await page.locator(".workflow-steps").count(), 0);
      const starts = await page.evaluate(() => window.__evidenceStarts);
      assert.equal(starts.length, 1);
      assert.equal(starts[0].duplicateOptions.verificationMode, "organize");
      await page.setViewportSize({ width, height: 840 });
      assert.ok(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
      );
      await page.screenshot({
        path: path.join(output, `evidence-overview-${width}.png`),
      });
      await page
        .getByRole("button", { name: "查看可回收", exact: true })
        .click();
      await page.locator(".review-image").first().waitFor();
      await page.locator(".review-image-open").nth(1).click();
      const dialog = page.getByRole("dialog", { name: "查看分组原图" });
      await dialog.waitFor();
      assert.ok(
        await page.evaluate(() => {
          const root = document.querySelector(".duplicate-review-viewer");
          return (
            root.parentElement === document.body &&
            [
              [24, 100],
              [innerWidth - 8, innerHeight - 8],
            ].every(([x, y]) => root.contains(document.elementFromPoint(x, y)))
          );
        }),
      );
      await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "detached" });
      await page
        .locator(".review-image")
        .nth(1)
        .getByRole("button", { name: "保留", exact: true })
        .click();
      await page
        .locator(".review-image")
        .nth(1)
        .getByText("人工保留", { exact: true })
        .waitFor();
      await page.screenshot({
        path: path.join(output, `evidence-review-${width}.png`),
      });
      await page.getByRole("button", { name: "返回结果", exact: true }).click();
      await page.locator(".workflow-cleanup-button").click();
      await page
        .getByRole("button", { name: "撤销本次整理", exact: true })
        .waitFor();
      await page
        .getByRole("button", { name: "撤销本次整理", exact: true })
        .click();
      await page.locator('[data-category="recycle"] strong').last().waitFor();
      assert.deepEqual(errors, []);
      passed.push({ width, oneJob: true, states: 2, review: true, undo: true });
    } finally {
      await context.close();
    }
  }
  console.log(JSON.stringify({ passed }));
} finally {
  await browser?.close();
  await server?.close();
  if (previousTemp === undefined) delete process.env.TEMP;
  else process.env.TEMP = previousTemp;
  if (previousTmp === undefined) delete process.env.TMP;
  else process.env.TMP = previousTmp;
}
