// Isolated synthetic fixtures only; never starts the desktop app or reads a library.
import { createServer } from "vite";
import { chromium } from "playwright";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const output = path.resolve(process.argv[2] || "");
assert.ok(
  process.argv[2] &&
    output
      .toLowerCase()
      .startsWith(
        "d:\\ai\\illutag-refactor-work\\performance-1.12.1\\output\\playwright",
      ),
);
fs.mkdirSync(output, { recursive: true });
assert.ok(
  fs
    .realpathSync(output)
    .toLowerCase()
    .startsWith("d:\\ai\\illutag-refactor-work\\performance-1.12.1\\"),
);
process.env.TEMP = process.env.TMP = output;
let server, browser;
const checks = [];
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
  const page = await browser.newPage({
    viewport: { width: 1430, height: 840 },
  });
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.route("**/*", (r) => {
    const u = new URL(r.request().url());
    return ["127.0.0.1", "localhost"].includes(u.hostname) ||
      ["data:", "blob:"].includes(u.protocol)
      ? r.continue()
      : r.abort();
  });
  async function nav(name) {
    const button = page
      .locator(".sidebar")
      .getByRole("button", { name, exact: true });
    await button.waitFor({ state: "attached" });
    if (!(await button.isVisible()))
      await page.locator('[aria-label="展开导航"]:visible').first().click();
    await button.click();
  }
  await page.goto(origin + "/?preview=1&tags=layout");
  await nav("标签");
  await page.locator(".tag-card-main").first().waitFor();
  for (const width of [1430, 1024, 768, 500]) {
    await page.setViewportSize({ width, height: 840 });
    if (width < 960) {
      const mine = await page.locator(".tag-my-tags").boundingBox(),
        menu = await page.locator(".page-nav-opener").boundingBox();
      assert.ok(
        mine.x + mine.width <= menu.x,
        "page navigation must not cover tag tools",
      );
    }
    const cards = page.locator(".tag-cover-card");
    await cards.first().hover();
    const geometry = await cards.evaluateAll((rows) =>
      rows.slice(0, 6).map((row) => {
        const rect = (selector) => {
          const b = row.querySelector(selector).getBoundingClientRect();
          return { x: b.x, right: b.right, width: b.width };
        };
        return {
          image: rect(".tag-card-image"),
          caption: rect(".tag-card-caption"),
          count: rect(".tag-card-count"),
          favorite: rect(".tag-favorite-toggle"),
          menu: rect(".tag-cover-edit"),
        };
      }),
    );
    for (const r of geometry) {
      assert.ok(
        r.image.right <= r.caption.x && r.caption.right <= r.count.x + 1,
      );
      assert.ok(
        r.count.right <= r.favorite.x + 1 && r.favorite.right <= r.menu.x + 1,
      );
      assert.ok(
        r.caption.width >= (width === 500 ? 130 : 110),
        `caption too small at ${width}: ${r.caption.width}`,
      );
      assert.ok(
        Math.abs(r.count.right - geometry[0].count.right) <= 1,
        "counts must align",
      );
    }
    assert.ok(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    );
    const scroll = page.locator(".tag-catalog-scroll");
    await scroll.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    assert.ok(
      await scroll.evaluate((el) => el.scrollTop > 0),
      "catalog scroll must work",
    );
    await scroll.evaluate((el) => {
      el.scrollTop = 0;
    });
    await page.screenshot({
      path: path.join(output, `tag-layout-${width}.png`),
    });
    checks.push({ width, alignedRows: true, scroll: true });
  }
  await page.setViewportSize({ width: 1430, height: 840 });
  const favorite = page.locator(".tag-favorite-toggle").first();
  await favorite.focus();
  assert.equal(
    await favorite.evaluate((el) => getComputedStyle(el).opacity),
    "1",
  );
  await favorite.press("Enter");
  assert.equal(await page.locator(".tag-query-chip").count(), 0);
  await page.locator(".tag-card-main").first().click();
  await page.locator(".tag-preview-masonry img").first().waitFor();
  assert.equal(await page.locator(".tag-query-chip").count(), 1);
  await page
    .getByRole("combobox", { name: "更多标签分组", exact: true })
    .click();
  const list = page.getByRole("listbox");
  await list.waitFor();
  assert.equal(
    await list.getByRole("option", { name: "全部", exact: true }).count(),
    0,
  );
  await list.getByRole("option").first().click();
  assert.equal(await page.locator(".tag-query-chip").count(), 1);
  await page.screenshot({ path: path.join(output, "tag-selected.png") });
  for (const mode of ["loading", "error"]) {
    await page.goto(origin + `/?preview=1&duplicates=refined&snapshot=${mode}`);
    await nav("重复检索");
    if (mode === "loading") {
      await page
        .getByRole("heading", { name: "正在读取检索结果", exact: true })
        .waitFor();
      assert.equal(
        await page.getByRole("button", { name: "查看", exact: true }).count(),
        0,
      );
      await page.screenshot({
        path: path.join(output, "duplicates-loading.png"),
      });
    } else {
      await page
        .getByRole("heading", { name: "检索结果暂未读取完成", exact: true })
        .waitFor();
      await page.screenshot({
        path: path.join(output, "duplicates-error.png"),
      });
      await page
        .locator(".error-dialog")
        .getByRole("button", { name: "刷新结果", exact: true })
        .click();
    }
    await page
      .getByRole("heading", { name: "整理结果", exact: true })
      .waitFor();
    checks.push({ snapshot: mode, recovery: true });
  }
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ checks }));
} finally {
  await browser?.close();
  await server?.close();
}
