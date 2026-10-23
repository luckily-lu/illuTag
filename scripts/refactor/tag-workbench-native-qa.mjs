import { launch, isolatedPath, sleep } from "./native-session.mjs";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
const [exeArg, rootArg, resources] = process.argv.slice(2);
const exe = isolatedPath(exeArg, "candidate"),
  root = isolatedPath(rootArg, "fresh QA root");
assert.ok(!fs.existsSync(root), "Fresh isolated root required");
assert.ok(path.isAbsolute(resources));
const python = path.join(
  fs.realpathSync(resources),
  "runtime/python/python.exe",
);
const report = {
  passed: false,
  syntheticOnly: true,
  checks: [],
  exeSha256: createHash("sha256").update(fs.readFileSync(exe)).digest("hex"),
};
let session, failure;
function py(code) {
  return JSON.parse(
    execFileSync(python, ["-B", "-c", code, root], {
      windowsHide: true,
      env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" },
      timeout: 30000,
    }).toString(),
  );
}
async function finish(id) {
  const end = performance.now() + 60000;
  while (performance.now() < end) {
    const job = (await session.invoke("list_jobs")).find((j) => j.id === id);
    if (
      ["completed", "partial", "failed", "interrupted", "cancelled"].includes(
        job?.state,
      )
    ) {
      assert.equal(job.state, "completed", JSON.stringify(job));
      return job;
    }
    await sleep(30);
  }
  throw Error("Isolated job timeout");
}
async function start(kind) {
  const j = await session.invoke("start_job", {
    request: {
      kind,
      thumbnails: false,
      sourceFolderId: null,
    },
  });
  return finish(j.id);
}
const fixture = String.raw`
from PIL import Image,ImageDraw
import pathlib,sys,shutil,json,random
p=pathlib.Path(sys.argv[1]).resolve();assert 'illutag-refactor-work' in [s.lower() for s in p.parts]
a=p/'assets'; random.seed(17)
for n in range(10):
 im=Image.new('RGB',(360+n*12,240+n*9),(70+n*7,140,200-n*6));d=ImageDraw.Draw(im)
 for j in range(15):
  x=random.randrange(im.width-45);y=random.randrange(im.height-45);d.ellipse((x,y,x+40,y+40),fill=tuple(random.randrange(30,230) for _ in range(3)))
 f=a/f'sample-{n:02}.png';im.save(f)
 if n<6:shutil.copyfile(f,a/f'copy-{n:02}.png')
for name in ['nested-source-with-long-name-for-alignment','short']:(a/name).mkdir()
print(json.dumps({'count':16,'bytes':sum(f.stat().st_size for f in a.glob('*.png'))}))
`;
const seed = String.raw`
import sqlite3,pathlib,sys,json
p=pathlib.Path(sys.argv[1]).resolve();assert 'illutag-refactor-work' in [s.lower() for s in p.parts]
c=sqlite3.connect(p/'data'/'illutag.sqlite');c.execute('pragma foreign_keys=on')
tags=['long_hair','blue_sky','outdoors','scenery','cloud','white_dress','beach','sunset','city']
for s in ['wd-swinv2-tagger-v3','general']+tags:c.execute('insert or ignore into storage_strings(value) values(?)',(s,))
pk=lambda s:c.execute('select string_pk from storage_strings where value=?',(s,)).fetchone()[0]
for image, in c.execute('select image_pk from images').fetchall():
 for tag in tags:c.execute('insert into storage_auto_tags values(?,?,?,?,NULL,0.95,0)',(image,pk('wd-swinv2-tagger-v3'),pk('general'),pk(tag)))
c.commit();print(json.dumps({'images':c.execute('select count(*) from images').fetchone()[0]}));c.close()
`;
try {
  session = await launch(exe, root, 9796, { resourceRoot: resources });
  await session.page.waitForSelector(".app");
  assert.equal((await session.invoke("list_jobs")).length, 0);
  report.fixture = py(fixture);
  for (const dir of [
    path.join(root, "assets"),
    path.join(root, "assets/nested-source-with-long-name-for-alignment"),
    path.join(root, "assets/short"),
  ])
    await session.invoke("add_gallery_folder_command", { folderPath: dir });
  await start("scan");
  assert.equal(py(seed).images, 16);
  await session.page.reload();
  await session.page.waitForSelector(".app");
  await session.page
    .locator(".sidebar")
    .getByRole("button", { name: "标签", exact: true })
    .click();
  await session.page.locator(".tag-cover-card").first().waitFor();
  await session.page.getByLabel("仅图库内", { exact: true }).check();
  const hair = session.page
      .locator(".tag-cover-card")
      .filter({ hasText: "long_hair" }),
    sky = session.page
      .locator(".tag-cover-card")
      .filter({ hasText: "blue_sky" });
  await hair.locator(".tag-card-main").click();
  await sky.locator(".tag-card-main").click();
  await session.page.waitForFunction(
    () => document.querySelectorAll(".tag-preview-masonry img").length === 16,
  );
  assert.equal(await session.page.locator(".tag-query-chip").count(), 2);
  for (const width of [500, 1280, 1430]) {
    await session.page.setViewportSize({ width, height: 900 });
    assert.equal(
      await session.page
        .locator(".tag-workbench")
        .evaluate((el) => el.scrollWidth > el.clientWidth),
      false,
    );
  }
  await session.page.screenshot({
    path: path.join(root, "logs/tag-workbench-A.png"),
  });
  await hair.locator(".tag-cover-edit").click();
  const editor = session.page.getByRole("dialog", { name: /设置标签封面/ });
  await editor.waitFor();
  await editor.locator(".tag-cover-choices button").nth(1).click();
  const ranges = editor.locator("input[type=range]");
  for (const [index, value] of [
    [0, "22"],
    [1, "73"],
    [2, "1.65"],
  ])
    await ranges.nth(index).evaluate((el, value) => {
      el.value = value;
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
    }, value);
  await editor.getByRole("button", { name: "固定此封面", exact: true }).click();
  await editor.waitFor({ state: "detached" });
  const pin = (
    await session.invoke("list_tag_covers_command", { tags: ["long_hair"] })
  )[0];
  assert.equal(pin.pinned, true);
  assert.deepEqual([pin.x, pin.y, pin.zoom], [22, 73, 1.65]);
  await session.page
    .getByRole("button", { name: /查看图片：/ })
    .first()
    .click();
  await session.page.locator(".viewer").waitFor();
  await session.page.locator('.viewer').getByRole('button',{name:'收藏',exact:true}).click();
  await session.page.locator('.viewer').getByRole('button',{name:'已收藏',exact:true}).waitFor();
  await session.page.keyboard.press("Escape");
  await session.page.locator(".viewer").waitFor({ state: "detached" });
  await session.page.getByRole('button',{name:/查看图片：/}).first().click();
  await session.page.locator('.viewer').getByRole('button',{name:'已收藏',exact:true}).waitFor();
  await session.page.keyboard.press('Escape');
  await session.page.locator('.viewer').waitFor({state:'detached'});
  assert.equal(await session.page.locator(".tag-query-chip").count(), 2);
  await session.page
    .getByRole("button", { name: "浏览 16 张图片", exact: true })
    .click();
  await session.page.locator(".image-tile").first().waitFor();
  await session.page
    .locator(".sidebar")
    .getByRole("button", { name: "标签", exact: true })
    .click();
  assert.equal(await session.page.locator(".tag-query-chip").count(), 2);
  report.checks.push(
    "A workbench: category browsing, two-tag intersection, real viewer return, persistent selection, fixed crop and 500/1280/1430 layout",
  );
  await session.page.getByRole("button", { name: "设置", exact: true }).click();
  await session.page
    .getByRole("button", { name: "图库与存储", exact: true })
    .click();
  const rows = session.page.locator(".source-row");
  await rows.first().waitFor();
  assert.equal(await rows.count(), 3);
  const positions = await rows.evaluateAll((rows) =>
    rows.map((row) => {
      const buttons = row.querySelectorAll("button");
      return Array.from(buttons).map(
        (button) => button.getBoundingClientRect().left,
      );
    }),
  );
  assert.ok(
    positions.every(
      (p) => Math.abs(p[0] - positions[0][0]) <= 1 && p[0] < p[1],
    ),
  );
  await rows
    .first()
    .getByRole("button", { name: "移除来源", exact: true })
    .click();
  const removal = session.page.getByRole("dialog", {
    name: "移除磁盘来源",
    exact: true,
  });
  await removal.waitFor();
  assert.ok((await removal.innerText()).includes(path.join(root, "assets")));
  await removal.getByRole("button", { name: "取消", exact: true }).click();
  assert.equal((await session.invoke("get_navigation")).folders.length, 3);
  await session.page
    .getByRole("button", { name: "外观与窗口", exact: true })
    .click();
  const toggle = session.page
    .getByText("隐藏窗口标题栏（移动到顶端显示）", { exact: true })
    .locator("..")
    .locator("input");
  await toggle.check();
  await session.page.mouse.move(650, 600);
  await sleep(250);
  assert.ok(
    await session.page
      .locator(".titlebar")
      .evaluate((el) => el.getBoundingClientRect().bottom <= 1),
  );
  const contentY = await session.page
    .locator(".workspace")
    .evaluate((el) => el.getBoundingClientRect().top);
  await session.page.locator(".titlebar-reveal").hover();
  await sleep(250);
  const rect = await session.page.locator(".titlebar").boundingBox();
  assert.ok(rect.y >= -1 && rect.height >= 43);
  assert.equal(
    await session.page
      .locator(".workspace")
      .evaluate((el) => el.getBoundingClientRect().top),
    contentY,
  );
  await session.page.screenshot({
    path: path.join(root, "logs/titlebar-revealed.png"),
  });
  await toggle.uncheck();
  let dragRequests = 0;
  session.page.on("request", (request) => {
    if (request.url().includes("window_start_dragging_command")) dragRequests++;
  });
  await session.page
    .locator(".titlebar .spacer")
    .dispatchEvent("pointerdown", {
      button: 0,
      buttons: 1,
      pointerType: "mouse",
      isPrimary: true,
      pointerId: 1,
    });
  await sleep(100);
  assert.equal(dragRequests, 1);
  await session.page
    .locator(".titlebar button")
    .first()
    .dispatchEvent("pointerdown", {
      button: 0,
      buttons: 1,
      pointerType: "mouse",
      isPrimary: true,
      pointerId: 1,
    });
  await sleep(100);
  assert.equal(dragRequests, 1);
  report.checks.push(
    "Source action alignment and cancellation; full titlebar hide/reveal without shifting content; blank-area drag request excludes buttons",
  );
  assert.equal(
    (await session.invoke("list_jobs")).filter((j) =>
      ["queued", "running", "paused", "cancelling"].includes(j.state),
    ).length,
    0,
  );
  report.passed = true;
} catch (error) {
  failure = error;
  report.error = error.message;
} finally {
  if (session) {
    await session.close();
    report.session = session.getDiagnostics();
    if (report.session.killed || report.session.exitCode !== 0) {
      report.passed = false;
      failure ||= Error("Test application did not close normally");
    }
  }
  fs.writeFileSync(
    path.join(root, "logs/tag-workbench-native-qa.json"),
    JSON.stringify(report, null, 2),
  );
  console.log(JSON.stringify(report));
}
if (failure) throw failure;
