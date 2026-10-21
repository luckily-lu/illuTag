import { launch, isolatedPath, sleep } from "./native-session.mjs";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";

// EXE FRESH_ROOT RESOURCE_ROOT. Only generated files under the isolated QA fence are used.
const [exeArg, rootArg, resourceArg, ...extra] = process.argv.slice(2);
assert.equal(extra.length, 0, "Unexpected arguments");
const fence = path.resolve("D:/Ai/illuTag-refactor-work");
function fenced(value, label) {
  const target = isolatedPath(value, label);
  assert.ok(
    target.toLowerCase().startsWith((fence + path.sep).toLowerCase()),
    `${label} escapes QA fence`,
  );
  let ancestor = target;
  while (!fs.existsSync(ancestor)) ancestor = path.dirname(ancestor);
  const real = fs.realpathSync(ancestor).toLowerCase();
  assert.ok(
    real === fence.toLowerCase() ||
      real.startsWith((fence + path.sep).toLowerCase()),
    `${label} resolves outside QA fence`,
  );
  return target;
}
const executable = fenced(exeArg, "candidate"),
  root = fenced(rootArg, "fresh QA root");
assert.equal(path.dirname(root).toLowerCase(), fence.toLowerCase());
assert.match(path.basename(root), /^recycle-1\.6\.0-native-[\w.-]+$/i);
assert.ok(!fs.existsSync(root), "QA root must be fresh");
assert.ok(
  resourceArg && path.isAbsolute(resourceArg),
  "Explicit absolute resource root required",
);
const resourceRoot = fs.realpathSync(resourceArg),
  python = path.join(resourceRoot, "runtime", "python", "python.exe");
assert.ok(
  fs.statSync(executable).isFile() && fs.statSync(python).isFile(),
  "Candidate/runtime missing",
);
assert.notEqual(
  process.env.ILLUTAG_READ_ONLY,
  "1",
  "QA requires its own writable schema 3 database",
);

const data = path.join(root, "data"),
  assets = path.join(root, "assets"),
  logs = path.join(root, "logs"),
  temp = path.join(root, "temp");
for (const p of [data, assets, logs, temp])
  fs.mkdirSync(fenced(p, "output"), { recursive: true });
assert.ok(
  !fs.existsSync(path.join(data, "illutag.sqlite")),
  "Fresh database required",
);
fs.writeFileSync(
  path.join(data, "window-state.json"),
  JSON.stringify({
    x: 24,
    y: 24,
    width: 1312,
    height: 920,
    isMaximized: false,
    isFullscreen: false,
  }),
);
const report = {
  passed: false,
  syntheticOnly: true,
  exeSha256: createHash("sha256")
    .update(fs.readFileSync(executable))
    .digest("hex"),
  startedAt: new Date().toISOString(),
  checks: [],
  jobs: [],
  screenshots: [],
  sessions: [],
  fixture: null,
};
const began = performance.now();
let session, failure;
function py(code, args = []) {
  return JSON.parse(
    execFileSync(python, ["-B", "-c", code, root, ...args], {
      cwd: root,
      windowsHide: true,
      timeout: 120000,
      maxBuffer: 2 * 1024 * 1024,
      env: {
        ...process.env,
        PYTHONDONTWRITEBYTECODE: "1",
        PYTHONPYCACHEPREFIX: path.join(root, "pycache"),
        TEMP: temp,
        TMP: temp,
      },
    })
      .toString()
      .trim(),
  );
}
const makeFixtures = String.raw`
from PIL import Image,ImageDraw,PngImagePlugin
import pathlib,sys,json
r=pathlib.Path(sys.argv[1]).resolve(); assert r.parent==pathlib.Path('D:/Ai/illuTag-refactor-work').resolve() and r.name.startswith('recycle-1.6.0-native-')
p=r/'assets'; im=Image.new('RGB',(48,40),(38,76,112)); d=ImageDraw.Draw(im); d.ellipse((4,5,25,26),fill=(234,188,64)); d.polygon([(2,38),(29,10),(46,38)],fill=(63,153,98)); d.rectangle((31,4,44,13),fill=(195,74,86))
for i in range(5007):
    meta=PngImagePlugin.PngInfo(); meta.add_text('synthetic-copy',str(i))
    im.save(p/f'recycle-{i:05d}.png',pnginfo=meta)
files=list(p.iterdir()); size=sum(x.stat().st_size for x in files); assert len(files)==5007 and size<12*1024*1024
print(json.dumps({'count':len(files),'bytes':size}))
`;
const seedTrashed = String.raw`
import pathlib,sqlite3,sys,json
r=pathlib.Path(sys.argv[1]).resolve(); db=sqlite3.connect(r/'data/illutag.sqlite'); db.execute('PRAGMA foreign_keys=ON')
assert db.execute('PRAGMA user_version').fetchone()[0]==3
rows=db.execute("SELECT image_pk,id,file_name FROM images WHERE source='library' ORDER BY image_pk").fetchall(); assert len(rows)==5007
db.execute('BEGIN IMMEDIATE'); db.execute('UPDATE images SET trashed=1,is_favorite=1 WHERE source=\'library\'')
db.executemany("INSERT INTO storage_image_user_custom_tags(image_id,tag_text,created_at,updated_at) VALUES(?, 'qa-preserve', 1, 1)",[(x[0],) for x in rows]); db.commit(); db.close()
print(json.dumps({'seeded':len(rows),'trashed':len(rows),'favoriteAndTag':len(rows)}))
`;
const inspect = String.raw`
import pathlib,sqlite3,sys,json
r=pathlib.Path(sys.argv[1]).resolve(); c=sqlite3.connect('file:'+str(r/'data/illutag.sqlite')+'?mode=ro',uri=True)
state=c.execute("SELECT COUNT(*),SUM(trashed),SUM(CASE WHEN is_favorite=1 THEN 1 ELSE 0 END),SUM(CASE WHEN EXISTS(SELECT 1 FROM storage_image_user_custom_tags t WHERE t.image_id=images.image_pk AND t.tag_text='qa-preserve') THEN 1 ELSE 0 END) FROM images WHERE source='library'").fetchone()
def flags(ident):
    return c.execute('SELECT trashed,is_favorite,EXISTS(SELECT 1 FROM storage_image_user_custom_tags t WHERE t.image_id=images.image_pk AND t.tag_text=\'qa-preserve\') FROM images WHERE id=?',(ident,)).fetchone() if ident else None
excluded=sys.argv[2] if len(sys.argv)>2 else None; restored=sys.argv[3] if len(sys.argv)>3 else None
excluded_flags=flags(excluded); restored_flags=flags(restored)
c.close(); print(json.dumps({'images':state[0],'trashed':state[1] or 0,'favorites':state[2] or 0,'tagged':state[3] or 0,'excluded':excluded_flags,'restored':restored_flags}))
`;
const query = (scope = "all", extra = {}) => ({
  scope,
  folderId: null,
  unclassifiedOnlyParentFolderId: null,
  tags: [],
  englishQuery: "",
  fileName: "",
  naturalLanguage: "",
  offset: 0,
  limit: 5000,
  randomSeed: null,
  ...extra,
});
async function waitJob(kind, timeout = 900000, excludedJobIds = []) {
  const until = performance.now() + timeout;
  while (performance.now() < until) {
    const jobs = await session.invoke("list_jobs");
    const job = jobs
      .filter((j) => j.request.kind === kind && !excludedJobIds.includes(j.id))
      .sort((a, b) => b.updatedAt - a.updatedAt)[0];
    if (
      job &&
      ["completed", "partial", "cancelled", "interrupted", "failed"].includes(
        job.state,
      )
    ) {
      report.jobs.push({
        id: job.id,
        kind,
        state: job.state,
        processed: job.processed,
        total: job.total,
        operationId: job.request.recycleOperationId ?? null,
      });
      return job;
    }
    await sleep(75);
  }
  throw Error(`${kind} task timed out`);
}
async function waitForJob(kind, timeout = 30000) {
  const until = performance.now() + timeout;
  while (performance.now() < until) {
    const jobs = await session.invoke("list_jobs");
    const job = jobs
      .filter((j) => j.request.kind === kind)
      .sort((a, b) => b.updatedAt - a.updatedAt)[0];
    if (job) return job;
    await sleep(10);
  }
  throw Error(`${kind} task was not created`);
}
async function snap(name) {
  const dest = fenced(path.join(logs, name), "screenshot");
  await session.page.screenshot({ path: dest });
  report.screenshots.push(dest);
}
async function closeSession() {
  const closing = session;
  if (!closing) return;
  await closing.close();
  const diagnostic = closing.getDiagnostics();
  report.sessions.push(diagnostic);
  assert.equal(diagnostic.killed, false);
  assert.equal(diagnostic.exitCode, 0);
  session = null;
}
async function openTrash() {
  await session.page
    .getByRole("navigation", { name: "图库导航" })
    .getByRole("button", { name: "回收站", exact: true })
    .click();
  await session.page.locator(".recycle-toolbar").waitFor();
}
async function setFileFilter(name) {
  await session.page
    .getByRole("button", { name: "文件名", exact: true })
    .click();
  const input = session.page.getByRole("textbox", { name: "文件名搜索" });
  await input.fill(name);
  await input.press("Enter");
  await session.page.waitForFunction(
    (expected) =>
      document.querySelector(".result-count")?.textContent?.trim() === expected,
    "1 张",
    { timeout: 30000 },
  );
}
async function waitRecycleDone(operationId, expectedCount) {
  const status = await session.invoke("recycle_operation_status", {
    operationId,
  });
  const until = performance.now() + 900000;
  let current = status;
  while (
    performance.now() < until &&
    !["completed", "partial", "interrupted"].includes(current.state)
  ) {
    await sleep(60);
    current = await session.invoke("recycle_operation_status", { operationId });
  }
  assert.equal(current.state, "completed", JSON.stringify(current));
  assert.equal(current.succeeded, expectedCount);
  assert.equal(current.failed, 0);
  assert.equal(current.skipped, 0);
  const page = await session.invoke("recycle_operation_results", {
    query: { operationId, cursor: null, limit: 20 },
  });
  assert.equal(page.items.length, 0);
  return current;
}

try {
  report.fixture = py(makeFixtures);
  session = await launch(executable, root, 9816, { resourceRoot });
  await session.page.waitForSelector(".app", { timeout: 30000 });
  const info = await session.invoke("data_directory_info_command");
  assert.equal(
    path.resolve(info.dataDir.replace(/^\\\?\\/, "")).toLowerCase(),
    data.toLowerCase(),
  );
  assert.equal(info.usingFallback, false);
  await session.invoke("add_gallery_folder_command", { folderPath: assets });
  const scan = await session.invoke("start_job", {
    request: {
      kind: "scan",
      thumbnails: false,
      sourceFolderId: null,
      duplicateOptions: null,
      duplicateOperationId: null,
      recycleOperationId: null,
    },
  });
  const scanDone = await waitJob("scan");
  assert.equal(scanDone.state, "completed");
  report.checks.push(
    "5007 isolated abstract-copy images scanned into fresh schema 3",
  );
  await closeSession();

  report.seed = py(seedTrashed);
  session = await launch(executable, root, 9817, { resourceRoot });
  await session.page.waitForSelector(".app");
  await openTrash();
  await session.page.waitForFunction(
    () =>
      document.querySelector(".result-count")?.textContent?.includes("5,007"),
    { timeout: 30000 },
  );
  assert.equal(
    (await session.invoke("query_gallery", { query: query("trash") }))
      .totalImageCount,
    5007,
  );
  const toolbar = session.page.locator(".recycle-toolbar");
  const toolbarText = await toolbar.innerText();
  for (const label of ["收藏", "分类", "移动", "标签"])
    assert.ok(
      !toolbarText.includes(label),
      `Editing action leaked into recycle toolbar: ${label}`,
    );
  assert.equal(await session.page.locator(".tile-favorite").count(), 0);
  await session.page.locator(".image-open").first().click();
  await session.page.locator(".viewer").waitFor();
  assert.equal(
    await session.page
      .locator(".viewer-toolbar")
      .getByRole("button", { name: /收藏/ })
      .count(),
    0,
  );
  await session.page.getByRole("button", { name: "信息", exact: true }).click();
  assert.equal(
    await session.page.getByRole("heading", { name: "虚拟分类" }).count(),
    0,
  );
  assert.equal(
    await session.page.getByRole("heading", { name: "手动标签" }).count(),
    0,
  );
  await session.page.keyboard.press("Escape");
  await session.page.locator(".viewer").waitFor({ state: "detached" });
  await session.page.setViewportSize({ width: 500, height: 900 });
  await snap("recycle-toolbar-500-normal.png");
  await session.page.setViewportSize({ width: 1280, height: 900 });
  await snap("recycle-toolbar-1280-normal.png");

  await session.page
    .getByRole("button", { name: "选择图片", exact: true })
    .click();
  await session.page
    .getByRole("button", { name: /全选当前结果（5,007 张）/ })
    .click();
  await session.page.waitForFunction(
    () =>
      document
        .querySelector(".recycle-summary")
        ?.textContent?.replace(/,/g, "").includes("5007"),
    { timeout: 30000 },
  );
  const allQuery = await session.invoke("query_gallery", {
    query: query("trash"),
  });
  assert.equal(allQuery.images.length, 5000);
  assert.equal(allQuery.totalImageCount, 5007);
  const firstTile = session.page.locator(".image-tile").first();
  const excludedId = await firstTile.getAttribute("data-gallery-image-id");
  assert.ok(excludedId);
  await firstTile.locator(".image-open").click();
  await session.page.waitForFunction(
    () =>
      document
        .querySelector(".recycle-summary")
        ?.textContent?.replace(/,/g, "").includes("5006"),
    { timeout: 10000 },
  );
  await session.page.setViewportSize({ width: 500, height: 900 });
  await snap("recycle-toolbar-500-selected.png");
  await session.page.setViewportSize({ width: 1280, height: 900 });
  await snap("recycle-toolbar-1280-selected.png");
  const excludedFile = allQuery.images.find(
    (i) => i.id === excludedId,
  )?.fileName;
  assert.ok(excludedFile);
  await session.page
    .getByRole("button", { name: "还原所选", exact: true })
    .click();
  const restoreDialog = session.page.getByRole("dialog", { name: "还原图片" });
  await restoreDialog.waitFor();
  assert.match(await restoreDialog.innerText(), /5,006 张/);
  await session.page
    .getByRole("button", { name: "确认还原", exact: true })
    .click();
  const restoreJob = await waitForJob("recycleRestore");
  const operationId = restoreJob.request.recycleOperationId;
  assert.ok(operationId);
  await session
    .invoke("control_job", { id: restoreJob.id, action: "cancel" })
    .catch(() => {});
  const cancelled = await waitJob("recycleRestore");
  let resumedSameBatch = false;
  if (["cancelled", "interrupted", "partial"].includes(cancelled.state)) {
    const resumed = await session.invoke("apply_recycle_action", {
      request: {
        selectionId: operationId,
        action: "restore",
        confirmed: false,
      },
    });
    assert.equal(
      resumed.request.recycleOperationId,
      operationId,
      "Resume must retain the same recycle selection",
    );
    const completed = await waitJob("recycleRestore");
    assert.equal(completed.state, "completed");
    resumedSameBatch = true;
  } else
    assert.equal(cancelled.state, "completed", "Unexpected recycle task state");
  await waitRecycleDone(operationId, 5006);
  const restoredId = allQuery.images.find((i) => i.id !== excludedId)?.id;
  assert.ok(restoredId);
  const afterRestore = py(inspect, [excludedId, restoredId]);
  assert.equal(afterRestore.images, 5007);
  assert.equal(afterRestore.trashed, 1);
  assert.equal(afterRestore.favorites, 5007);
  assert.equal(afterRestore.tagged, 5007);
  assert.deepEqual(
    afterRestore.excluded,
    [1, 1, 1],
    "Excluded file must remain trashed and restored files retain favorite/custom tag",
  );
  assert.deepEqual(afterRestore.restored, [0, 1, 1]);
  report.checks.push(
    "Frozen selection covered 5007 results beyond the 5000-row page; one exclusion remained trashed while 5006 restored with metadata intact",
  );
  report.checks.push(
    resumedSameBatch
      ? "Recycle status/results verified; interrupted batch resumed with the same operation ID"
      : "Recycle status/results verified; completion won the cancel race and the operation completed once",
  );

  const active = await session.invoke("query_gallery", {
    query: query("all", { limit: 3 }),
  });
  assert.ok(active.images.length >= 2);
  const trashed = await session.invoke("mutate_images", {
    request: {
      action: "trash",
      imageIds: active.images.slice(0, 2).map((i) => i.id),
      folderId: null,
      favorite: null,
      customTags: [],
      supplementTags: [],
    },
  });
  assert.equal(trashed.successfulIds.length, 2);
  const trashNow = await session.invoke("query_gallery", {
    query: query("trash"),
  });
  assert.equal(trashNow.totalImageCount, 3);
  const filteredFile = trashNow.images.find(
    (i) => i.id !== excludedId,
  )?.fileName;
  assert.ok(filteredFile);
  await setFileFilter(filteredFile);
  assert.equal(
    (
      await session.invoke("query_gallery", {
        query: query("trash", { fileName: filteredFile }),
      })
    ).totalImageCount,
    1,
  );
  await session.page.getByRole("button", { name: /清空应用回收站/ }).click();
  const clearDialog = session.page.getByRole("dialog", {
    name: "清空应用回收站",
  });
  await clearDialog.waitFor();
  assert.match(await clearDialog.innerText(), /全部回收项：3 张/);
  await snap("recycle-clear-confirm-filtered.png");
  const jobsBeforeCancel = (await session.invoke("list_jobs")).length;
  await clearDialog.getByRole("button", { name: "取消", exact: true }).click();
  assert.equal((await session.invoke("list_jobs")).length, jobsBeforeCancel);
  assert.equal(
    (await session.invoke("query_gallery", { query: query("trash") }))
      .totalImageCount,
    3,
  );
  report.checks.push(
    "Filtered clear confirmation reported all 3 trash items; cancellation created no task and left them intact",
  );

  const beforeFilteredJobs = (await session.invoke("list_jobs")).map(j => j.id);
  await session.page.getByRole("button", { name: /还原当前结果/ }).click();
  const filteredRestore = session.page.getByRole("dialog", {
    name: "还原图片",
  });
  await filteredRestore.waitFor();
  assert.match(await filteredRestore.innerText(), /共 1 张/);
  await session.page
    .getByRole("button", { name: "确认还原", exact: true })
    .click();
  const filteredJob = await waitJob("recycleRestore", 30000, beforeFilteredJobs);
  assert.equal(filteredJob.state, "completed");
  await waitRecycleDone(filteredJob.request.recycleOperationId, 1);
  assert.equal(
    (await session.invoke("query_gallery", { query: query("trash") }))
      .totalImageCount,
    2,
  );
  report.checks.push(
    "Filtered restore affected exactly the single matching item",
  );

  await session.page
    .getByRole("button", { name: "清空条件", exact: true })
    .click();
  await openTrash();
  await session.page
    .getByRole("button", { name: "选择图片", exact: true })
    .click();
  const remainingTrash = await session.invoke("query_gallery", {
    query: query("trash", { limit: 10 }),
  });
  const removedFiles = remainingTrash.images.map((i) => i.fileName);
  assert.equal(removedFiles.length, 2);
  await session.page
    .getByRole("button", { name: /全选当前结果（2 张）/ })
    .click();
  await session.page
    .getByRole("button", { name: /移入 Windows 回收站/ })
    .click();
  const danger = session.page.getByRole("dialog", {
    name: "移入 Windows 回收站",
  });
  await danger.waitFor();
  assert.match(await danger.innerText(), /2 张/);
  assert.match(await danger.innerText(), /不保证立即释放磁盘空间/);
  await snap("recycle-system-trash-confirm.png");
  await session.page
    .getByRole("button", { name: "确认移入 Windows 回收站", exact: true })
    .click();
  const deleteJob = await waitJob("recycleDelete");
  assert.equal(deleteJob.state, "completed");
  await waitRecycleDone(deleteJob.request.recycleOperationId, 2);
  const finalState = py(inspect, [excludedId, restoredId]);
  assert.equal(finalState.images, 5005);
  assert.equal(finalState.trashed, 0);
  assert.equal(finalState.favorites, 5005);
  assert.equal(finalState.tagged, 5005);
  assert.equal(
    fs.existsSync(path.join(assets, excludedFile)),
    false,
    "Previously excluded fixture must be removed by the final two-item system-trash action",
  );
  assert.ok(removedFiles.every((fileName) => !fs.existsSync(path.join(assets, fileName))));
  assert.equal(fs.readdirSync(assets).length, 5005);
  report.checks.push(
    "Only two generated files entered the Windows Recycle Bin; all remaining app trash items restored",
  );
  report.passed = true;
} catch (error) {
  failure = error;
  report.error = error?.stack || String(error);
} finally {
  if (session) {
    try {
      await closeSession();
    } catch (error) {
      failure ||= error;
      report.passed = false;
      report.closeError = error.stack || String(error);
    }
  }
  report.durationMs = Math.round(performance.now() - began);
  const dest = fenced(path.join(logs, "recycle-native-qa.json"), "report");
  fs.writeFileSync(dest, JSON.stringify(report, null, 2));
  console.log(
    JSON.stringify({
      passed: report.passed,
      syntheticOnly: report.syntheticOnly,
      exeSha256: report.exeSha256,
      checks: report.checks.length,
      durationMs: report.durationMs,
      error: report.error,
      report: dest,
    }),
  );
}
if (failure) throw failure;
