import { launch, isolatedPath, sleep } from "./native-session.mjs";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";

// All writes and recycle-bin checks use newly generated synthetic assets only.
const [exeArgument, rootArgument, resourceArgument] = process.argv.slice(2);
const fence = path.resolve("D:/Ai/illuTag-refactor-work") + path.sep;
function qaPath(value, label) {
  const target = isolatedPath(value, label);
  assert.ok(
    target.toLowerCase().startsWith(fence.toLowerCase()),
    `${label} escapes QA fence`,
  );
  return target;
}
const executable = qaPath(exeArgument, "candidate"),
  root = qaPath(rootArgument, "fresh session");
assert.ok(
  resourceArgument && path.isAbsolute(resourceArgument),
  "Explicit resource root required",
);
const resourceRoot = fs.realpathSync(resourceArgument);
const python = path.join(resourceRoot, "runtime/python/python.exe");
const data = path.join(root, "data"),
  assets = path.join(root, "assets"),
  logs = path.join(root, "logs");
const database = path.join(data, "illutag.sqlite"),
  cache = path.join(data, "cache/duplicates.sqlite");
assert.ok(
  fs.existsSync(executable) && fs.existsSync(python),
  "Candidate/runtime missing",
);
assert.ok(
  !fs.existsSync(database) && !fs.existsSync(cache),
  "A fresh QA database is required",
);
assert.ok(
  !fs.existsSync(assets) || fs.readdirSync(assets).length === 0,
  "Synthetic asset root must be empty",
);
for (const folder of [data, assets, logs])
  fs.mkdirSync(folder, { recursive: true });
fs.writeFileSync(
  path.join(data, "window-state.json"),
  JSON.stringify({
    x: 26,
    y: 26,
    width: 1312,
    height: 838,
    isMaximized: false,
    isFullscreen: false,
  }),
);
const fixtureCode = String.raw`
from PIL import Image, ImageDraw, PngImagePlugin
import pathlib, sys, shutil, json
p=pathlib.Path(sys.argv[1]).resolve()
assert p.name=='assets' and 'illutag-refactor-work' in [s.lower() for s in p.parts]
im=Image.new('RGB',(128,96)); pixels=im.load()
for y in range(96):
    for x in range(128): pixels[x,y]=(int(35+x*1.1),int(55+y*1.2),int(155-x*.4+y*.2))
d=ImageDraw.Draw(im); d.ellipse((10,8,48,46),fill=(232,204,112)); d.polygon([(18,78),(64,22),(116,82)],fill=(43,86,72)); d.rectangle((74,12,116,24),fill=(177,75,70))
master=p/'a-original.png'; im.save(master)
for i in range(12): shutil.copyfile(master,p/f'a-copy-{i:02}.png')
meta=PngImagePlugin.PngInfo(); meta.add_text('test','different container, same pixels'); im.save(p/'a-metadata.png',pnginfo=meta)
other=Image.eval(im,lambda x:255-x); other.save(p/'b-original.png')
for i in range(5): shutil.copyfile(p/'b-original.png',p/f'b-copy-{i:02}.png')
size=sum(x.stat().st_size for x in p.iterdir()); assert size < 1024*1024
print(json.dumps({'count':len(list(p.iterdir())),'bytes':size}))
`;
const inspection = String.raw`
import sqlite3,pathlib,sys,json,hashlib
p=pathlib.Path(sys.argv[1]).resolve(); assert 'illutag-refactor-work' in [s.lower() for s in p.parts]
c=sqlite3.connect(p.as_uri()+'?mode=ro',uri=True,timeout=10)
if sys.argv[2]=='main':
    result={'schema':c.execute('pragma user_version').fetchone()[0],'images':c.execute('select count(*) from images').fetchone()[0],'trashed':c.execute('select count(*) from images where trashed=1').fetchone()[0],'customTags':c.execute('select count(*) from storage_image_user_custom_tags').fetchone()[0],'contentHashDigest':hashlib.sha256(json.dumps(c.execute('select id,content_hash from images order by id').fetchall()).encode()).hexdigest()}
else:
    rows=c.execute('select image_pk,hex(signature),hex(sha),hex(phash),hex(dhash),hex(color) from fingerprints order by image_pk').fetchall()
    result={'fingerprints':len(rows),'digest':hashlib.sha256(json.dumps(rows).encode()).hexdigest()}
c.close(); print(json.dumps(result))
`;
function pythonRun(code, args) {
  return JSON.parse(
    execFileSync(python, ["-B", "-c", code, ...args], {
      cwd: root,
      windowsHide: true,
      timeout: 30000,
      maxBuffer: 1024 * 1024,
      env: {
        ...process.env,
        PYTHONDONTWRITEBYTECODE: "1",
        PYTHONPYCACHEPREFIX: path.join(root, "pycache"),
        TEMP: logs,
        TMP: logs,
      },
    })
      .toString()
      .trim(),
  );
}
const report = {
  passed: false,
  checks: [],
  sessions: [],
  jobs: [],
  fixture: pythonRun(fixtureCode, [assets]),
  startedAt: new Date().toISOString(),
};
let session = null,
  failure = null;
const began = performance.now();
function snapshot(kind = "main") {
  return pythonRun(inspection, [kind === "main" ? database : cache, kind]);
}
async function openSession() {
  session = await launch(executable, root, 9797, { resourceRoot });
  await session.page.waitForSelector(".app", { timeout: 30000 });
  const info = await session.invoke("data_directory_info_command");
  assert.equal(
    path.resolve(info.dataDir.replace(/^\\\\\?\\/, "")).toLowerCase(),
    data.toLowerCase(),
  );
  assert.equal(info.usingFallback, false);
}
async function closeSession() {
  if (!session) return;
  const closing = session;
  session = null;
  await closing.close();
  const diagnostic = closing.getDiagnostics();
  report.sessions.push(diagnostic);
  assert.equal(diagnostic.killed, false);
  assert.equal(diagnostic.exitCode, 0);
}
async function finish(id, expected = "completed") {
  const until = performance.now() + 60000;
  while (performance.now() < until) {
    const row = (await session.invoke("list_jobs")).find((j) => j.id === id);
    if (
      row &&
      ["completed", "partial", "cancelled", "interrupted", "failed"].includes(
        row.state,
      )
    ) {
      assert.ok(
        (Array.isArray(expected) ? expected : [expected]).includes(row.state),
        JSON.stringify({ state: row.state, errors: row.errors }),
      );
      report.jobs.push({
        id: row.id,
        kind: row.request.kind,
        state: row.state,
        processed: row.processed,
        total: row.total,
        failures: row.failures,
      });
      return row;
    }
    await sleep(30);
  }
  throw Error(`Task ${id} timed out`);
}
async function start(kind, duplicateOptions = null) {
  const job = await session.invoke("start_job", {
    request: {
      kind,
      thumbnails: false,
      sourceFolderId: null,
      duplicateOptions,
    },
  });
  return finish(job.id);
}
async function groupPages(status) {
  const result = [];
  let cursor = null;
  do {
    const page = await session.invoke("query_duplicate_groups", {
      query: { generation: status.generation, kind: "all", cursor, limit: 20 },
    });
    result.push(...page.groups);
    cursor = page.nextCursor;
  } while (cursor);
  return result;
}
async function members(status, group) {
  const result = [...group.members];
  let cursor = group.nextCursor;
  while (cursor) {
    const page = await session.invoke("query_duplicate_members", {
      query: {
        generation: status.generation,
        groupId: group.id,
        cursor,
        limit: 50,
      },
    });
    result.push(...page.members);
    cursor = page.nextCursor;
  }
  return result;
}
async function operationResults(operationId, state = "all") {
  const result = [];
  let cursor = null;
  do {
    const page = await session.invoke("duplicate_operation_results", {
      query: { operationId, cursor, limit: 50, state },
    });
    assert.equal(page.operationId, operationId);
    assert.ok(page.items.length <= 50);
    result.push(...page.items);
    cursor = page.nextCursor;
  } while (cursor);
  return result;
}
async function apply(preview, expected = "completed") {
  const job = await session.invoke("apply_duplicate_action", {
    request: { operationId: preview.operationId, confirmed: true },
  });
  await finish(job.id, expected);
  return session.invoke("duplicate_operation_status", {
    operationId: preview.operationId,
  });
}
async function mutate(action, imageIds) {
  return session.invoke("mutate_images", {
    request: {
      action,
      imageIds,
      folderId: null,
      favorite: null,
      customTags: [],
      supplementTags: [],
    },
  });
}
async function managementChecks({ status, groups, exact, original }) {
  const before = snapshot(),
    fingerprintBefore = snapshot("cache");
  const all = await members(status, exact);
  const protectedImage = all.find(
    (m) => m.image.fileName === "a-copy-01.png",
  ).image;
  const removedReference = exact.reference.image.id;
  assert.notEqual(removedReference, original.id);
  assert.notEqual(removedReference, protectedImage.id);
  const request = {
    generation: status.generation,
    groupId: exact.id,
    mode: "exactOthers",
    keepIds: [original.id],
    protectedIds: [protectedImage.id],
    imageIds: [],
    action: "trash",
    sourceOperationId: null,
  };
  const outdated = await session.invoke("prepare_duplicate_action", {
    request,
  });
  const keeperBytes = fs.readFileSync(original.path);
  const keeperTimes = pythonRun(
    "import pathlib,sys,json; p=pathlib.Path(sys.argv[1]).resolve(); assert 'illutag-refactor-work' in [s.lower() for s in p.parts]; s=p.stat(); print(json.dumps({'atime':str(s.st_atime_ns),'mtime':str(s.st_mtime_ns)}))",
    [original.path],
  );
  try {
    fs.appendFileSync(
      original.path,
      Buffer.from("changed during confirmation"),
    );
    let rejectedJob = null;
    try {
      rejectedJob = await session.invoke("apply_duplicate_action", {
        request: { operationId: outdated.operationId, confirmed: true },
      });
    } catch {
      /* A pre-enqueue rejection is also valid. */
    }
    if (rejectedJob) await finish(rejectedJob.id, ["partial", "failed"]);
    assert.equal(
      snapshot().trashed,
      0,
      "An invalid keeper must stop before the first image mutation",
    );
  } finally {
    fs.writeFileSync(original.path, keeperBytes);
    pythonRun(
      "import os,pathlib,sys,json; p=pathlib.Path(sys.argv[1]).resolve(); assert 'illutag-refactor-work' in [s.lower() for s in p.parts]; os.utime(p,ns=(int(sys.argv[2]),int(sys.argv[3]))); print(json.dumps({'restored':True}))",
      [original.path, keeperTimes.atime, keeperTimes.mtime],
    );
  }
  report.checks.push(
    "keeper changed after preview prevents all image mutations before execution",
  );
  const preview = await session.invoke("prepare_duplicate_action", { request });
  assert.equal(preview.targetCount, 11);
  assert.ok(preview.protectedCount >= 1);
  assert.ok(preview.sample.length <= 6);
  assert.equal(
    snapshot().trashed,
    0,
    "Preparing a review must not modify library",
  );
  const completed = await apply(preview);
  assert.equal(completed.succeeded, 11);
  assert.equal(completed.failed, 0);
  assert.equal(completed.undoable, true);
  const results = await operationResults(preview.operationId);
  assert.equal(results.length, 11);
  assert.ok(
    !results.some((r) => [original.id, protectedImage.id].includes(r.imageId)),
  );
  const after = snapshot();
  assert.equal(after.images, before.images);
  assert.equal(after.trashed, 11);
  assert.equal(after.customTags, before.customTags);
  assert.equal(after.contentHashDigest, before.contentHashDigest);
  assert.equal(after.schema, 3);
  assert.deepEqual(snapshot("cache"), fingerprintBefore);
  assert.equal(
    fs.readdirSync(assets).length,
    report.fixture.count,
    "Soft recycle must retain original files",
  );
  const survivors = await session.invoke("query_duplicate_group", {
    query: { generation: status.generation, groupId: exact.id },
  });
  assert.ok(
    survivors,
    "Removing the old reference must not hide surviving copies",
  );
  assert.equal(survivors.totalMembers, 2);
  assert.ok(
    [original.id, protectedImage.id].includes(survivors.reference.image.id),
  );
  assert.equal(
    (await session.invoke("duplicate_status")).generation,
    status.generation,
  );
  report.checks.push(
    "whole-group action includes unloaded members; cross-group keep protection; local reference repair; fingerprints reused",
  );

  // An ordinary later restore/trash must take ownership away from the earlier undo.
  const laterOwner = all.find((m) => m.image.fileName === "a-copy-02.png").image
    .id;
  await mutate("restore", [laterOwner]);
  await mutate("trash", [laterOwner]);
  const undo = await session.invoke("undo_duplicate_action", {
    operationId: preview.operationId,
  });
  await finish(undo.id, ["partial", "completed"]);
  const undoStatus = await session.invoke("duplicate_operation_status", {
    operationId: undo.request.duplicateOperationId,
  });
  assert.equal(undoStatus.succeeded, 10);
  assert.equal(
    snapshot().trashed,
    1,
    "Undo must not revive a later independently recycled image",
  );
  assert.equal(snapshot().customTags, before.customTags);
  assert.deepEqual(snapshot("cache"), fingerprintBefore);
  await mutate("restore", [laterOwner]);
  report.checks.push(
    "soft-recycle undo retains metadata and does not override later ordinary recycle ownership",
  );

  const automatic = await session.invoke("prepare_duplicate_action", {
    request: {
      ...request,
      groupId: "all-exact",
      mode: "autoExact",
      keepIds: [],
      protectedIds: [original.id],
    },
  });
  assert.equal(
    automatic.targetCount,
    17,
    "Only the two byte-identical groups enter automatic processing",
  );
  await apply(automatic);
  assert.equal(snapshot().trashed, 17);
  assert.equal(
    (await session.invoke("get_image_detail", { imageId: original.id })).image
      .trashed,
    false,
  );
  const autoUndo = await session.invoke("undo_duplicate_action", {
    operationId: automatic.operationId,
  });
  await finish(autoUndo.id);
  assert.equal(snapshot().trashed, 0);
  assert.deepEqual(snapshot("cache"), fingerprintBefore);
  report.checks.push(
    "one-click exact-only batch retains one copy per group and leaves different-byte near candidates untouched",
  );

  const near = groups.find(
    (g) =>
      g.kind === "near" &&
      g.members.some((m) => m.image.fileName === "a-metadata.png"),
  );
  assert.ok(near);
  await assert.rejects(
    () =>
      session.invoke("prepare_duplicate_action", {
        request: { ...request, groupId: near.id },
      }),
    "Near groups must reject implicit whole-group other selection",
  );
  const metadata = (await members(status, near)).find(
    (m) => m.image.fileName === "a-metadata.png",
  ).image;
  const selectedPreview = await session.invoke("prepare_duplicate_action", {
    request: {
      ...request,
      groupId: near.id,
      mode: "selected",
      imageIds: [metadata.id],
    },
  });
  assert.equal(selectedPreview.targetCount, 1);
  await apply(selectedPreview);
  assert.equal(snapshot().trashed, 1);
  assert.equal(fs.readdirSync(assets).length, report.fixture.count);
  const nearUndo = await session.invoke("undo_duplicate_action", {
    operationId: selectedPreview.operationId,
  });
  await finish(nearUndo.id);
  report.checks.push(
    "near group processes only the explicitly selected candidate, permits multiple surviving versions",
  );

  await session.page.reload();
  await session.page.waitForSelector(".app");
  await session.page
    .getByRole("navigation", { name: "图库导航" })
    .getByRole("button", { name: "重复检索", exact: true })
    .click();
  const nearArticle = session.page.locator(
    `[data-duplicate-group-id="${near.id}"]`,
  );
  await nearArticle
    .getByRole("button", { name: "多图键盘审核", exact: true })
    .click();
  const review = session.page.getByRole("dialog", {
    name: "同组多图审核",
    exact: true,
  });
  await review.waitFor();
  await session.page.waitForFunction(
    () => document.querySelectorAll(".duplicate-multi-card").length >= 13,
  );
  assert.ok((await review.locator(".duplicate-multi-card").count()) >= 13);
  for (const width of [500, 1280]) {
    await session.page.setViewportSize({ width, height: 800 });
    assert.equal(
      await review.evaluate((el) => el.scrollWidth > el.clientWidth),
      false,
    );
  }
  await review.focus();
  await session.page.keyboard.press("k");
  await session.page.keyboard.press("ArrowRight");
  const beforeUiJobs = new Set(
    (await session.invoke("list_jobs")).map((j) => j.id),
  );
  await session.page.keyboard.press("Delete");
  await session.page
    .getByRole("dialog", { name: "移入应用回收站", exact: true })
    .waitFor();
  await session.page.keyboard.press("Enter");
  let uiJob;
  const uiUntil = performance.now() + 10000;
  while (performance.now() < uiUntil) {
    uiJob = (await session.invoke("list_jobs")).find(
      (j) =>
        j.request.kind === "duplicateManagement" && !beforeUiJobs.has(j.id),
    );
    if (uiJob) break;
    await sleep(30);
  }
  assert.ok(uiJob, "Keyboard review must create an explicit soft-recycle task");
  await finish(uiJob.id);
  assert.equal(snapshot().trashed, 1);
  assert.equal(fs.readdirSync(assets).length, report.fixture.count);
  await session.page.keyboard.press("Escape");
  const uiUndo = await session.invoke("undo_duplicate_action", {
    operationId: uiJob.request.duplicateOperationId,
  });
  await finish(uiUndo.id);
  assert.equal(snapshot().trashed, 0);
  report.checks.push(
    "real multi-image review displays the group at 500/1280 CSS widths; K/arrow/Delete/Enter recycles only one manually reviewed candidate",
  );

  // Native system recycle is tested on one small generated copy, never an external image.
  const systemTarget = all.find(
    (m) => m.image.fileName === "a-copy-03.png",
  ).image;
  const soft = await session.invoke("prepare_duplicate_action", {
    request: { ...request, mode: "selected", imageIds: [systemTarget.id] },
  });
  await apply(soft);
  const system = await session.invoke("prepare_duplicate_action", {
    request: {
      ...request,
      mode: "selected",
      imageIds: [systemTarget.id],
      action: "systemTrash",
      sourceOperationId: soft.operationId,
    },
  });
  assert.equal(system.targetCount, 1);
  assert.equal(system.requiresSystemConfirmation, true);
  await assert.rejects(() =>
    session.invoke("apply_duplicate_action", {
      request: { operationId: system.operationId, confirmed: false },
    }),
  );
  assert.ok(fs.existsSync(systemTarget.path));
  const systemStatus = await apply(system);
  assert.equal(systemStatus.succeeded, 1);
  assert.equal(systemStatus.undoable, false);
  const systemResults = await operationResults(system.operationId);
  assert.equal(systemResults.length, 1);
  assert.equal(systemResults[0].fileMoved, true);
  assert.equal(systemResults[0].dbUpdated, true);
  assert.equal(fs.existsSync(systemTarget.path), false);
  assert.equal(snapshot().images, before.images - 1);
  assert.equal(snapshot().customTags, before.customTags);
  assert.equal(snapshot().schema, 3);
  report.checks.push(
    "explicit second-step system recycle moves only one synthetic file and updates its index; no implicit execution",
  );

  // Clearing derived fingerprints must not discard operation records needed for review/undo.
  await session.invoke("clear_duplicate_cache");
  const retained = await session.invoke("duplicate_operation_status", {
    operationId: soft.operationId,
  });
  assert.equal(retained.operationId, soft.operationId);
  assert.equal(snapshot().customTags, before.customTags);
  report.checks.push(
    "clearing fingerprint cache retains separate operation history and original tag data",
  );
}
try {
  await openSession();
  assert.equal((await session.invoke("get_bootstrap")).page.totalImageCount, 0);
  assert.equal((await session.invoke("list_jobs")).length, 0);
  await session.invoke("add_gallery_folder_command", { folderPath: assets });
  await start("scan");
  const page = (await session.invoke("get_bootstrap")).page;
  assert.equal(page.totalImageCount, report.fixture.count);
  const original = page.images.find((i) => i.fileName === "a-original.png");
  assert.ok(original);
  await session.invoke("add_image_user_custom_tag_command", {
    imageId: original.id,
    tagText: "management-qa-marker",
  });
  await start("duplicates", {
    sensitivity: "standard",
    forceRecompute: false,
    regroupOnly: false,
  });
  const status = await session.invoke("duplicate_status");
  assert.equal(status.state, "ready");
  const groups = await groupPages(status);
  const exact = groups.find((g) => g.kind === "exact" && g.totalMembers === 13);
  assert.ok(exact);
  assert.equal(exact.members.length, 6);
  assert.equal((await members(status, exact)).length, 13);
  report.checks.push(
    "synthetic schema 3, manual indexing, exact full group beyond six-member preview",
  );
  await managementChecks({ status, groups, exact, original });
  report.passed = true;
} catch (error) {
  failure = error;
  report.error = error?.message || String(error);
} finally {
  try {
    await closeSession();
  } catch (error) {
    failure ||= error;
    report.passed = false;
    report.closeError = error.message;
  }
  report.durationMs = Math.round(performance.now() - began);
  fs.writeFileSync(
    path.join(logs, "duplicate-management-native-qa.json"),
    JSON.stringify(report, null, 2),
  );
  console.log(JSON.stringify(report));
}
if (failure) throw failure;
