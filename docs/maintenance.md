# illuTag 1.4.0 维护指南

当前版本使用 Vue 3、TypeScript、Tauri 2、Rust、SQLite schema 3 与 Python ONNX。
界面为浅色无框画廊。源码、正式程序/数据、测试工作区应使用不同目录。

1.3.0 增加独立的[重复与近似图片检索](duplicate-search.md)，1.4.0 加入组内保留、选择、连续比对和明确执行的回收操作。
任务由用户手动启动，不加入“一键处理”；主库仍为 schema 3，无本次数据库升级。
查重数据属于 `cache/duplicates.sqlite` 派生缓存，可单独清理，不进入完整备份或数据目录迁移包。
1.4.0 的批次操作与撤销记录保存在 `operations/duplicate-management.sqlite`，独立于指纹缓存，预算为 32 MiB。
记录绑定原主库身份，恢复或替换图库后不能沿用旧撤销；清理记录需要明确确认，不能与活动整理任务同时执行。

## 开发与验证入口

在仓库根目录运行，推荐 Node.js 22.12 或更新版本，Rust 和 Windows Tauri 构建依赖需要
预先安装；模型与 Python 运行时不由 npm 或 Cargo 下载。

```powershell
npm ci
npm run typecheck
npm test
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --release --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --no-bundle
```

这些命令会生成依赖或构建缓存。`npm run build` 只验证并构建前端；最后一条才
生成正式 EXE。文档更新不需要重跑这些命令，也不等于完成新一轮运行验收。
可设置 `CARGO_TARGET_DIR` 到空间充足的独立构建目录，避免在系统盘积累缓存。

Rust DTO 和生成的 `src/shared/contracts` 是 IPC 契约。修改 Rust DTO 后应同步
检查生成类型、字段序列化与前端调用，不手工维护另一套不一致的接口。

## 隔离开发与测试

下面是路径示例，执行前换成自己的独立目录。资源目录应包含匹配版本的
`runtime/python`、模型和 `scripts`；把少量可写测试图片放到 `assets`。
不要把生产数据库硬链接当成可写测试副本。

```powershell
$taskRoot = 'D:\illuTag-dev-work'
$taskResources = 'D:\illuTag-resources'
foreach ($taskName in @('data', 'profile', 'assets', 'logs', 'pycache', 'cuda', 'temp')) {
  New-Item -ItemType Directory -Path (Join-Path $taskRoot $taskName) -Force | Out-Null
}
$env:ILLUTAG_DATA_ROOT = Join-Path $taskRoot 'data'
$env:ILLUTAG_PROFILE_ROOT = Join-Path $taskRoot 'profile'
$env:ILLUTAG_ASSET_ROOT = Join-Path $taskRoot 'assets'
$env:ILLUTAG_LOG_ROOT = Join-Path $taskRoot 'logs'
$env:ILLUTAG_RESOURCE_ROOT = $taskResources
$env:PYTHONPYCACHEPREFIX = Join-Path $taskRoot 'pycache'
$env:CUDA_CACHE_PATH = Join-Path $taskRoot 'cuda'
$env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $taskRoot 'profile'
$env:TEMP = Join-Path $taskRoot 'temp'
$env:TMP = Join-Path $taskRoot 'temp'
$env:CARGO_TARGET_DIR = 'D:\illuTag-build'
npm run tauri dev
```

指定 `ILLUTAG_DATA_ROOT` 时，运行时同时要求绝对路径的 profile、asset、log 目录；
无效配置应直接失败。`ILLUTAG_USE_TEST_DB` 只改数据库文件名，不替代这些隔离项。
新库初始化为 schema 3；schema 1/2 通过离线工具转换，禁止隐式打开旧库写入。

启动、进入设置、切换缩略图开关不会自动处理图库。登记来源后，到任务中心手动
启动扫描/一键处理。任务取消保留已提交结果，重启后的未完成任务由用户重新运行。

测试结束后正常关闭测试窗口，再关闭专用终端。不要从带有上述环境变量的终端
启动正式程序，不按全局进程名结束 Python、WebView2 或 Codex。

### 自动化脚本边界

- `scripts/refactor/native-session.mjs` 的程序和数据目录必须位于名称为
  `illuTag-refactor-work` 的隔离路径下，并核对目录链接。多数原生 UI/恢复脚本
  基于这个入口，执行前查看各自参数和输出范围。可通过 `resourceRoot` 参数只读复用已有静态资源，
  不复制模型；WebView、Python、CUDA 缓存和临时目录仍隔离在测试根目录内。
- `scripts/refactor/duplicate-native-qa.mjs` 参数为 `EXE ROOT RESOURCE_ROOT`，仅接受工具指定隔离工作区中的
  候选程序和全新测试根目录。它生成少量抽象素材，使用真实 IPC 和 WebView2 检查查重任务、分页、比对、
  暂停／取消、重启及缓存清理；正常关闭本次启动的进程，截图与小型 JSON 记录写入测试根目录的 `logs`。
  不要把正式数据库或生产素材放入该脚本的测试根目录。
- `scripts/refactor/duplicate-management-native-qa.mjs` 使用相同的 `EXE ROOT RESOURCE_ROOT` 参数与隔离约束，
  生成少量抽象副本，验证完整组管理、跨组保留、应用回收与撤销、源文件变更和系统回收的明确确认。
  系统回收测试只处理新生成的小文件；不能改成真实图库或将生产原图复制进去作为删除样例。
- `scripts/refactor/storage-native-benchmark.mjs` 只接受
  `D:\Ai\illuTag-storage-work\` 内的新测试工作目录。参数顺序为
  `EXE DATABASE WORKSPACE RESOURCE_ROOT [benchmark|smoke]`，均为绝对路径。
  它创建数据库的只读 NTFS 硬链接、独立 WebView/日志/缓存；固定窗口，先预热一次，
  benchmark 再运行五次并观察每次首 90 秒。工作目录中的数据库链接不得预先存在，
  源库须正常关闭且没有未结算 WAL/journal；不要与写入同一源库的应用并行运行。
- `src-tauri/scripts/test_storage_cutover.py` 使用微型隔离样本，需显式设置
  `ILLUTAG_STORAGE_TEST_ROOT` 为 `D:\Ai\illuTag-storage-work` 下已准备好的目录，
  然后运行 `python -B src-tauri/scripts/test_storage_cutover.py`。
- `scripts/refactor/prepare-isolation.py` 是 1.1.0 重构期间的历史准备脚本，会复制
  全库与重复模型，候选名称仍固定为旧版本。它不是当前推荐的通用测试准备入口。
  `scripts/startup-perf` 及 [2026-09-25 报告](startup-perf/2026-09-25-startup-perf.md)
  同样用于历史追溯，不能沿用旧指标或生产目录运行步骤替代当前验收。
- 压力数据生成见 [开发压力数据工具](dev-stress-tools.md)。这些旧命令需要维护者
  自行保证隔离，不能在性能测试的生产数据库硬链接上执行。

## 便携发布

`scripts/package-portable.ps1` 从 `package.json` 读取版本，默认读取
`CARGO_TARGET_DIR\release\illutag.exe` 或 `src-tauri\target\release\illutag.exe`。
`ResourceRoot` 必填，资源缺失或输出已存在时失败。

```powershell
.\scripts\package-portable.ps1 `
  -ResourceRoot 'D:\illuTag-resources' `
  -OutputRoot 'D:\illuTag-release-work' `
  -ReleaseExe 'D:\illuTag-build\release\illutag.exe' `
  -SkipZip
```

`-Build` 会先执行正式构建；已有匹配 EXE 时省略它。`-SkipZip` 避免同时保存目录
和 ZIP 两套产物；需要 ZIP 时再明确选择。包必须包含 DTQ 的 `tagger_config.json`
与 `tag_groups.json`、CLIP 资源、匹配 Python 脚本、存储转换脚本/SQL、Lucide 许可、
模型及发布哈希清单。包内默认 `illutag.portable.json` 使用相邻 `data` 目录，不带
用户数据库或外部原图。

发布前在独立目录验收程序与资源的整套组合。打包不是生产部署，不覆盖正在使用
的数据目录。升级数据库的步骤见 [Schema 3 存储维护](storage-schema3.md)。

## 1.4.0 验证范围

2026-10-15 前端类型检查、构建及 23 项 Vitest 测试通过；Rust release 检查、4 项管理专项测试、
DTO 契约导出和正式 custom-protocol release EXE 构建通过。本轮只运行改动相关检查，未重复全库性能实测。

真实 Tauri／WebView2 管理验收通过 9 项检查，使用 20 张合成图片，共 32,585 字节，数据库和 WebView
均位于隔离目录。覆盖未展开成员、保留项变化拦截、全库完全相同组一键整理、跨组保护、局部分组复核、
近似候选逐项处理、普通回收后的撤销归属、系统回收站明确第二步，以及清指纹缓存后保留管理记录。
真实多图审核页在 500／1280 CSS px 宽度无横向溢出，K／方向键／Delete／Enter 只回收人工选择的候选。
正常退出测试进程；未在生产图库执行回收、重新建索引或复制全库。

## 1.3.0 验证范围

2026-10-14 的前端类型检查及 21 项 Vitest 测试通过。Rust 常规测试通过 75 项库测试与 4 项程序测试，
另显式运行通过 1 项默认忽略的 80 万条 MIH 规模测试，共 80 项。第二轮真实 Tauri／WebView2 合成素材验收
通过 12 项检查，确认任务控制、重启中断、缓存清理及主库图片／标签完整性。

真实图库 832,758 张中已提交 45,900 张指纹；按用户要求暂停全库实测，保留已提交的缓存供后续复用。
全库分组、总耗时与真实峰值内存尚未验收，不能由合成规模测试或这段部分运行推定全库已通过。
具体样本规模、索引估算及时间记录见[重复检索验证记录](duplicate-search.md#验证记录与实测边界)。

## 空间清理与保留边界

先退出属于该测试或构建的进程，核对完整路径、目录链接和数据用途，再清理。
无需为清理另做整库备份或生成候选包。没有确认归属的共享 npm/Cargo/Python 缓存
不按日期猜删。

| 可清理内容 | 前提或影响 |
|---|---|
| 仓库 `node_modules`、`dist`、`src-tauri/target` 或独立 `CARGO_TARGET_DIR` | 可重新安装/构建；不删除依赖清单、源码或 `.git` |
| 独立测试工作目录的库、日志、截图、WebView、CUDA/Python 缓存 | 已结束相关测试，确认不包含正式数据或仅存源码 |
| 重复候选程序、ZIP、过期发布输出 | 已确认当前正式程序与最终源码保留 |
| `cache/duplicates.sqlite` 及其伴随文件 | 优先使用“重复检索 → 清理查重缓存”；无运行或排队的查重任务，清理后须重新计算 |
| `illutag.schema2.rollback.sqlite`、`storage-program-rollback` | 升级状态为 accepted 且用户决定放弃旧版本恢复；见存储维护说明 |

必须保留正式目录里的当前 `illutag.sqlite`、仍在使用的 WAL/SHM、EXE、模型、运行时、
脚本、配置、偏好、窗口状态、生产 WebView、托管图片及维护状态。外部图库原图不属于
测试产物。源码里的模型 Junction 不是可递归清理的缓存，不沿链接进入正式资源目录。

NTFS 硬链接共享同一文件数据：目录属性中的逻辑大小会重复计算，最后一个链接
删除后才释放物理空间。正式库在生产路径仍存在时，移除其测试链接不会删除正式库。
旧回滚库的测试链接若留下，删掉回滚文件名仍不会释放整块空间。

删除路径应细化到具体缓存或回滚文件，**不能删除整个源码仓库或正式部署目录**。
清理完成只核对目标消失、保留文件存在和磁盘剩余空间，不为验证再次编译、扫描、
打标或复制全库。
