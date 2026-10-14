import { launch, isolatedPath, sleep } from "./native-session.mjs";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";

// Explicit synthetic-data QA only. Never point this at an installed EXE or a real library.
const [exeArgument, rootArgument, resourceArgument] = process.argv.slice(2);
const fence = path.resolve("D:/Ai/illuTag-refactor-work") + path.sep;
function workspacePath(value, label) {
  const target = isolatedPath(value, label);
  assert.ok(
    target.toLowerCase().startsWith(fence.toLowerCase()),
    `${label} must be below D:/Ai/illuTag-refactor-work`,
  );
  let ancestor = target;
  while (!fs.existsSync(ancestor)) ancestor = path.dirname(ancestor);
  assert.ok(
    (fs.realpathSync(ancestor) + path.sep)
      .toLowerCase()
      .startsWith(fence.toLowerCase()),
    `${label} resolves outside the QA workspace`,
  );
  return target;
}
const executable = workspacePath(exeArgument, "candidate program");
const root = workspacePath(rootArgument, "fresh QA root");
assert.ok(
  resourceArgument && path.isAbsolute(resourceArgument),
  "Explicit absolute resource root required",
);
const resourceRoot = fs.realpathSync(resourceArgument);
const python = path.join(resourceRoot, "runtime", "python", "python.exe");
assert.ok(fs.statSync(executable).isFile(), "Candidate executable missing");
assert.ok(
  fs.statSync(python).isFile(),
  "Existing read-only resource Python missing",
);
assert.notEqual(
  process.env.ILLUTAG_READ_ONLY,
  "1",
  "Native QA needs its own writable empty test database",
);
const data = path.join(root, "data"),
  assets = path.join(root, "assets"),
  logs = path.join(root, "logs");
const database = path.join(data, "illutag.sqlite"),
  cache = path.join(data, "cache", "duplicates.sqlite");
assert.ok(
  !fs.existsSync(database),
  "Use a fresh QA root: database already exists",
);
assert.ok(
  !fs.existsSync(cache),
  "Use a fresh QA root: duplicate cache already exists",
);
assert.ok(
  !fs.existsSync(assets) || fs.readdirSync(assets).length === 0,
  "Use fresh empty synthetic assets",
);
for (const directory of [data, assets, logs])
  fs.mkdirSync(directory, { recursive: true });
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
const began = performance.now();
const report = {
  passed: false,
  startedAt: new Date().toISOString(),
  checks: [],
  sessions: [],
  jobs: [],
  assetBytes: 0,
};
let session = null,
  failure = null;

const fixtures = String.raw`
from PIL import Image, ImageDraw, ImageEnhance, PngImagePlugin
import pathlib, sys, math, shutil, struct, zlib, json
mode, directory = sys.argv[1:3]
p = pathlib.Path(directory).resolve()
assert p.name == 'assets' and 'illutag-refactor-work' in [s.lower() for s in p.parts]
def canvas(w, h):
    im = Image.new('RGB', (w, h)); pixels = im.load()
    for y in range(h):
        for x in range(w):
            u, v = x / w, y / h
            wave = 8 * math.sin(u * 23 + v * 11)
            pixels[x,y] = (int(35 + 140*u + wave), int(55 + 120*v + wave), int(155 - 70*u + 30*v))
    draw = ImageDraw.Draw(im)
    draw.ellipse((w*.08, h*.08, w*.38, h*.48), fill=(232,204,112))
    draw.polygon([(w*.15,h*.8),(w*.5,h*.25),(w*.9,h*.85)], fill=(43,86,72))
    draw.rectangle((w*.58,h*.12,w*.9,h*.24), fill=(177,75,70))
    return im
if mode == 'initial':
    im = canvas(192,144); im.save(p/'original.png')
    shutil.copyfile(p/'original.png', p/'copy-a.png'); shutil.copyfile(p/'original.png', p/'copy-b.png')
    meta = PngImagePlugin.PngInfo(); meta.add_text('qa', 'different bytes, identical pixels'); im.save(p/'metadata.png', pnginfo=meta)
    im.resize((96,72), Image.Resampling.LANCZOS).save(p/'resized.png')
    im.save(p/'recompressed.jpg', quality=94, subsampling=0)
    ImageEnhance.Brightness(im).enhance(1.025).save(p/'light-color.png')
    Image.eval(im, lambda x: 255-x).save(p/'negative-color.png')
    negative = Image.new('RGB',(192,144),(13,24,39)); d=ImageDraw.Draw(negative)
    d.rectangle((8,10,80,130),fill=(185,48,148)); d.ellipse((94,15,182,116),fill=(37,209,230))
    negative.save(p/'negative-composition.png')
    negative.resize((140,100)).save(p/'decode-fixture.png')
    Image.new('RGB',(128,128),(240,240,240)).save(p/'solid.png')
elif mode == 'bulk':
    im = canvas(128,96)
    for i in range(200):
        name=p/f'bulk-{i:03d}.png'; assert not name.exists()
        meta=PngImagePlugin.PngInfo(); meta.add_text('qa', f'unique container {i}')
        im.save(name,pnginfo=meta)
elif mode == 'damage':
    target=p/'decode-fixture.png'; raw=target.read_bytes(); out=bytearray(raw[:8]); at=8; replaced=False
    while at<len(raw):
        length=struct.unpack('>I',raw[at:at+4])[0]; kind=raw[at+4:at+8]; payload=raw[at+8:at+8+length]; at+=length+12
        if kind==b'IDAT':
            if replaced: continue
            payload=zlib.compress(b'\x00'); replaced=True
        out+=struct.pack('>I',len(payload))+kind+payload+struct.pack('>I',zlib.crc32(kind+payload)&0xffffffff)
    assert replaced; target.write_bytes(out)
    with Image.open(target) as damaged:
        assert damaged.size == (140,100)
        try: damaged.load()
        except Exception: pass
        else: raise AssertionError('Fixture must fail pixel decoding')
else: raise AssertionError('Unknown fixture mode')
size=sum(x.stat().st_size for x in p.iterdir() if x.is_file())
assert size < 3*1024*1024, f'Synthetic asset budget exceeded: {size}'
print(json.dumps({'bytes':size,'files':len(list(p.iterdir()))}))
`;
const inspection = String.raw`
import sqlite3, pathlib, sys, hashlib, json
p=pathlib.Path(sys.argv[1]).resolve(); mode=sys.argv[2]
assert 'illutag-refactor-work' in [s.lower() for s in p.parts]
c=sqlite3.connect(p.as_uri()+'?mode=ro',uri=True,timeout=10)
def digest(rows): return hashlib.sha256(json.dumps(rows,ensure_ascii=False,separators=(',',':')).encode()).hexdigest()
if mode=='main':
    names=[r[0] for r in c.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")]
    counts={name:c.execute('SELECT COUNT(*) FROM '+name).fetchone()[0] for name in ['images','storage_auto_tags','storage_image_user_custom_tags','storage_image_user_supplement_tags','storage_image_tags','tags']}
    result={'schema':c.execute('PRAGMA user_version').fetchone()[0],'tables':names,'counts':counts,'contentHashDigest':digest(c.execute('SELECT id,content_hash FROM images ORDER BY id').fetchall())}
else:
    rows=c.execute('SELECT image_pk,hex(public_hash),hex(signature),size,mtime,hex(identity),hex(sha),hex(phash),hex(dhash),hex(color),width,height,low_info,skip FROM fingerprints ORDER BY image_pk').fetchall()
    result={'fingerprints':len(rows),'fingerprintDigest':digest(rows)}
c.close(); print(json.dumps(result))
`;
function pythonRun(program, args) {
  return JSON.parse(
    execFileSync(python, ["-B", "-c", program, ...args], {
      windowsHide: true,
      cwd: root,
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
function makeFixtures(mode) {
  const result = pythonRun(fixtures, [mode, assets]);
  report.assetBytes = result.bytes;
  return result;
}
function snapshot(kind = "main") {
  return pythonRun(inspection, [kind === "main" ? database : cache, kind]);
}
function assertMainPreserved(before, label) {
  const after = snapshot();
  assert.equal(after.schema, 3, `${label}: main schema changed`);
  assert.deepEqual(
    after.tables,
    before.tables,
    `${label}: new main tables appeared`,
  );
  assert.deepEqual(
    after.counts,
    before.counts,
    `${label}: image/tag data changed`,
  );
  assert.equal(
    after.contentHashDigest,
    before.contentHashDigest,
    `${label}: content_hash changed`,
  );
  report.checks.push(label);
  return after;
}
async function openSession() {
  session = await launch(executable, root, 9796, { resourceRoot });
  await session.page.waitForURL("http://tauri.localhost/**", {
    timeout: 30000,
  });
  await session.page.waitForSelector(".app", { timeout: 30000 });
  const info = await session.invoke("data_directory_info_command");
  assert.equal(
    path.resolve(info.dataDir.replace(/^\\\\\?\\/, "")).toLowerCase(),
    path.resolve(data).toLowerCase(),
    "Isolation must never fall back to production data",
  );
  assert.equal(info.usingFallback, false);
  assert.ok(Number.isInteger(session.child.pid));
  return session;
}
async function closeSession() {
  const closing = session;
  if (!closing) return;
  session = null;
  try {
    await closing.close();
  } finally {
    report.sessions.push({
      ...closing.getDiagnostics(),
      durationMs: Math.round(performance.now() - closing.started),
    });
  }
  const diagnostic = report.sessions.at(-1);
  assert.equal(
    diagnostic.killed,
    false,
    "Test process should close through its normal window command",
  );
  assert.equal(
    diagnostic.exitCode,
    0,
    "Native test process did not exit normally",
  );
}
async function jobs() {
  return session.invoke("list_jobs");
}
async function waitForJob(id, predicate, label, timeout = 120000) {
  const until = performance.now() + timeout;
  while (performance.now() < until) {
    const row = (await jobs()).find((item) => item.id === id);
    if (row && predicate(row)) return row;
    await sleep(50);
  }
  throw Error(`${label}: task ${id} timed out`);
}
const terminal = (row) =>
  ["completed", "partial", "cancelled", "interrupted", "failed"].includes(
    row.state,
  );
async function finish(id, expected = "completed") {
  const row = await waitForJob(id, terminal, "Task completion");
  assert.equal(row.state, expected, JSON.stringify(row.errors));
  if (row.request.kind === "duplicates" && row.state === "completed") {
    assert.equal(row.processed, row.total, "Terminal progress must immediately include every checked image");
  }
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
async function scan() {
  const job = await session.invoke("start_job", {
    request: {
      kind: "scan",
      thumbnails: false,
      sourceFolderId: null,
      duplicateOptions: null,
    },
  });
  await finish(job.id);
}
async function startDuplicate(options = {}) {
  return session.invoke("start_job", {
    request: {
      kind: "duplicates",
      thumbnails: false,
      sourceFolderId: null,
      duplicateOptions: {
        sensitivity: "standard",
        forceRecompute: false,
        regroupOnly: false,
        ...options,
      },
    },
  });
}
async function pausedDuplicate() {
  // One CDP hop minimizes the gap between running observation and real JobRuntime pause.
  return session.page.evaluate(async () => {
    const invoke = (command, args = {}) =>
      window.__TAURI_INTERNALS__.invoke(command, args);
    const row = await invoke("start_job", {
      request: {
        kind: "duplicates",
        thumbnails: false,
        sourceFolderId: null,
        duplicateOptions: {
          sensitivity: "standard",
          forceRecompute: true,
          regroupOnly: false,
        },
      },
    });
    const until = performance.now() + 15000;
    while (performance.now() < until) {
      const current = (await invoke("list_jobs")).find(
        (job) => job.id === row.id,
      );
      if (current?.state === "running") {
        await invoke("control_job", { id: row.id, action: "pause" });
        const paused = (await invoke("list_jobs")).find(
          (job) => job.id === row.id,
        );
        if (paused?.state !== "paused")
          throw Error("Native pause state not persisted");
        return paused;
      }
      if (
        current &&
        ["completed", "partial", "cancelled", "failed"].includes(current.state)
      )
        throw Error("Synthetic task completed before pause could be exercised");
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    throw Error("Native running task never appeared");
  });
}
async function uiJob(buttonName) {
  const before = new Set((await jobs()).map((row) => row.id));
  await session.page
    .getByRole("button", { name: buttonName, exact: true })
    .click();
  const until = performance.now() + 15000;
  while (performance.now() < until) {
    const row = (await jobs()).find(
      (job) => job.request.kind === "duplicates" && !before.has(job.id),
    );
    if (row) return finish(row.id);
    await sleep(25);
  }
  throw Error("UI did not start a duplicate task");
}
async function openDuplicates() {
  await session.page
    .getByRole("button", { name: "重复检索", exact: true })
    .click();
  await session.page
    .getByRole("heading", { name: "重复检索", exact: true })
    .waitFor();
}
async function allGroups(status) {
  const collected = [];
  let cursor = null;
  do {
    const result = await session.invoke("query_duplicate_groups", {
      query: { generation: status.generation, kind: "all", cursor, limit: 20 },
    });
    assert.equal(result.generation, status.generation);
    assert.ok(result.groups.length <= 20);
    collected.push(...result.groups);
    cursor = result.nextCursor;
  } while (cursor);
  return collected;
}
async function fullMembers(status, group) {
  const members = [...group.members];
  let cursor = group.nextCursor;
  while (cursor) {
    const result = await session.invoke("query_duplicate_members", {
      query: {
        generation: status.generation,
        groupId: group.id,
        cursor,
        limit: 50,
      },
    });
    assert.ok(result.members.length <= 50);
    members.push(...result.members);
    cursor = result.nextCursor;
  }
  return members;
}

try {
  const initialFiles = makeFixtures("initial");
  assert.equal(initialFiles.files, 11);
  await openSession();
  const initial = await session.invoke("get_bootstrap");
  assert.equal(initial.page.totalImageCount, 0);
  assert.equal((await jobs()).length, 0);
  assert.ok(!fs.existsSync(cache), "Startup must not create a duplicate cache");
  assert.equal((await session.invoke("duplicate_status")).generation, null);
  await session.invoke("add_gallery_folder_command", { folderPath: assets });
  const registered = await session.invoke("get_bootstrap");
  assert.equal(registered.page.totalImageCount, 0);
  assert.equal(registered.folders[0].lastScannedAt, null);
  assert.equal((await jobs()).length, 0);
  report.checks.push(
    "empty isolated schema 3, source registration and startup start no jobs",
  );
  await scan();
  const scanned = await session.invoke("get_bootstrap");
  assert.equal(scanned.page.totalImageCount, 11);
  const original = scanned.page.images.find(
    (image) => image.fileName === "original.png",
  );
  assert.ok(original);
  await session.invoke("add_image_user_custom_tag_command", {
    imageId: original.id,
    tagText: "duplicate-qa-marker",
  });
  const beforeDuplicate = snapshot();
  assert.equal(beforeDuplicate.schema, 3);
  assert.ok(!beforeDuplicate.tables.some((name) => /duplicate/i.test(name)));
  assert.equal(beforeDuplicate.counts.storage_image_user_custom_tags, 1);
  await session.page.reload();
  await session.page.waitForSelector("[data-gallery-image-id]", {
    timeout: 30000,
  });
  await openDuplicates();
  await uiJob("开始查重");
  let status = await session.invoke("duplicate_status");
  assert.equal(status.state, "ready");
  assert.equal(status.coverage.processed, 11);
  assert.equal(status.coverage.failed, 0);
  assert.ok(status.coverage.lowInformation >= 1);
  let groups = await allGroups(status);
  const exact = groups.find(
    (group) =>
      group.kind === "exact" &&
      group.members.some((member) => member.image.fileName === "original.png"),
  );
  assert.ok(exact, "Byte-identical copies not grouped");
  assert.deepEqual(
    (await fullMembers(status, exact))
      .map((member) => member.image.fileName)
      .sort(),
    ["copy-a.png", "copy-b.png", "original.png"],
  );
  const nearPairs = [];
  for (const group of groups.filter((group) => group.kind === "near"))
    nearPairs.push(
      (await fullMembers(status, group)).map((member) => member.image.fileName),
    );
  assert.ok(
    nearPairs.some(
      (names) =>
        names.includes("metadata.png") && names.includes("original.png"),
    ),
    "Same pixels in different PNG containers not identified",
  );
  assert.ok(
    nearPairs.some(
      (names) =>
        names.includes("original.png") &&
        names.some((name) =>
          ["resized.png", "recompressed.jpg", "light-color.png"].includes(name),
        ),
    ),
    "No resize/recompression/color candidate",
  );
  assert.ok(
    !nearPairs.some(
      (names) =>
        names.includes("original.png") &&
        (names.includes("negative-color.png") ||
          names.includes("negative-composition.png")),
    ),
    "Negative scene incorrectly matched original",
  );
  assertMainPreserved(
    beforeDuplicate,
    "duplicate search preserves main schema/tags/content_hash",
  );
  await session.page.locator(".duplicate-group").first().waitFor();
  await session.page.screenshot({
    path: path.join(logs, "duplicates-native-groups.png"),
  });
  const candidate = session.page
    .getByRole("button", { name: "与参考图比对：copy-a.png", exact: true })
    .first();
  await candidate.scrollIntoViewIfNeeded();
  const scrollBefore = await session.page
    .locator(".duplicate-page")
    .evaluate((el) => el.scrollTop);
  await candidate.click();
  await session.page
    .getByRole("dialog", { name: "图片并排比对", exact: true })
    .waitFor();
  await session.page
    .getByRole("button", { name: "参考图放大", exact: true })
    .click();
  const transforms = await session.page
    .locator(".duplicate-pan-stage img")
    .evaluateAll((nodes) => nodes.map((node) => node.style.transform));
  assert.ok(
    transforms[0].includes("scale(1.2)") && transforms[1].includes("scale(1)"),
    "Native independent zoom failed",
  );
  await session.page.screenshot({
    path: path.join(logs, "duplicates-native-comparison.png"),
  });
  await session.page.keyboard.press("Escape");
  await session.page
    .locator(".duplicate-comparison")
    .waitFor({ state: "hidden" });
  assert.equal(
    await candidate.evaluate((el) => document.activeElement === el),
    true,
  );
  assert.equal(
    await session.page
      .locator(".duplicate-page")
      .evaluate((el) => el.scrollTop),
    scrollBefore,
  );
  report.checks.push(
    "WebView2 original-image comparison, independent zoom, Esc focus and scroll restore",
  );
  const cacheBeforeRegroup = snapshot("cache");
  await session.page
    .getByRole("combobox", { name: "近似筛选严格度", exact: true })
    .click();
  await session.page.getByRole("option", { name: "严格", exact: true }).click();
  await session.page
    .getByText("已选严格，当前结果仍使用标准。点击“重新检索”后生效。", {
      exact: true,
    })
    .waitFor();
  assert.equal(
    (await session.invoke("duplicate_status")).sensitivity,
    "standard",
  );
  await uiJob("重新检索");
  status = await session.invoke("duplicate_status");
  assert.equal(status.sensitivity, "strict");
  assert.deepEqual(
    snapshot("cache"),
    cacheBeforeRegroup,
    "Threshold-only grouping changed fingerprints",
  );
  assertMainPreserved(
    beforeDuplicate,
    "strict regroup reuses fingerprints and preserves main data",
  );

  makeFixtures("bulk");
  await scan();
  assert.equal(
    (await session.invoke("get_bootstrap")).page.totalImageCount,
    211,
  );
  assert.equal((await session.invoke("duplicate_status")).needsUpdate, true);
  const beforeBulkDuplicate = snapshot(),
    retainedGeneration = (await session.invoke("duplicate_status")).generation;
  const paused = await pausedDuplicate();
  await sleep(250);
  assert.equal(
    (await jobs()).find((job) => job.id === paused.id).state,
    "paused",
  );
  await session.invoke("control_job", { id: paused.id, action: "cancel" });
  await finish(paused.id, "cancelled");
  assert.equal(
    (await session.invoke("duplicate_status")).generation,
    retainedGeneration,
    "Cancellation replaced the last complete generation",
  );
  assert.ok(snapshot("cache").fingerprints >= cacheBeforeRegroup.fingerprints);
  report.checks.push(
    "real JobRuntime pause/cancel keeps submitted fingerprints and complete generation",
  );
  const updated = await startDuplicate();
  await finish(updated.id);
  status = await session.invoke("duplicate_status");
  assert.equal(status.coverage.processed, 211);
  assertMainPreserved(
    beforeBulkDuplicate,
    "bulk update preserves image/tag data and content_hash",
  );
  groups = await allGroups(status);
  const large = groups.find(
    (group) => group.kind === "near" && group.totalMembers >= 56,
  );
  assert.ok(
    large,
    "Synthetic metadata variants should exercise real member pagination",
  );
  assert.equal(large.members.length, 6);
  const memberPage = await session.invoke("query_duplicate_members", {
    query: {
      generation: status.generation,
      groupId: large.id,
      cursor: large.nextCursor,
      limit: 50,
    },
  });
  assert.equal(memberPage.members.length, 50);
  const largeArticle = session.page
    .locator(".duplicate-group")
    .filter({
      has: session.page.getByRole("button", {
        name: "查看参考图：" + large.reference.image.fileName,
        exact: true,
      }),
    })
    .filter({
      has: session.page.locator("header strong", { hasText: "近似候选" }),
    })
    .first();
  await largeArticle.waitFor();
  assert.equal(await largeArticle.locator(".duplicate-thumb").count(), 6);
  await largeArticle
    .getByRole("button", { name: "展开更多图片（每次 50 张）", exact: true })
    .click();
  await session.page.waitForFunction(
    (name) =>
      Array.from(document.querySelectorAll(".duplicate-group")).some(
        (group) =>
          group.querySelector("header strong")?.textContent === "近似候选" &&
          group.querySelector('[aria-label="查看参考图：' + name + '"]') &&
          group.querySelectorAll(".duplicate-thumb").length === 56,
      ),
    large.reference.image.fileName,
  );
  report.checks.push(
    "real 20-group contract and 6-preview/50-member pagination",
  );
  await session.page.screenshot({
    path: path.join(logs, "duplicates-native-expanded.png"),
  });

  const generationBeforeRestart = status.generation;
  const interrupted = await pausedDuplicate();
  await closeSession();
  await openSession();
  const restartJobs = await jobs();
  assert.equal(
    restartJobs.find((job) => job.id === interrupted.id)?.state,
    "interrupted",
  );
  assert.equal(
    restartJobs.filter((job) =>
      ["queued", "running", "paused", "cancelling"].includes(job.state),
    ).length,
    0,
  );
  status = await session.invoke("duplicate_status");
  assert.equal(status.generation, generationBeforeRestart);
  assert.equal(status.lastAttempt?.state, "interrupted");
  report.checks.push(
    "normal process close/restart persists interrupted tasks without automatic resume",
  );
  assertMainPreserved(
    beforeBulkDuplicate,
    "restart preserves main images/tags/content_hash",
  );

  makeFixtures("damage");
  await scan();
  const beforeDamagedDuplicate = snapshot();
  const damagedRun = await startDuplicate();
  await finish(damagedRun.id);
  status = await session.invoke("duplicate_status");
  assert.ok(
    status.coverage.skippedNear >= 2,
    "Low-information and corrupt pixel streams must be reported",
  );
  assert.equal(status.coverage.processed, 211);
  assertMainPreserved(
    beforeDamagedDuplicate,
    "decode skip retains byte-hash checking without main data writes",
  );
  await openDuplicates();
  await session.page
    .getByRole("button", { name: "清理查重缓存", exact: true })
    .click();
  await session.page
    .getByRole("dialog", { name: "清理查重缓存", exact: true })
    .getByRole("button", { name: "清理缓存", exact: true })
    .click();
  await session.page
    .getByRole("heading", { name: "开始一次全库检查", exact: true })
    .waitFor();
  for (const suffix of ["", "-wal", "-shm", "-journal"])
    assert.ok(
      !fs.existsSync(cache + suffix),
      "Derived cache companion remained after UI clear: " + suffix,
    );
  assert.equal((await session.invoke("duplicate_status")).generation, null);
  assertMainPreserved(
    beforeDamagedDuplicate,
    "UI clear removes only derived duplicate cache",
  );
  await session.page.getByRole("navigation", { name: "图库导航" }).getByRole("button", { name: "全部", exact: true }).click();
  await session.page.waitForSelector("[data-gallery-image-id]", {
    timeout: 30000,
  });
  assert.equal(
    (await session.invoke("get_bootstrap")).page.totalImageCount,
    211,
  );
  assert.ok(
    (
      await session.invoke("get_image_detail", { imageId: original.id })
    ).userTags.customTags.some((tag) => tag.tagText === "duplicate-qa-marker"),
  );
  await session.page.screenshot({
    path: path.join(logs, "duplicates-native-gallery-after-clear.png"),
  });
  report.checks.push(
    "normal gallery and existing custom tag remain usable after cache clearing",
  );
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
    report.closeError = error?.message || String(error);
  }
  report.durationMs = Math.round(performance.now() - began);
  report.finishedAt = new Date().toISOString();
  fs.writeFileSync(
    path.join(logs, "duplicates-native-qa.json"),
    JSON.stringify(report, null, 2),
  );
  console.log(JSON.stringify(report));
}
if (failure) throw failure;
