# illuTag 启动性能诊断与优化报告

日期：2026-09-25
目标部署：`D:\Ai\illuTag-danbooru-tag-query`（`illutag.sqlite`）
源码：`D:\Ai\illuTag-src`（无单实例插件；`illutag.exe` 每次启动都会无条件跑启动清理）

---

## 0. 结论摘要（TL;DR）

- **启动主瓶颈已定位并修复：`main.rs` setup() 里同步预热 CLIP 向量缓存（H1）。**
  809,231 × 512 f32 ≈ **1.66 GB** 的全量读发生在事件循环启动前，阻塞首屏，占热缓存 setup 的 **98%**。
- **P0 补丁（已实施并验证，独立可回滚）：** 删除 setup 中的 eager warmup，改为首次语义/自然语言/以图搜图时惰性加载；同时把自然语言扫描命令改到后台线程，避免首次使用时主线程卡死。
- **热缓存实测（pre vs opt，P50）：**
  | 指标 | 优化前 | 优化后 | 降幅 |
  |---|---|---|---|
  | setup 阻塞（`[database]`→`list_library begin`） | 3,684 ms | 70 ms | **−98.1%** |
  | 进程启动→可交互（SegmentedMasonry 首渲染） | 5,408 ms | 1,764 ms | **−67.4%** |
  | 内存峰值 | 2,776 MB | 970 MB | **−65.1%** |
  | 结束时工作集 | 2,067 MB | 295 MB | **−85.7%** |
  | 启动后 30s CPU | ~31 s | ~27.5 s | −11% |
- **硬指标（冷启动 P50 相对基线降 ≥50%）**：热缓存已降 67%，冷缓存基线待用户协作采集（脚本已交付），静态上界估算降幅同量级（H1 占冷 setup ~15s，见 §5）。**绝对上限 SSD 可交互 ≤3s 在热缓存下已满足（1.76s），冷缓存待验证。**
- **意外发现（H4 修正）：** 每次启动无条件运行的“启动清理”扫描 809K 行 DB + 809K 次文件 stat，实测耗时 **~59s**、删除 0 张，并在结束时清空 library 缓存触发**第二次 list_library（~100MB 重载）**。这是仅次于 H1 的启动后台争用源。

---

## 1. 环境与规模（简报数字作废，以下为实测）

用 Python sqlite3 只读打开部署库（`git`/代码均为只读，未写库）：

| 项 | 简报（9/9 快照） | 实测（2026-09-25） |
|---|---|---|
| illutag.sqlite | 4.7 GB | **29.25 GB**（page_size=4096, page_count=7,142,114, freelist=0） |
| images | 195,103 | **809,250**（全部 `source='library'`，trashed=18） |
| image_auto_tags | 9.73 M | **41,255,938**（模型 `wd-swinv2-tagger-v3`） |
| image_user_folders | 195 K | **809,249** |
| image_clip_embeddings | 0 | **809,231**（dim=512，blob 均 2048 B，`cn_clip_vit_base_patch16/onnx_v1`） |
| image_thumbnails | 0 | **0**（`thumbs\library` 0 文件；`thumbnailCacheEnabled` 默认 false） |
| user_folders | — | 15 |
| 库大小/内存比 | — | 29 GB DB ≫ 可用页缓存，冷/热差异极大 |

`images.id` 即完整文件路径（如 `\\?\D:\Ai\warehouse_18\image\...\x.jpg`，均长约 72 字符），这是后文 IPC 载荷膨胀的根因。

---

## 2. 采集方法

- **工具**：`scripts/startup-perf/harness.mjs`（Node ≥22）启动 exe，逐行给 Rust stderr 打上进程相对时间戳；通过 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` + CDP 抓取前端 console 的 `[startup-prof]` 日志；结束后取 `WorkingSet64 / PeakWorkingSet64 / CPU`。
- **聚合**：`scripts/startup-perf/aggregate.py` 输出 P50/P90。
- **跑法**：先优雅关闭用户正在运行的实例（PID 23632；`taskkill` 未生效后 `/F`），确认无残留 `python.exe`；然后 `base1..5`（旧 exe）、`pre1..3`（加埋点、行为不变）、`opt1..5`（P0）。每次 30s，force kill。
- **热/冷**：本轮全部为**热缓存（warm）**——上一条进程刚读过同一批页。`base1` 因前序脚本把 CLIP 页挤出缓存而呈**半冷**，恰好给出冷/热差异样本。真冷启动需重启或清 OS 缓存，已交付 `collect-cold-start.cmd` 由用户执行，**本报告不编造冷数据**。
- **指标定义**（沿用简报拍板版）：
  - 首帧 = 窗口可见（WebView2 第一帧）。外部可测的近似为 setup 中 `window.show()`（`setup_window_shown_ms`）。
  - 可交互 = SegmentedMasonry 首渲染完成且可视区 `first_img_events` 覆盖完毕（harness 记录 `SegmentedMasonry mounted_ms` 与 `img_nodes`）。**不**用 5000 条 img load 全齐当可交互。

---

## 3. 启动阶段模型（热缓存，优化前）

以进程启动为 0（`pre1..3` P50）：

```
0ms            exe 启动
~330ms         [database] using ...                 (db_ready_ms)
~339ms         setup_window_shown_ms=9              ← 窗口已 show（首帧窗口级）
~3960ms        setup_total_ms=3630
                 ├─ warmup_clip_vector_cache_ms = 3617   ← H1，98% of setup
                 └─ resume_incomplete_scan_ms   = 1      ← 可忽略
~4020ms        list_library_command begin
~4540ms        load_store total_ms=522
                 ├─ images_ms=180  (COUNT 809K + 5000 行分页，含索引)
                 └─ image_folders_ms=337 (809K 行读 + 排序 + 建 Vec)
~4610ms        list_library_from_state total_ms=588
~4700ms        list_library_command total_ms=623
~5393ms        invoke_return (前端)                ← IPC/序列化/反序列化 ~1393ms
~5408ms        SegmentedMasonry mounted (可交互)  img_nodes=22
~10417ms       first_img_events (5s 窗口，22/22 loaded)
peak 2776MB, ws 2067MB, CPU(30s) ~31s
后台：startup-cleanup 约 +59s 完成（removed 0），随后清空 library 缓存 → 触发第二次 list_library
```

要点：
- **窗口级“首帧”其实很早（~339ms）**，但 **WebView2 内容/JS 在 ~3.96s 前完全跑不起来**——`index.html inline_head_ms=15319`（base1 冷样本）证明主线程被 setup 全程占据，形成白屏段。P0 后 `inline_head_ms` 降到 ~330ms。
- 可交互热缓存 5.4s，其中 H1 占 3.6s；剩余 ~1.8s 里 load_store 0.52s、IPC 1.4s。

---

## 4. Top 瓶颈

| # | 瓶颈 | 证据 | 成本 |
|---|---|---|---|
| 1 | **H1 同步 CLIP 预热** | `warmup_clip_vector_cache_ms=3617`（热）/ 半冷 `setup_block=15322ms`；1.66 GB 全量读 + HashMap 构建 | 热 3.6s / 冷 ~15s，峰值内存 +1.8GB |
| 2 | **启动清理后台争用** | 实测 ~59s 完成、`removed 0`；扫 809K 行 + 809K stat；结束清库缓存→第二次 list_library | ~59s 后台 CPU/IO；第二次 ~100MB 重载 |
| 3 | **list_library 载荷（809K assignments）** | `image_folders_ms=337`（冷半样本 1963ms）+ `ipc_roundtrip=1393ms`（Rust 623ms，其余为 ~100MB JSON 序列化/传/proxy） | 热 ~1.1s / 冷更大；webview 堆 ~150MB+ |
| 4 | `count_gallery_images` | `images_ms=180`（含 COUNT+分页） | 热 0.18s，**非**瓶颈 |
| 5 | 5000 条元数据 computed/渲染 | `renderedLayoutItems first_ms=0.2`，`img_nodes=22`，`galleryLayout` 首算 <0.3ms | 可忽略 |
| 6 | WebView2 初始化 / exe 加载 | `db_ready≈330ms`、`inline_head≈330ms` | ~0.3s，**排除** |

---

## 5. H1–H6 逐条证实/证伪

- **H1（升级为 P0）：证实，且为头号瓶颈。**
  `main.rs:1944` 原先 `let _ = warmup_clip_vector_cache(&app_state);` 在 `app.manage()` 之前同步执行。`ensure_clip_vector_cache_loaded` 读 `image_clip_embeddings` JOIN `images`，加载全部 809,231 条 512-d 向量。热 3617ms；`base1` 半冷 setup 15322ms。Python 单独验证：热读+解码 809K 向量 3.15s / 1.66GB；冷盘按 SSD 500MB/s 下界 ~3.3s、HDD 更久，与 15s 量级吻合。
  同类懒加载路径 `ensure_atmosphere_signature_cache_loaded` / `ensure_color_signature_cache_loaded` 也仅在首次使用时触发，且两表 0 行，启动无成本。
- **H2：部分证实。** `count_gallery_images`（809K 行）实测含在 `images_ms=180` 内，**不是**“4.7GB COUNT”级别的开销；`open_database` 每命令重连 `open_db_ms≈1ms`，可忽略。真正成本是 `load_image_folder_assignments`（809K 行，热 337ms / 半冷 1963ms）及其 ~100MB JSON → IPC 1393ms。**结论：H2 的成本在 assignments 载荷，而非 COUNT。**
- **H3：证伪。** 5000 条元数据的 computed/反序列化不是瓶颈：`images_ms=180`、`renderedLayoutItems first_ms=0.2`、`img_nodes=22`。5000→500/1000 无必要，符合“由数据决定”。
- **H4：修正后证实。** `thumbnailCacheEnabled` 默认 `false`（`useAppSettings.ts:30`，仅 localStorage='true' 才开），`image_thumbnails=0`、`thumbs/library` 空 → 缩略图生成**未**运行；`autoScanOnStartup` 默认 false。但 `App.vue onMounted` 无条件 `void startStartupCleanup()`，后端 `cleanup_missing_library_images_batched` 扫全表 + `Path::exists()` 每张原图，实测 **~59s、removed 0**，且完成时 `*library_cache=None`，观测到随后的第二次 `list_library_command begin`。这是真实的启动后 IO/CPU 争用与二次全量重载源。
- **H5：证伪（就启动而言）。** `sync_tag_dictionary_from_source_if_changed` 仅在扫描/打标路径被调用（`library.rs:4501/4643/4845/6408`），启动链不触发；`app_meta.tag_dictionary_source_signature` 已存在，不会误触发 xlsx 解析。
- **H6：证伪。** `db_ready≈330ms`、`setup_window_shown≈9ms`、P0 后 `inline_head≈330ms`；WebView2 初始化与 exe 加载合计 ~0.3s，非瓶颈。

---

## 6. 优化清单（按收益/风险排序）

| 项 | 收益 | 风险 | 成本 | 回滚 | 状态 |
|---|---|---|---|---|---|
| **P0 惰性化 CLIP 缓存 + NL 扫描命令转后台** | 热可交互 −3.6s（−67%）、内存 −1.8GB、冷 ~−15s | 低（3 个既有 `ensure_` 站点已覆盖） | 1 文件、~15 行 | 恢复 `warmup_clip_vector_cache` 调用即可 | **已实施/已验证** |
| P1 list_library 初始载荷去掉 `image_folders`，新增 `list_image_folder_assignments_command` 按需拉取 | 热 ~−1.1s（Rust 337ms + IPC ~750ms）、webview 堆 −150MB；冷更大 | 中：文件夹角标/scope 在拉取前为空 → **改变启动行为** | 后端新命令 + 前端 1 处按需加载 | 恢复 `list_library` 返回全量 assignments | 待批准（见 §9） |
| P2 启动清理门槛化（app_meta 记录 last_cleanup_at，如 ≥24h 才跑；或首屏后调度） | 每启动省 ~59s 后台 IO/CPU；消除第二次 100MB 重载 | 中：缺失图片清理时机改变 | 后端 ~10 行 | 去掉门槛判断 | 待批准 |
| P3 去掉 `load_image_folder_assignments` 的 `ORDER BY assigned_at DESC` | 不确定（Rust 热 337ms，Python 有/无序差异在噪声内） | 中：导出清单顺序可能变化 | 1 行 | 恢复 ORDER BY | **不推荐**（无数据支撑） |
| P4 首屏后低优先级后台预热 CLIP 缓存（保留首次搜索体感） | 首次搜索免 ~3.6s 冷加载 | 中：与首屏/图片加载争 IO | ~15 行 + 调度 | 删除后台任务 | 备选（若在意首次搜索延迟） |

> 已按“单点改动”执行：本轮**只落地 P0**。P1/P2 影响可观测行为，需你确认后再做，避免违反“行为不变”。

---

## 7. P0 补丁（独立可回滚）

文件：`src-tauri/src/main.rs`（唯一源改动；`Cargo.toml` 仅行尾差异，非本次改动）

```diff
@@ use illutag_core::library::{
-    warmup_clip_vector_cache,
@@ #[tauri::command]
-fn start_natural_language_scan_command(state: State<AppState>) -> Result<bool, String> {
-    start_natural_language_scan(&state)
+async fn start_natural_language_scan_command(app: tauri::AppHandle) -> Result<bool, String> {
+    // 缓存改为惰性加载后，首次启动可能触发 1.66GB 全量读；放到后台线程避免主线程卡顿。
+    off_main(app, |state| start_natural_language_scan(&state)).await
 }
@@ .setup(|app| {
+            let setup_started = Instant::now();
...
+            eprintln!("[startup-prof] setup_window_shown_ms={}", setup_started.elapsed().as_millis());
...
-            let _ = warmup_clip_vector_cache(&app_state);
+            // P0: 启动路径不再同步预热点 CLIP 向量缓存（809K×512 f32 ≈ 1.66GB）。
+            // 首次语义/自然语言/以图搜图时由 ensure_clip_vector_cache_loaded 惰性加载。
+            // 回滚：恢复此处的 warmup_clip_vector_cache 调用。
+            let resume_scan_started = Instant::now();
             if let Err(error) = resume_incomplete_library_scan(&app_state) { ... }
+            eprintln!("[startup-prof] resume_incomplete_scan_ms={}", ...);
             app.manage(app_state);
+            eprintln!("[startup-prof] setup_total_ms={}", ...);
```

**为什么安全**：`ensure_clip_vector_cache_loaded` 在 3 个使用点已被调用（`library.rs:3404` 自然语言扫描、`3601` 自然语言搜索、`3739` 以图搜图），且带双检锁；其余读写（`remove/upsert/invalidate`）均 `if Some` 容错。删除 eager warmup 不改变任何公开 API。
**影响面/回滚**：首次语义/自然语言/以图搜图将现场加载缓存（热 ~3.6s / 冷 ~15s），但不再卡首屏；NL 扫描命令已转后台线程，UI 不会“未响应”。回滚即恢复那行调用。
**埋点保留**：新增 `setup_window_shown_ms / resume_incomplete_scan_ms / setup_total_ms`（既有的 `load_store` 分阶段与前端 `[startup-prof]` 全部保留）。

---

## 8. 验证（热缓存，P50/P90）

| 指标 | 基线（pre1–3，P50） | P0（opt1–5，P50） | 变化 |
|---|---|---|---|
| setup_window_shown_ms | 9 | 9 | — |
| setup_total_ms | 3,630 | 12 | −99.7% |
| setup 阻塞（db→list_library begin） | 3,684 | 70 | −98.1% |
| warmup_clip_vector_cache_ms | 3,617 | 不执行 | — |
| rust load_store | 526 | 522 | ~0 |
| image_folders_ms | 350 | 337 | ~0 |
| IPC roundtrip | 1,393 | 1,367 | ~0 |
| invoke_return（进程→数据） | 5,393 | 1,751 | −67.5% |
| masonry mounted（可交互） | 5,408 | 1,764 | −67.4% |
| 内存峰值 | 2,775 MB | 970 MB | −65.1% |
| 结束时工作集 | 2,067 MB | 295 MB | −85.7% |
| 启动后 30s CPU | ~31.1 s | ~27.5 s | −11% |
| JS 堆（snapshot） | — | 见 `runs/opt*.snapshots.json` | — |

P90：可交互 5,572→1,834ms；峰值 2,777→971MB。
**冷缓存**：`base1` 半冷样本 setup 15,322ms / 可交互 18,847ms，说明冷盘下 H1 成本可达热的 ~4 倍；据此静态估算 P0 冷启动可交互降幅 ≥80%，但**待用户冷采集脚本实测确认，不与热数据混算**。

---

## 9. 后续计划

1. **冷数据采集（用户协作）**：重启后立即双击 `scripts/startup-perf/collect-cold-start.cmd`，5 次采样自动出 P50/P90；结果回填本报告 §8。
2. **埋点保留与防退化**：
   - Rust：`setup_window_shown_ms`（应 <50ms）、`setup_total_ms`（应 <100ms）、`load_store` 分阶段、`list_library_command total_ms`。
   - 前端：既有 `[startup-prof] loadLibrary / SegmentedMasonry` 全部保留。
   - 建议 CI/自检门槛（热缓存）：`setup_total_ms ≤ 100`、`masonry mounted ≤ 3s`。
3. **剩余风险（重点 H1 回归）**：
   - 当前 embeddings 已 809K；若语义扫描继续增长，首次相似搜索将现场加载更大缓存，内存尖峰 ~2.7GB。若首搜体感不可接受 → 采用 P4（首屏后后台预热）。
   - P1/P2 待你确认是否允许改变“启动即加载文件夹归属 / 启动即清理”的可观测行为后再实施。
4. **未做/不推荐**：P3（ORDER BY）无数据支撑，跳过；H3（5000→500）证伪，不做；不引入 Splash/延时遮丑、不开无界线程、不改公开 API、不动事务语义。

---

## 附录 A：产物与复现

- 采样工具：`scripts/startup-perf/harness.mjs`、`aggregate.py`
- 冷启动采集：`scripts/startup-perf/collect-cold-start.cmd`（双击）/ `collect-startup.ps1`
- 原始数据：`%TEMP%\illutag-startup-perf\*.summary.json|.stderr.log|.console.jsonl`（本轮另存于 `D:\Temp\Lu\opencode\runs`）
- 部署/回滚：当前部署 `illutag.exe` = P0 版；旧基线备份在 `illutag.exe.prev` 与 `D:\Temp\Lu\opencode\illutag.exe.shipped`。回滚 = 用 `.prev` 覆盖 `illutag.exe` 并恢复 setup 中的 warmup 调用。
- 数据安全：全程未写入 `illutag.sqlite`（WAL=0，images/assignments/clip/thumbnails 行数前后一致）；素材目录只读；`window-state.json` 采集期间临时归一化为 1400×900 以保证可比性，结束后恢复原值。
