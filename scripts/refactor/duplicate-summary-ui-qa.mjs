import { launch, isolatedPath, sleep } from "./native-session.mjs";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";

// EXE NEW_ROOT RESOURCE_ROOT. UI-only native smoke: six abstract fixtures,
// scan + duplicate search, disclosure inspection. No management,
// trash/undo, cache clear, source deletion, or deployed app is used.
const [exeArgument, rootArgument, resourceArgument, ...extra] = process.argv.slice(2);
assert.equal(extra.length, 0, "Unexpected arguments");
const fence = path.resolve("D:/Ai/illuTag-refactor-work");
function fenced(value, label) {
  const target = isolatedPath(value, label);
  assert.ok(target.toLowerCase().startsWith((fence + path.sep).toLowerCase()), `${label} escapes QA fence`);
  let ancestor = target;
  while (!fs.existsSync(ancestor)) ancestor = path.dirname(ancestor);
  const real = fs.realpathSync(ancestor).toLowerCase();
  assert.ok(real === fence.toLowerCase() || real.startsWith((fence + path.sep).toLowerCase()), `${label} resolves outside QA fence`);
  return target;
}
const executable = fenced(exeArgument, "candidate"), root = fenced(rootArgument, "fresh UI root");
assert.equal(path.dirname(root).toLowerCase(), fence.toLowerCase());
assert.match(path.basename(root), /^duplicates-1\.5\.2-ui-[\w.-]+$/i);
assert.ok(!fs.existsSync(root), "UI smoke root must be completely new");
assert.ok(resourceArgument && path.isAbsolute(resourceArgument), "Explicit absolute resource root required");
const resourceRoot = fs.realpathSync(resourceArgument), python = path.join(resourceRoot, "runtime/python/python.exe");
assert.ok(fs.statSync(executable).isFile() && fs.statSync(python).isFile(), "Candidate/runtime missing");
assert.notEqual(process.env.ILLUTAG_READ_ONLY, "1");
const data = path.join(root, "data"), assets = path.join(root, "assets"), logs = path.join(root, "logs"), temporary = path.join(root, "temp");
for (const directory of [data, assets, logs, temporary]) fs.mkdirSync(fenced(directory, "output"), { recursive: true });
fs.writeFileSync(path.join(data, "window-state.json"), JSON.stringify({ x: 24, y: 24, width: 1312, height: 1000, isMaximized: false, isFullscreen: false }));
const report = { passed: false, syntheticOnly: true, uiOnly: true, executable, exeSha256: createHash("sha256").update(fs.readFileSync(executable)).digest("hex"), startedAt: new Date().toISOString(), checks: [], jobs: [], layouts: [], screenshots: [], sessions: [] };
let session, failure;
const began = performance.now();
function pythonRun(code, args = []) {
  return JSON.parse(execFileSync(python, ["-B", "-c", code, root, ...args], {
    cwd: root, windowsHide: true, timeout: 30000, maxBuffer: 1024 * 1024,
    env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1", PYTHONPYCACHEPREFIX: path.join(root, "pycache"), TEMP: temporary, TMP: temporary },
  }).toString().trim());
}
const fixtureCode = String.raw`
from PIL import Image,ImageDraw
import pathlib,sys,shutil,json
r=pathlib.Path(sys.argv[1]).resolve(); assert r.parent==pathlib.Path('D:/Ai/illuTag-refactor-work').resolve() and r.name.startswith('duplicates-1.5.2-ui-')
p=r/'assets'; im=Image.new('RGB',(128,96)); pixels=im.load()
for y in range(96):
 for x in range(128): pixels[x,y]=(30+x,45+y,165-x//2+y//3)
d=ImageDraw.Draw(im); d.ellipse((7,8,47,48),fill=(231,199,72)); d.polygon([(14,85),(69,24),(119,87)],fill=(34,117,87)); d.rectangle((79,10,118,26),fill=(182,60,92))
im.save(p/'abstract-original.png')
for name in ['abstract-copy-a.png','abstract-copy-b.png']: shutil.copyfile(p/'abstract-original.png',p/name)
im.resize((64,48),Image.Resampling.LANCZOS).save(p/'abstract-small.png')
Image.eval(im,lambda x:255-x).save(p/'abstract-negative.png'); shutil.copyfile(p/'abstract-negative.png',p/'abstract-negative-copy.png')
files=list(p.iterdir()); size=sum(f.stat().st_size for f in files); assert len(files)==6 and size<128*1024
print(json.dumps({'count':len(files),'bytes':size}))
`;
const inspectionCode = String.raw`
import pathlib,sqlite3,sys,json,hashlib
r=pathlib.Path(sys.argv[1]).resolve(); assert r.parent==pathlib.Path('D:/Ai/illuTag-refactor-work').resolve() and r.name.startswith('duplicates-1.5.2-ui-')
c=sqlite3.connect((r/'data/illutag.sqlite').as_uri()+'?mode=ro',uri=True)
rows=c.execute('select id,trashed,content_hash from images order by id').fetchall(); result={'images':len(rows),'trashed':sum(x[1] for x in rows),'digest':hashlib.sha256(json.dumps(rows).encode()).hexdigest()}; c.close(); print(json.dumps(result))
`;
const summaries = [
  { selector: ".duplicate-scan-settings > summary", label: "检索设置" },
  { selector: ".duplicate-scan-details > summary", label: "检索详情与维护" },
  { selector: ".duplicate-inspection > summary", label: "查看扫描结果（可选）" },
];
async function finish(job) {
  const until = performance.now() + 60000;
  while (performance.now() < until) {
    const row = (await session.invoke("list_jobs")).find((j) => j.id === job.id);
    if (row && ["completed", "partial", "cancelled", "interrupted", "failed"].includes(row.state)) {
      assert.equal(row.state, "completed", JSON.stringify(row.errors));
      report.jobs.push({ id: row.id, kind: row.request.kind, state: row.state, processed: row.processed, total: row.total });
      return;
    }
    await sleep(30);
  }
  throw Error(`Fixture job ${job.id} timed out`);
}
async function start(kind, duplicateOptions = null) {
  assert.ok(["scan", "duplicates"].includes(kind), "Only fixture scan/search are permitted");
  const job = await session.invoke("start_job", { request: { kind, thumbnails: false, sourceFolderId: null, duplicateOptions } });
  await finish(job);
}
async function screenshot(name) {
  const destination = fenced(path.join(logs, name), "screenshot");
  await session.page.screenshot({ path: destination });
  report.screenshots.push(destination);
}
async function layout(width, mode) {
  const rows = await session.page.evaluate((expected) => expected.map(({ selector, label }) => {
    const summary = document.querySelector(selector);
    if (!summary) throw Error(`Missing summary: ${selector}`);
    const span = summary.querySelector(":scope > span");
    if (!span) throw Error(`Summary title must have a direct span: ${selector}`);
    if (span.textContent.trim() !== label) throw Error(`Unexpected title: ${span.textContent}`);
    const rect = summary.getBoundingClientRect(), labelRect = span.getBoundingClientRect();
    const style = getComputedStyle(summary), arrow = getComputedStyle(summary, "::before");
    const range = document.createRange(); range.selectNodeContents(span);
    const textRect = range.getBoundingClientRect();
    const arrowX = rect.x + parseFloat(style.paddingLeft) + parseFloat(style.borderLeftWidth);
    return { selector, label, open: summary.parentElement.open, summaryX: rect.x, labelX: labelRect.x, textX: textRect.x, arrowX, arrowWidth: parseFloat(arrow.width), arrowTransform: arrow.transform, gap: labelRect.x - arrowX - parseFloat(arrow.width), expectedGap: parseFloat(style.columnGap), textAlign: getComputedStyle(span).textAlign, flexGrow: getComputedStyle(span).flexGrow };
  }), summaries);
  const base = rows[0].labelX;
  for (const row of rows) {
    assert.ok(Math.abs(row.labelX - base) <= 1, `${width}/${mode}: title is not aligned: ${JSON.stringify(row)}`);
    assert.ok(Math.abs(row.textX - row.labelX) <= 1, `${width}/${mode}: title text is not left aligned`);
    assert.ok(Number.isFinite(row.gap) && Math.abs(row.gap - row.expectedGap) <= 1, `${width}/${mode}: arrow/title gap differs`);
    assert.ok(Math.abs(row.gap - rows[0].gap) <= 1, `${width}/${mode}: inconsistent arrow gaps`);
    assert.equal(row.flexGrow, "1");
    assert.equal(row.open, mode === "expanded");
  }
  assert.equal(await session.page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `${width}/${mode}: horizontal overflow`);
  report.layouts.push({ width, mode, rows });
}
try {
  report.fixture = pythonRun(fixtureCode);
  session = await launch(executable, root, 9802, { resourceRoot });
  await session.page.waitForSelector(".app", { timeout: 30000 });
  const info = await session.invoke("data_directory_info_command");
  assert.equal(path.resolve(info.dataDir.replace(/^\\\\\?\\/, "")).toLowerCase(), data.toLowerCase());
  assert.equal(info.usingFallback, false);
  assert.equal((await session.invoke("get_bootstrap")).page.totalImageCount, 0);
  assert.equal((await session.invoke("list_jobs")).length, 0);
  await session.invoke("add_gallery_folder_command", { folderPath: assets });
  await start("scan");
  await start("duplicates", { sensitivity: "standard", forceRecompute: false, regroupOnly: false });
  const status = await session.invoke("duplicate_status");
  assert.equal(status.state, "ready"); assert.ok(status.exactGroups >= 2);
  const baseline = pythonRun(inspectionCode);
  assert.equal(baseline.images, 6); assert.equal(baseline.trashed, 0);
  await session.page.reload(); await session.page.waitForSelector(".app");
  await session.page.getByRole("navigation", { name: "图库导航" }).getByRole("button", { name: "重复检索", exact: true }).click();
  await session.page.locator(".duplicate-inspection > summary").waitFor();
  assert.equal(await session.page.locator(".duplicate-history").count(), 0, "Fresh UI smoke must not create management history");
  report.checks.push("Six new abstract files scanned/searched in fresh isolated schema 3; no management history created");
  for (const width of [500, 1280]) {
    await session.page.setViewportSize({ width, height: 1100 });
    for (const { selector } of summaries) {
      const summary = session.page.locator(selector);
      if (await summary.evaluate((element) => element.parentElement.open)) await summary.click();
    }
    await session.page.locator(".duplicate-secondary-area").scrollIntoViewIfNeeded();
    await layout(width, "folded");
    await screenshot(`duplicate-summary-${width}-folded.png`);
    for (const { selector } of summaries) await session.page.locator(selector).click();
    await session.page.locator(".duplicate-inspection .duplicate-group").first().waitFor();
    await session.page.locator(".duplicate-secondary-area").scrollIntoViewIfNeeded();
    await layout(width, "expanded");
    await screenshot(`duplicate-summary-${width}-expanded.png`);

  }
  report.checks.push("At 500/1280 CSS px, all three visible disclosure titles and arrow gaps align in folded/expanded states");
  assert.deepEqual(pythonRun(inspectionCode), baseline);
  const jobs = await session.invoke("list_jobs");
  assert.equal(jobs.length, 2);
  assert.ok(jobs.every((j) => ["scan", "duplicates"].includes(j.request.kind) && j.state === "completed"));
  assert.equal(fs.existsSync(path.join(data, "operations/duplicate-management.sqlite")), false);
  assert.equal(fs.readdirSync(assets).length, report.fixture.count);
  report.checks.push("UI inspection leaves images live, source files present, no trash/undo/clear operations or extra jobs");
  report.passed = true;
} catch (error) {
  failure = error; report.error = error?.stack || String(error);
} finally {
  if (session) {
    try {
      await session.close();
      const diagnostic = session.getDiagnostics(); report.sessions.push(diagnostic);
      assert.equal(diagnostic.killed, false); assert.equal(diagnostic.exitCode, 0);
    } catch (error) { failure ||= error; report.passed = false; report.closeError = error.stack || String(error); }
  }
  report.durationMs = Math.round(performance.now() - began);
  const destination = fenced(path.join(logs, "duplicate-summary-ui-qa.json"), "report");
  fs.writeFileSync(destination, JSON.stringify(report, null, 2));
  console.log(JSON.stringify({passed:report.passed,exeSha256:report.exeSha256,checks:report.checks.length,durationMs:report.durationMs,error:report.error,report:destination}));
}
if (failure) throw failure;
