// Uses only generated preview fixtures in headless Edge. Never starts Tauri.
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
    .split(/[\\/]/)
    .some((p) => p.toLowerCase() === "illutag-refactor-work"),
);
fs.mkdirSync(output, { recursive: true });
const oldTemp = process.env.TEMP,
  oldTmp = process.env.TMP;
process.env.TEMP = output;
process.env.TMP = output;
let server, browser;
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
  for (const width of [1430, 500]) {
    const context = await browser.newContext({
      viewport: { width: 1430, height: 840 },
    });
    try {
      let broken = true,
        injected = false;
      await context.route("**/*", async (route) => {
        const url = new URL(route.request().url());
        if (
          !["127.0.0.1", "localhost"].includes(url.hostname) &&
          !["data:", "blob:"].includes(url.protocol)
        )
          return route.abort();
        if (url.pathname === "/src/shared/ipc.ts") {
          const response = await route.fetch();
          let body = await response.text();
          const marker = "if (!isTauri()) {";
          assert.ok(body.includes(marker));
          body = body.replace(
            marker,
            'if (command === "duplicate_workflow_status" && globalThis.__fixtureBroken) throw new Error("invalid uri authority: ?");\n' +
              marker,
          );
          injected = true;
          return route.fulfill({ response, body });
        }
        return route.continue();
      });
      await context.addInitScript(() => {
        globalThis.__fixtureBroken = true;
      });
      const page = await context.newPage();
      await page.goto(`${origin}/?preview=1&duplicates=ready`);
      await page.getByRole("button", { name: "重复检索", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "重复检索操作未完成" });
      await dialog.waitFor();
      assert.ok(injected);
      await page.setViewportSize({ width, height: 840 });
      assert.ok(
        await page.evaluate(() => {
          const mask = document.querySelector(".error-dialog");
          return (
            mask?.parentElement === document.body &&
            [
              [24, 12],
              [24, 110],
              [24, 230],
              [24, 650],
              [innerWidth - 8, innerHeight - 8],
            ].every(([x, y]) => mask.contains(document.elementFromPoint(x, y)))
          );
        }),
        "error dialog must cover navigation and titlebar, not stay inside the workspace layer",
      );
      assert.ok(
        await dialog
          .getByText("暂时无法读取图库数据。请确认数据目录可访问，再刷新结果。")
          .isVisible(),
      );
      assert.equal(await dialog.locator("pre").count(), 0);
      await dialog.getByRole("button", { name: "技术详情" }).click();
      assert.ok(
        await dialog
          .locator("pre")
          .filter({ hasText: "invalid uri authority: ?" })
          .isVisible(),
      );
      assert.ok(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
      );
      await page.screenshot({
        path: path.join(output, `error-dialog-${width}.png`),
      });
      await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "detached" });
      // A later explicit user refresh can recover without starting another task.
      await page.setViewportSize({ width: 1430, height: 840 });
      await page.reload();
      await page.getByRole("button", { name: "重复检索", exact: true }).click();
      await dialog.waitFor();
      await page.setViewportSize({ width, height: 840 });
      await page.evaluate(() => {
        globalThis.__fixtureBroken = false;
      });
      broken = false;
      await dialog.getByRole("button", { name: "刷新结果" }).click();
      await dialog.waitFor({ state: "detached" });
      assert.ok(
        await page
          .getByRole("heading", { name: "已保存旧版检索结果", exact: true })
          .isVisible(),
      );
      assert.equal(broken, false);
    } finally {
      await context.close();
    }
  }
  console.log(
    "Error dialog headless QA passed at 1430/500 CSS px with synthetic fixtures only.",
  );
} finally {
  await browser?.close();
  await server?.close();
  process.env.TEMP = oldTemp;
  process.env.TMP = oldTmp;
}
