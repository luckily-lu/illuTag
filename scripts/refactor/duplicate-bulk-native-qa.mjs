import { launch, isolatedPath, sleep } from "./native-session.mjs";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";

// Native acceptance using only fresh, generated, nonsexual abstract images.
// EXE ROOT RESOURCE_ROOT [--large] [--skip-ui]
// ROOT must be a new duplicates-1.5.x* directory directly beneath the fence.
// --large adds 20,006 identical files, crossing the old 20,000-item limit.
// The synthetic cache is seeded while the app is CLOSED to provide exact,
// reproducible A--B--C nontransitive near relationships; source SHA/signatures
// are still computed by the real scanner and validated by the real IPC path.
const [exeArgument, rootArgument, resourceArgument, ...flags] = process.argv.slice(2);
assert.ok(flags.every((f) => ["--large", "--skip-ui"].includes(f)), "Unknown QA option");
const fence = path.resolve("D:/Ai/illuTag-refactor-work");
function qaPath(value, label) {
  const target = isolatedPath(value, label);
  assert.ok(target.toLowerCase().startsWith((fence + path.sep).toLowerCase()), `${label} outside QA fence`);
  let ancestor = target;
  while (!fs.existsSync(ancestor)) ancestor = path.dirname(ancestor);
  const actual = fs.realpathSync(ancestor);
  assert.ok(actual.toLowerCase() === fence.toLowerCase() || actual.toLowerCase().startsWith((fence + path.sep).toLowerCase()), `${label} resolves outside QA fence`);
  return target;
}
const executable = qaPath(exeArgument, "candidate"), root = qaPath(rootArgument, "new test root");
assert.equal(path.dirname(root).toLowerCase(), fence.toLowerCase());
assert.match(path.basename(root), /^duplicates-1\.5\.\d+[\w.-]*$/i);
assert.ok(!fs.existsSync(root), "Test root must be completely new; no real or previous database accepted");
assert.ok(resourceArgument && path.isAbsolute(resourceArgument), "Explicit absolute resource root required");
const resourceRoot = fs.realpathSync(resourceArgument), python = path.join(resourceRoot, "runtime/python/python.exe");
assert.ok(fs.statSync(executable).isFile() && fs.statSync(python).isFile(), "Candidate or Python missing");
assert.notEqual(process.env.ILLUTAG_READ_ONLY, "1");
const data = path.join(root, "data"), assets = path.join(root, "assets"), logs = path.join(root, "logs");
const database = path.join(data, "illutag.sqlite"), cache = path.join(data, "cache/duplicates.sqlite");
for (const folder of [data, assets, logs, path.join(root, "temp")]) fs.mkdirSync(qaPath(folder, "output"), { recursive: true });
fs.writeFileSync(path.join(data, "window-state.json"), JSON.stringify({ x: 26, y: 26, width: 1312, height: 838, isMaximized: false, isFullscreen: false }));
const large = flags.includes("--large"), exactGroups = 1003;
const report = { passed: false, startedAt: new Date().toISOString(), checks: [], sessions: [], jobs: [], exactGroups, large, syntheticOnly: true };
report.executable = executable;
report.exeSha256 = createHash("sha256").update(fs.readFileSync(executable)).digest("hex");
let session, failure, generation, fixture;
const began = performance.now();
function pythonRun(code, args = [], timeout = 120000) {
  return JSON.parse(execFileSync(python, ["-B", "-c", code, root, ...args.map(String)], {
    cwd: root, windowsHide: true, timeout, maxBuffer: 8 * 1024 * 1024,
    env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1", PYTHONPYCACHEPREFIX: path.join(root, "pycache"), TEMP: path.join(root, "temp"), TMP: path.join(root, "temp") },
  }).toString().trim());
}
const fixtureCode = String.raw`
from PIL import Image, ImageDraw
import pathlib,sys,random,shutil,json
r=pathlib.Path(sys.argv[1]).resolve(); assert r.parent==pathlib.Path('D:/Ai/illuTag-refactor-work').resolve() and r.name.startswith('duplicates-1.5.')
p=r/'assets'; groups=int(sys.argv[2]); large=sys.argv[3]=='true'
for n in range(groups):
    rng=random.Random(190001+n); im=Image.frombytes('RGB',(32,24),bytes(rng.randrange(256) for _ in range(32*24*3)))
    master=p/f'exact-{n:04d}-0.png'; im.save(master)
    for copy in (1,2): shutil.copyfile(master,p/f'exact-{n:04d}-{copy}.png')
im=Image.new('RGB',(64,48),(33,55,88)); d=ImageDraw.Draw(im); d.ellipse((3,4,30,35),fill=(211,169,64)); d.polygon([(12,44),(41,7),(61,45)],fill=(52,142,73))
im.save(p/'wide-00.png')
for n in range(1,23): shutil.copyfile(p/'wide-00.png',p/f'wide-{n:02d}.png')
for name,size,color in [('near-a',(512,384),(44,77,133)),('near-b',(256,192),(44,78,133)),('near-c',(128,96),(44,79,133)),('tie-a',(192,144),(166,33,77)),('tie-b',(192,144),(167,33,77))]:
    im=Image.new('RGB',size,color); d=ImageDraw.Draw(im); d.rectangle((8,8,size[0]//2,size[1]//2),fill=(211,188,55)); d.line((0,size[1],size[0],0),fill=(25,99,144),width=5); im.save(p/(name+'.png'))
if large:
    im=Image.new('RGB',(32,24),(44,155,60)); d=ImageDraw.Draw(im); d.rectangle((2,3,20,15),fill=(199,45,111)); im.save(p/'large-00000.png')
    for n in range(1,20006): shutil.copyfile(p/'large-00000.png',p/f'large-{n:05d}.png')
files=list(p.iterdir()); size=sum(x.stat().st_size for x in files); assert size<24*1024*1024
print(json.dumps({'count':len(files),'bytes':size,'exactGroups':groups+1+int(large),'expectedExactTrash':groups*2+22+(20005 if large else 0)}))
`;
const seedCode = String.raw`
import sqlite3,pathlib,sys,json
r=pathlib.Path(sys.argv[1]).resolve(); assert r.parent==pathlib.Path('D:/Ai/illuTag-refactor-work').resolve() and r.name.startswith('duplicates-1.5.')
db=sqlite3.connect(r/'data/illutag.sqlite'); c=sqlite3.connect(r/'data/cache/duplicates.sqlite')
gen=c.execute("select value from meta where key='published'").fetchone()[0]
db.execute("update images set is_favorite=1 where file_name glob 'exact-*-0.png'"); db.commit()
ids={name:(pk,ident) for pk,ident,name in db.execute('select image_pk,id,file_name from images')}
# Remove scanner-produced near groups so only deliberately constructed stars
# are considered below. Every exact group and scanned fingerprint is retained.
c.execute('delete from members where generation=? and group_no in (select group_no from groups where generation=? and kind=1)',(gen,gen))
c.execute('delete from groups where generation=? and kind=1',(gen,))
color=bytes([96]*48)
for name,p,w,h in [('near-a.png',0,512,384),('near-b.png',15,256,192),('near-c.png',255,128,96),('tie-a.png',0xffffffffffff0000,192,144),('tie-b.png',0xffffffffffff0000,192,144)]:
    pk=ids[name][0]; c.execute('update fingerprints set phash=?,dhash=?,color=?,width=?,height=?,low_info=0 where image_pk=?',(p.to_bytes(8,'little'),bytes(8),color,w,h,pk))
start=c.execute('select coalesce(max(group_no),0)+100 from groups').fetchone()[0]
for offset,names in enumerate([['near-b.png','near-a.png','near-c.png'],['near-a.png','near-b.png'],['tie-a.png','tie-b.png']]):
    group=start+offset; c.execute('insert into groups values(?,?,?,?,?)',(gen,group,1,ids[names[0]][0],len(names)))
    for i,name in enumerate(names):
        pk=ids[name][0]; c.execute('insert into members(generation,group_no,member_no,image_pk,public_hash,signature,phash_distance,dhash_distance,color_distance) select ?,?,?,image_pk,public_hash,signature,4,0,0 from fingerprints where image_pk=?',(gen,group,i,pk))
c.execute('update generations set near_groups=3 where generation=?',(gen,)); c.commit(); c.execute('pragma wal_checkpoint(truncate)'); db.close(); c.close()
print(json.dumps({'generation':str(gen),'ids':{k:v[1] for k,v in ids.items() if k.startswith(('near-','tie-'))}}))
`;
const snapshotCode = String.raw`
import sqlite3,pathlib,sys,json,hashlib
r=pathlib.Path(sys.argv[1]).resolve(); assert r.parent==pathlib.Path('D:/Ai/illuTag-refactor-work').resolve() and r.name.startswith('duplicates-1.5.')
db=sqlite3.connect((r/'data/illutag.sqlite').as_uri()+'?mode=ro',uri=True,timeout=10)
db.execute('attach database ? as dup',((r/'data/cache/duplicates.sqlite').as_uri()+'?mode=ro',))
gen=db.execute("select value from dup.meta where key='published'").fetchone()[0]
groups=db.execute("select g.group_no,g.total_members,sum(case when coalesce(i.trashed,0)=0 then 1 else 0 end) from dup.groups g join dup.members m on m.generation=g.generation and m.group_no=g.group_no join images i on i.image_pk=m.image_pk where g.generation=? and g.kind=0 group by g.group_no,g.total_members order by g.group_no",(gen,)).fetchall()
near=db.execute("select file_name,id,trashed,width,height from images where file_name like 'near-%' or file_name like 'tie-%' order by file_name").fetchall()
hashrows=db.execute('select id,content_hash from images order by id').fetchall()
fprows=db.execute('select image_pk,hex(signature),hex(sha),hex(phash),hex(dhash),hex(color) from dup.fingerprints order by image_pk').fetchall()
result={'schema':db.execute('pragma user_version').fetchone()[0],'images':db.execute('select count(*) from images').fetchone()[0],'trashed':db.execute('select count(*) from images where trashed=1').fetchone()[0],'customTags':db.execute('select count(*) from storage_image_user_custom_tags').fetchone()[0],'groups':groups,'near':near,'favoriteSurvivors':db.execute("select count(*) from images where is_favorite=1 and trashed=0 and file_name glob 'exact-*-0.png'").fetchone()[0],'contentHashDigest':hashlib.sha256(json.dumps(hashrows).encode()).hexdigest(),'fingerprintDigest':hashlib.sha256(json.dumps(fprows).encode()).hexdigest()}
db.close(); print(json.dumps(result))
`;
function snapshot() { return pythonRun(snapshotCode); }
function bulkSnapshot(operationId) {
  return pythonRun(String.raw`
import sqlite3,pathlib,sys,json
r=pathlib.Path(sys.argv[1]).resolve(); assert r.parent==pathlib.Path('D:/Ai/illuTag-refactor-work').resolve() and r.name.startswith('duplicates-1.5.')
c=sqlite3.connect((r/'data/operations/duplicate-management.sqlite').as_uri()+'?mode=ro',uri=True,timeout=10)
seed,stage=c.execute('select seed,stage from bulk_runs where operation=?',(sys.argv[2],)).fetchone()
keepers=[pk for pk, in c.execute('select image_pk from bulk_keepers where operation=? order by image_pk',(sys.argv[2],))]
c.close(); print(json.dumps({'seed':seed,'stage':stage,'keepers':keepers}))
`, [operationId]);
}
async function openSession() {
  session = await launch(executable, root, 9798, { resourceRoot });
  await session.page.waitForSelector(".app", { timeout: 30000 });
  const info = await session.invoke("data_directory_info_command");
  assert.equal(path.resolve(info.dataDir.replace(/^\\\\\?\\/, "")).toLowerCase(), data.toLowerCase());
  assert.equal(info.usingFallback, false);
}
async function closeSession() {
  if (!session) return;
  const closing = session; session = null; await closing.close();
  const diagnostic = closing.getDiagnostics(); report.sessions.push(diagnostic);
  assert.equal(diagnostic.killed, false, "Test process must close normally");
  assert.equal(diagnostic.exitCode, 0);
}
async function finish(id, expected = ["completed"]) {
  const waitStarted = performance.now(), stages = [];
  const until = waitStarted + (large ? 900000 : 240000);
  while (performance.now() < until) {
    const row = (await session.invoke("list_jobs")).find((j) => j.id === id);
    if (row?.request.duplicateOperationId && !stages.some(s => s.jobStage === row.stage)) {
      const status = await session.invoke("duplicate_operation_status", { operationId: row.request.duplicateOperationId });
      assert.ok(["snapshot", "planning", "applying", "undoSnapshot"].includes(status.stage), `Missing bulk phase: ${JSON.stringify(status)}`);
      assert.ok(status.stageProcessed >= 0 && status.stageTotal >= status.stageProcessed, "Invalid phase progress");
      stages.push({ jobStage: row.stage, stage: status.stage, processed: status.stageProcessed, total: status.stageTotal });
    }
    if (row && ["completed", "partial", "cancelled", "interrupted", "failed"].includes(row.state)) {
      assert.ok(expected.includes(row.state), JSON.stringify({ state: row.state, errors: row.errors }));
      report.jobs.push({ id: row.id, kind: row.request.kind, state: row.state, processed: row.processed, total: row.total, failures: row.failures, elapsedMs: row.updatedAt-Number(row.id.split('-')[0]), observedDurationMs: Math.round(performance.now()-waitStarted), stages });
      return row;
    }
    await sleep(40);
  }
  throw Error(`Task ${id} timed out`);
}
async function start(kind, duplicateOptions = null) {
  const job = await session.invoke("start_job", { request: { kind, thumbnails: false, sourceFolderId: null, duplicateOptions } });
  return finish(job.id);
}
async function prepare(mode) {
  const preview = await session.invoke("prepare_duplicate_action", { request: { generation, groupId: mode === "autoExactAll" ? "all-exact" : "all-near", mode, keepIds: [], protectedIds: [], imageIds: [], action: "trash", sourceOperationId: null } });
  assert.equal(preview.requiresSystemConfirmation, false);
  assert.equal(preview.action, "trash");
  return preview;
}
async function apply(preview, expected = ["completed"]) {
  const job = await session.invoke("apply_duplicate_action", { request: { operationId: preview.operationId, confirmed: true } });
  await finish(job.id, expected);
  return session.invoke("duplicate_operation_status", { operationId: preview.operationId });
}
async function results(operationId) {
  const rows = []; let cursor = null;
  do {
    const page = await session.invoke("duplicate_operation_results", { query: { operationId, cursor, limit: 100, state: "all" } });
    assert.ok(page.items.length <= 100); rows.push(...page.items); cursor = page.nextCursor;
  } while (cursor);
  return rows;
}
async function undo(operationId) {
  const job = await session.invoke("undo_duplicate_action", { operationId });
  await finish(job.id); assert.equal(snapshot().trashed, 0);
}
function assertPreserved(before, after = snapshot()) {
  for (const name of ["schema", "images", "customTags", "contentHashDigest", "fingerprintDigest"]) assert.equal(after[name], before[name], `${name} changed`);
  assert.equal(fs.readdirSync(assets).length, fixture.count, "Soft trash must retain original generated files");
}
async function showDuplicates() {
  await session.page.reload(); await session.page.waitForSelector(".app");
  await session.page.getByRole("navigation", { name: "图库导航" }).getByRole("button", { name: "重复检索", exact: true }).click();
  await session.page.locator(".duplicate-page").waitFor();
}
async function clickUiBulk(label) {
  const known = new Set((await session.invoke("list_jobs")).map((j) => j.id));
  await session.page.getByRole("button", { name: label, exact: true }).click();
  const until = performance.now() + 15000;
  while (performance.now() < until) {
    const job = (await session.invoke("list_jobs")).find((j) => j.request.kind === "duplicateManagement" && !known.has(j.id));
    if (job) {
      assert.ok(job.request.duplicateOperationId, "UI must create a resumable bulk operation");
      if (label === "整理全部完全相同图片") {
        await session.page.locator(".duplicate-operation-panel").waitFor();
        assert.equal(await session.page.locator(".duplicate-operation-panel").count(), 1, "Only one current task panel should be expanded");
        await session.page.waitForFunction(() => {
          const progress = document.querySelector(".duplicate-operation-panel progress");
          return progress && Number(progress.getAttribute("value")) > 0;
        }, null, { timeout: 10000 });
        report.activeUiText = await session.page.locator(".duplicate-operation-panel").innerText();
        assert.ok(!/prepared|正在汇总全部扫描结果/.test(report.activeUiText), "Old misleading status must be absent");
        await session.page.screenshot({ path: qaPath(path.join(logs, "duplicate-bulk-active-1280.png"), "active screenshot") });
      }
      await finish(job.id);
      return session.invoke("duplicate_operation_status", { operationId: job.request.duplicateOperationId });
    }
    await sleep(30);
  }
  throw Error("Single UI action did not start automatic bulk processing");
}
try {
  fixture = pythonRun(fixtureCode, [exactGroups, large]); report.fixture = fixture;
  await openSession();
  assert.equal((await session.invoke("get_bootstrap")).page.totalImageCount, 0);
  assert.equal((await session.invoke("list_jobs")).length, 0);
  await session.invoke("add_gallery_folder_command", { folderPath: assets });
  await start("scan");
  await start("duplicates", { sensitivity: "standard", forceRecompute: false, regroupOnly: false });
  await closeSession();
  const seeded = pythonRun(seedCode); generation = seeded.generation; report.nearFixture = seeded;
  await openSession();
  const before = snapshot();
  assert.equal(before.schema, 3); assert.equal(before.images, fixture.count); assert.equal(before.groups.length, fixture.exactGroups);
  assert.ok(before.groups.some((g) => g[1] === 23));
  if (large) assert.ok(before.groups.some((g) => g[1] === 20006));
  const loaded = await session.invoke("query_duplicate_groups", { query: { generation, kind: "all", cursor: null, limit: 20 } });
  assert.equal(loaded.groups.length, 20); assert.ok(loaded.nextCursor);
  report.checks.push("Fresh schema 3 synthetic library; more than 1,000 exact groups; only first 20 groups loaded");

  const exact = await prepare("autoExactAll");
  // Start and pause in the same WebView hop, exercising the real JobRuntime.
  const paused = await session.page.evaluate(async ({ operationId }) => {
    const invoke = (command, args = {}) => window.__TAURI_INTERNALS__.invoke(command, args);
    const job = await invoke("apply_duplicate_action", { request: { operationId, confirmed: true } });
    const until = performance.now() + 15000;
    while (performance.now() < until) {
      const row = (await invoke("list_jobs")).find((j) => j.id === job.id);
      if (row?.state === "running") {
        await invoke("control_job", { id: job.id, action: "pause" });
        return (await invoke("list_jobs")).find((j) => j.id === job.id);
      }
      if (["completed", "partial", "failed"].includes(row?.state)) throw Error("Bulk completed before pause test");
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    throw Error("Bulk job did not become running");
  }, { operationId: exact.operationId });
  assert.equal(paused.state, "paused");
  await session.invoke("control_job", { id: paused.id, action: "cancel" });
  await finish(paused.id, ["cancelled", "partial"]);
  const interruptedPlan = bulkSnapshot(exact.operationId);
  const alreadySucceeded = (await results(exact.operationId)).filter((r) => r.state === "succeeded").map((r) => r.imageId);
  await closeSession(); await openSession();
  assert.equal(bulkSnapshot(exact.operationId).seed, interruptedPlan.seed, "Exact randomness must persist across restart");
  const exactDone = await apply(exact);
  const completedPlan = bulkSnapshot(exact.operationId);
  assert.equal(completedPlan.seed, interruptedPlan.seed);
  for (const keeper of interruptedPlan.keepers) assert.ok(completedPlan.keepers.includes(keeper), "Previously selected keeper changed during resume");
  assert.equal(exactDone.succeeded, fixture.expectedExactTrash);
  const afterExact = snapshot();
  assert.equal(afterExact.trashed, fixture.expectedExactTrash);
  assert.ok(afterExact.groups.every((g) => g[2] === 1), "Every exact group must retain exactly one image");
  assert.ok(afterExact.favoriteSurvivors > 20 && afterExact.favoriteSurvivors < exactGroups - 20, "Random keeper must not always favor metadata");
  assert.ok(afterExact.near.every((r) => r[2] === 0));
  const exactResults = await results(exact.operationId);
  assert.equal(exactResults.filter((r) => r.state === "succeeded").length, fixture.expectedExactTrash);
  for (const imageId of alreadySucceeded) assert.ok(exactResults.some((r) => r.imageId === imageId && r.state === "succeeded"));
  assertPreserved(before, afterExact);
  report.checks.push("One exact bulk operation pauses/cancels/restarts/resumes; all groups beyond page/1,000 limit retain one random image; metadata preference removed");

  await apply(await prepare("autoExactAll"));
  assert.equal(snapshot().trashed, afterExact.trashed);
  assert.deepEqual(snapshot().groups, afterExact.groups);
  report.checks.push("Repeated exact bulk operation is idempotent");
  await undo(exact.operationId); assertPreserved(before);
  report.checks.push("One bulk undo restores every successfully soft-trashed exact image with metadata and source files retained");

  const near = await prepare("autoNearAll"), nearDone = await apply(near);
  assert.equal(nearDone.succeeded, 2, "Only B and one equal-area tie candidate may be recycled");
  const afterNear = snapshot(), byName = Object.fromEntries(afterNear.near.map((r) => [r[0], r]));
  assert.equal(byName["near-a.png"][2], 0); assert.equal(byName["near-b.png"][2], 1); assert.equal(byName["near-c.png"][2], 0);
  assert.equal(byName["tie-a.png"][2] + byName["tie-b.png"][2], 1);
  assert.ok(afterNear.groups.every((g) => g[2] === g[1]), "Near mode must exclude exact groups");
  const firstTie = afterNear.near.find((r) => r[0].startsWith("tie-") && r[2] === 0)[1];
  const nearResults = await results(near.operationId);
  assert.equal(nearResults.filter((r) => r.state === "succeeded").length, 2);
  assert.ok(nearResults.every((r) => !r.fileMoved));
  assertPreserved(before, afterNear);
  report.checks.push("Near mode retains highest area; overlapping A-B-C stars preserve C without a direct A match; equal-area stable keeper; only direct targets soft-trash");
  await apply(await prepare("autoNearAll")); assert.equal(snapshot().trashed, 2);
  await undo(near.operationId);
  const nearAgain = await prepare("autoNearAll"); await apply(nearAgain);
  assert.equal(snapshot().near.find((r) => r[0].startsWith("tie-") && r[2] === 0)[1], firstTie);
  await undo(nearAgain.operationId); assertPreserved(before);
  report.checks.push("Near bulk repeat is idempotent; undo and rerun retain the same equal-area keeper");

  if (!flags.includes("--skip-ui")) {
    await showDuplicates();
    const controls = await session.page.locator(".duplicate-page button").allTextContents();
    assert.ok(!controls.some((s) => /键盘审核|多图审核|待审核|回收选中|选择其他|保留选中/.test(s)), "Manual review workflow must be absent");
    await session.page.getByRole("button", { name: "整理全部完全相同图片", exact: true }).waitFor();
    await session.page.getByRole("button", { name: "整理全部近似图片", exact: true }).waitFor();
    for (const width of [500, 1280]) {
      await session.page.setViewportSize({ width, height: 850 });
      assert.equal(await session.page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
      const cards = await session.page.locator(".duplicate-action-card").all();
      assert.equal(cards.length, 2);
      const first = await cards[0].boundingBox(), second = await cards[1].boundingBox();
      assert.ok(first && second);
      if (width === 1280) assert.ok(Math.abs(first.y-second.y)<2 && second.x>first.x, "Desktop actions must align in two columns");
      else {
        assert.ok(second.y>first.y+first.height-2, "Narrow actions must stack");
        const heading = await session.page.locator(".duplicate-page .page-header > div").boundingBox();
        assert.ok(heading && Math.abs(heading.x-first.x)<2, "Narrow page heading must align with content");
      }
      assert.equal(await session.page.locator("details.duplicate-secondary[open]").count(), 0, "Advanced settings and history must start collapsed");
      await session.page.screenshot({ path: qaPath(path.join(logs, `duplicate-bulk-${width}.png`), "screenshot") });
    }
    report.uiButtons = controls;
    const uiExact = await clickUiBulk("整理全部完全相同图片");
    assert.equal(uiExact.succeeded, fixture.expectedExactTrash);
    assert.ok(snapshot().groups.every((g) => g[2] === 1));
    await undo(uiExact.operationId);
    // Let the page consume the completed undo event before starting near mode.
    await showDuplicates();
    const uiNear = await clickUiBulk("整理全部近似图片");
    assert.equal(uiNear.succeeded, 2); await undo(uiNear.operationId); assertPreserved(before);
    report.checks.push("Real UI starts each all-results mode with one click and no review/confirmation; 500/1280 CSS widths fit; screenshots retained");
  }

  // File changes after preparation must never be accepted using stale SHA.
  const stale = await prepare("autoNearAll"), changed = path.join(assets, "tie-b.png"), originalBytes = fs.readFileSync(changed);
  fs.appendFileSync(changed, Buffer.from("synthetic-source-change"));
  const staleDone = await apply(stale, ["completed", "partial"]);
  assert.equal((await session.invoke("get_image_detail", { imageId: seeded.ids["tie-b.png"] })).image.trashed, false);
  if (staleDone.succeeded > 0) await undo(stale.operationId);
  fs.writeFileSync(changed, originalBytes);
  // Restoring bytes changes mtime: only refresh this synthetic pair by a new
  // scan in subsequent runs; the current acceptance leaves all originals live.
  assert.equal(snapshot().trashed, 0); assertPreserved(before);
  report.checks.push("A synthetic source changed after prepare is excluded from automatic trash; remaining successes undo safely");

  report.passed = true;
} catch (error) {
  failure = error; report.error = error?.stack || String(error);
} finally {
  try { await closeSession(); } catch (error) { failure ||= error; report.passed = false; report.closeError = error.stack || String(error); }
  report.durationMs = Math.round(performance.now() - began);
  fs.writeFileSync(qaPath(path.join(logs, "duplicate-bulk-native-qa.json"), "report"), JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ passed: report.passed, exeSha256: report.exeSha256, fixture: report.fixture, checks: report.checks.length, durationMs: report.durationMs, error: report.error, report: path.join(logs, "duplicate-bulk-native-qa.json") }));
}
if (failure) throw failure;
