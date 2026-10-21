// Headless Edge and generated SVG fixtures. Never starts Tauri or reads a real library.
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
    .some((s) => s.toLowerCase() === "illutag-refactor-work"),
);
let ancestor = output;
while (!fs.existsSync(ancestor)) ancestor = path.dirname(ancestor);
assert.ok(
  fs
    .realpathSync(ancestor)
    .split(/[\\/]/)
    .some((s) => s.toLowerCase() === "illutag-refactor-work"),
);
fs.mkdirSync(output, { recursive: true });
const oldTemp = process.env.TEMP,
  oldTmp = process.env.TMP;
process.env.TEMP = output;
process.env.TMP = output;
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
  for (const width of [1430, 768, 500]) {
    const context = await browser.newContext({
      viewport: { width: 1430, height: 840 },
    });
    try {
      await context.route("**/*", (route) => {
        const url = new URL(route.request().url());
        return ["127.0.0.1", "localhost"].includes(url.hostname) ||
          ["data:", "blob:"].includes(url.protocol)
          ? route.continue()
          : route.abort();
      });
      const page = await context.newPage(),
        errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto(origin + "/?preview=1&duplicates=ready");
      await page
        .locator(".sidebar")
        .getByRole("button", { name: "重复检索", exact: true })
        .click();
      await page.locator(".workflow-result-row").first().waitFor();
      await page.setViewportSize({ width, height: 840 });
      assert.equal(await page.locator(".workflow-result-row").count(), 2);
      await page.evaluate(async () => {
        const { api } = await import("/src/shared/ipc.ts");
        window.__workflowCalls = [];
        // Preserve a real running/paused interval; instant-completion fixtures
        // previously missed the production review lock entirely.
        const start = api.startJob,
          jobs = api.jobs,
          control = api.controlJob;
        let pending = null;
        api.startJob = async (request) => {
          if (
            request.kind === "duplicates" &&
            request.duplicateOptions?.verificationMode === "refine"
          ) {
            pending = {
              id: "qa-refine",
              request,
              state: "running",
              stage: "复检近似候选",
              processed: 1,
              total: 100,
              failures: 0,
              errors: [],
              updatedAt: Date.now(),
            };
            return pending;
          }
          return start(request);
        };
        api.jobs = async () => (pending ? [pending] : jobs());
        api.controlJob = async (id, action) => {
          if (id !== pending?.id) return control(id, action);
          if (action === "pause") pending = { ...pending, state: "paused" };
          else if (action === "resume") {
            await start(pending.request);
            pending = null;
          } else throw Error("unexpected QA control");
        };
        for (const name of [
          "startJob",
          "prepareDuplicateWorkflow",
          "editDuplicateReview",
        ]) {
          const original = api[name];
          api[name] = async (request) => {
            window.__workflowCalls.push({
              name,
              request: JSON.parse(JSON.stringify(request)),
            });
            return original(request);
          };
        }
      });
      // First-stage exact review has real pageable groups, and candidate actions cannot clean.
      await page
        .getByRole("button", { name: "查看完全相同图", exact: true })
        .click();
      await page.locator(".review-image").first().waitFor();
      assert.equal(await page.locator(".review-image").count(), 50);
      await page.getByRole("button", { name: /继续显示本组图片/ }).click();
      await page.waitForFunction(
        () => document.querySelectorAll(".review-image").length === 80,
      );
      await page.getByRole("button", { name: "返回结果", exact: true }).click();
      await page
        .getByRole("button", { name: "查看近似候选", exact: true })
        .click();
      await page.locator(".review-image").first().waitFor();
      assert.equal(
        await page
          .locator(".review-image-actions .danger:not(:disabled)")
          .count(),
        0,
      );
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
      await page.getByRole("button", { name: "返回结果", exact: true }).click();
      await page
        .getByRole("button", { name: "复检近似图片", exact: true })
        .click();
      await page
        .locator(".workflow-progress")
        .getByRole("button", { name: "暂停", exact: true })
        .waitFor();
      await page
        .getByRole("button", { name: "查看完全相同图", exact: true })
        .click();
      await page.locator(".review-image").first().waitFor();
      await page
        .locator(".review-hint")
        .filter({ hasText: "近似复检在后台进行" })
        .waitFor();
      await page.getByRole("button", { name: "下一组", exact: true }).click();
      await page.waitForFunction(() =>
        document
          .querySelector(".review-group-heading")
          ?.textContent.includes("第 2 组"),
      );
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
      await page.locator(".review-image-open").nth(1).click();
      await page.getByRole("dialog", { name: "查看分组原图" }).waitFor();
      const overlay = await page.evaluate(() => {
        const root = document.querySelector(".duplicate-review-viewer"),
          r = root.getBoundingClientRect();
        return {
          body: root.parentElement === document.body,
          x: r.x,
          y: r.y,
          width: r.width,
          height: r.height,
          covers: [
            [12, 12],
            [24, 110],
            [24, 420],
            [innerWidth - 12, innerHeight - 12],
          ].every(([x, y]) => root.contains(document.elementFromPoint(x, y))),
        };
      });
      assert.deepEqual(overlay, {
        body: true,
        x: 0,
        y: 0,
        width,
        height: 840,
        covers: true,
      });
      await page.keyboard.press("Control+Enter");
      assert.equal(
        (await page.evaluate(() => window.__workflowCalls)).filter(
          (c) =>
            c.name === "editDuplicateReview" && c.request.action === "confirm",
        ).length,
        0,
      );
      await page.screenshot({
        path: path.join(output, `review-viewer-${width}.png`),
      });
      await page.keyboard.press("Escape");
      await page
        .getByRole("dialog", { name: "查看分组原图" })
        .waitFor({ state: "hidden" });
      assert.equal(
        await page.evaluate(() =>
          document.activeElement?.classList.contains("review-image-open"),
        ),
        true,
      );
      await page
        .locator(".review-image")
        .nth(2)
        .getByRole("button", { name: "设为保留基准", exact: true })
        .click();
      await page
        .locator(".review-image")
        .nth(2)
        .getByText("保留基准", { exact: true })
        .first()
        .waitFor();
      await page.keyboard.press("Control+Enter");
      await page.waitForFunction(() =>
        document
          .querySelector(".review-group-heading")
          ?.textContent.includes("第 3 组"),
      );
      await page.getByRole("button", { name: "返回结果", exact: true }).click();
      await page
        .locator(".workflow-progress")
        .getByRole("button", { name: "暂停", exact: true })
        .click();
      await page
        .locator(".workflow-progress")
        .getByRole("button", { name: "继续", exact: true })
        .waitFor();
      await page
        .getByRole("button", { name: "查看近似候选", exact: true })
        .click();
      await page.locator(".review-image").first().waitFor();
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
      assert.equal(
        await page
          .locator(".review-image-actions .danger:not(:disabled)")
          .count(),
        0,
      );
      await page.getByRole("button", { name: "返回结果", exact: true }).click();
      await page
        .locator(".workflow-progress")
        .getByRole("button", { name: "继续", exact: true })
        .click();
      await page.locator('[data-category="near"]').waitFor();
      assert.equal(await page.locator(".workflow-result-row").count(), 3);
      const calls = await page.evaluate(() => window.__workflowCalls);
      const starts = calls.filter((call) => call.name === "startJob");
      assert.equal(starts.length, 1);
      assert.equal(
        starts[0].request.duplicateOptions.verificationMode,
        "refine",
      );
      assert.equal(starts[0].request.duplicateOptions.regroupOnly, true);
      await page
        .getByRole("button", { name: "查看近似图", exact: true })
        .click();
      await page.locator(".review-image").first().waitFor();
      assert.ok((await page.locator(".review-image").count()) > 2);
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
      await page
        .locator(".review-image")
        .nth(2)
        .getByRole("button", { name: "设为保留基准", exact: true })
        .click();
      await page
        .locator(".review-image")
        .nth(2)
        .getByText("保留基准", { exact: true })
        .first()
        .waitFor();
      await page.locator(".review-image-open").nth(1).click();
      await page.getByRole("dialog", { name: "查看分组原图" }).waitFor();
      const imageRect = await page
        .locator(".duplicate-review-viewer .duplicate-pan-stage img")
        .boundingBox();
      const stageRect = await page
        .locator(".duplicate-review-viewer .duplicate-pan-stage")
        .boundingBox();
      assert.ok(
        imageRect &&
          stageRect &&
          imageRect.width <= stageRect.width + 1 &&
          imageRect.height <= stageRect.height + 1,
      );
      await page
        .locator(".duplicate-review-viewer .duplicate-pan-stage")
        .hover();
      await page.mouse.wheel(0, -100);
      assert.match(
        await page
          .locator(".duplicate-review-viewer .duplicate-pan-stage img")
          .getAttribute("style"),
        /scale\(1.2\)/,
      );
      await page.keyboard.press("Escape");
      await page
        .getByRole("dialog", { name: "查看分组原图" })
        .waitFor({ state: "hidden" });
      await page.keyboard.press("Control+Enter");
      await page.waitForFunction(() =>
        document
          .querySelector(".review-group-heading")
          ?.textContent.includes("第 2 组"),
      );
      await page.keyboard.press("ArrowLeft");
      await page.waitForFunction(() =>
        document
          .querySelector(".review-group-heading")
          ?.textContent.includes("第 1 组"),
      );
      assert.equal(
        await page.evaluate(
          () => document.documentElement.scrollWidth > innerWidth,
        ),
        false,
      );
      if (width === 1430)
        await page.screenshot({
          path: path.join(output, "workflow-review-synthetic.png"),
        });
      await page.getByRole("button", { name: "返回结果", exact: true }).click();
      await page
        .getByRole("button", { name: "查看类似图", exact: true })
        .click();
      await page.locator(".review-image").first().waitFor();
      await page
        .locator(".review-image")
        .nth(1)
        .getByRole("button", { name: "待清理", exact: true })
        .click();
      await page
        .locator(".review-image")
        .nth(1)
        .getByText("人工待清理", { exact: true })
        .waitFor();
      await page.getByRole("button", { name: "返回结果", exact: true }).click();
      if (width === 1430)
        await page.screenshot({
          path: path.join(output, "workflow-overview-synthetic.png"),
        });
      await page
        .getByRole("button", { name: "移入应用回收站", exact: true })
        .click();
      await page.locator(".workflow-completed").waitFor();
      assert.equal(
        await page.locator(".workflow-cleanup-button").isDisabled(),
        true,
      );
      const prepares = (
        await page.evaluate(() => window.__workflowCalls)
      ).filter((call) => call.name === "prepareDuplicateWorkflow");
      assert.equal(prepares.length, 1);
      assert.ok(prepares[0].request.revision);
      await page
        .getByRole("button", { name: "查看近似图", exact: true })
        .click();
      await page
        .getByRole("button", { name: "撤销本次整理并继续审查", exact: true })
        .waitFor();
      await page
        .getByRole("button", { name: "撤销本次整理并继续审查", exact: true })
        .click();
      await page
        .getByRole("button", { name: "撤销本次整理并继续审查", exact: true })
        .waitFor({ state: "hidden" });
      await page.getByRole("button", { name: "返回结果", exact: true }).click();
      assert.equal(
        await page.locator(".workflow-cleanup-button").isEnabled(),
        true,
      );
      assert.equal(
        await page.evaluate(
          () => document.documentElement.scrollWidth > innerWidth,
        ),
        false,
      );
      assert.deepEqual(errors, []);
      checks.push(
        `${width}px: fixed steps; real group/member pagination; protect/keeper/manual-clean; original zoom; keyboard; combined cleanup; automatic refresh; review undo; no horizontal overflow`,
      );
    } finally {
      await context.close();
    }
  }
  console.log(
    JSON.stringify({
      passed: true,
      headless: true,
      syntheticOnly: true,
      productionLaunched: false,
      checks,
    }),
  );
} finally {
  await browser?.close();
  await server?.close();
  if (oldTemp === undefined) delete process.env.TEMP;
  else process.env.TEMP = oldTemp;
  if (oldTmp === undefined) delete process.env.TMP;
  else process.env.TMP = oldTmp;
}
