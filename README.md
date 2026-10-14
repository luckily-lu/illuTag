# illuTag 1.3.0

illuTag 是本地图片画廊与标签管理工具，使用 Vue 3、TypeScript、Tauri 2、Rust、SQLite 和 Python ONNX。图片处理和搜索在本机运行。

界面采用浅色“无框画廊”：原比例虚拟瀑布流、固定搜索区、文字与图标次要操作、炭黑主按钮。图片卡片默认不显示文件名和标签，收藏与选择状态持续可见。信息面板按中文标签、英文原名、置信度分栏，搜索条件也优先显示中文译名。

当前便携发布以 Windows / WebView2 为验收平台。应用图标为带浅色圆角背景的 `iT` 字标，EXE 内嵌图标与便携版图标资源保持一致。

## 浏览、搜索和整理

- 标签、文件名和自然语言可组合查询。结构化条件先筛选，自然语言在候选范围内精确排序，最多返回前 600 项。
- 全部、随机、收藏、待分类、应用回收站，以及多级虚拟文件夹；虚拟分类不移动磁盘原图。
- 拖拽分类、标签管理与字典、自动分类规则、批量操作和系统回收站。
- 大图缩放平移、轮播、窗口置顶和复制原图。关闭查看器后恢复浏览状态。
- 批量工具栏替换普通操作区；“选择当前已加载结果”只选择已加载图片，部分失败时保留失败项的选中状态。
- DanbooruTagQuery 自动打标保留部署模型和预处理，生效通用/角色阈值为 0.2/0.2；目录名沿用 `wd-swinv2-tagger-v3`。
- Chinese-CLIP 图片索引用于自然语言搜索，生成时读取原图。
- 独立的“重复检索”页通过内容哈希和感知指纹查找复制、改名、压缩或缩放版本，按组展示并支持并排比对；仅提供检索，不删除、合并或折叠图库。操作说明见 [重复与近似图片检索](docs/duplicate-search.md)。

普通以图、配色和氛围相似搜索已移除。历史配色、氛围、参考板及托管图片数据保留，并继续进入完整备份。

## 来源和任务

在“设置 → 图库与存储”登记磁盘来源后，到任务中心手动扫描或启动一键处理。登记来源、启动应用、切换缩略图显示不会自动开始后台任务。

一键处理顺序为：扫描来源 → 缩略图（启用时）→ 自然语言图片索引 → 自动打标。重任务串行排队；暂停/继续在当前进程内有效，取消保留已提交结果。重启后未完成任务标为已中断，由用户重新运行；有效历史结果继续复用。

扫描失败停止一键流水线。独立后续阶段失败可继续其它阶段，任务最终显示部分完成。缺失文件清理是手动操作，离线来源不会被整盘清理，文件读取权限错误不会被当作缺失。

## 数据和备份

便携版包含 `illutag.portable.json`。其中相对 `dataDir` 以 EXE 所在目录解析，绝对路径保持不变。数据库文件为 `illutag.sqlite`，当前数据版本为 **schema 3**。已有便携部署可能把数据库放在 EXE 同级，维护前应读取配置确认实际位置，不能仅凭 `data` 文件夹名称选择数据库。

schema 3 用内部整数键关联图片，集中保存重复标签文本，并保留原始 CLIP float32 向量。对外图片 ID、历史译名、标签置信度和搜索排序语义保持不变；存储优化不重新打标或生成向量。普通启动不会转换旧库：schema 1/2 必须通过离线转换工具或完整恢复流程升级，未知更高版本拒绝写入。转换、校验与切换步骤见 [Schema 3 存储维护](docs/storage-schema3.md)。

“完整恢复包”使用 SQLite Backup API，包含数据库、偏好、窗口状态、必要配置及现存托管图片。外部来源原图不包含在恢复包内；模型由程序包提供，缩略图是可重建缓存。旧 JSON 仍可交换元数据，但不是完整恢复包。

恢复和目录迁移先校验副本再切换，失败保留原库，成功后需要重启。旧 EXE 不得打开 schema 3 数据库。切换尚未接受时，可按维护流程恢复匹配的旧程序、脚本、配置和原数据库；切换状态为 `accepted` 后，禁止直接回滚旧库，以免丢失升级后新增的数据。

## 开发和验证

Windows 开发推荐 Node.js 22.12 或更新版本，并需要 npm、Rust、Microsoft C++ 构建工具和 WebView2。模型与 Python runtime 单独准备；开发、验收和打包均不得使用生产图库作为可写测试数据。

```powershell
npm ci
npm test
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
```

浏览器界面预览：

```powershell
npm run dev
# 打开 http://127.0.0.1:1421/?preview=1，内容为测试素材。
```

真实 Tauri 开发测试必须显式隔离数据、WebView、素材和日志：

```powershell
$env:ILLUTAG_DATA_ROOT='D:\illuTag-test\data'
$env:ILLUTAG_PROFILE_ROOT='D:\illuTag-test\profile'
$env:ILLUTAG_ASSET_ROOT='D:\illuTag-test\assets'
$env:ILLUTAG_LOG_ROOT='D:\illuTag-test\logs'
$env:ILLUTAG_RESOURCE_ROOT='D:\illuTag-test\program'
npm run tauri dev
```

隔离路径必须是有效绝对路径。资源目录必须提供完整 runtime、模型、字典、配置和脚本；错误配置直接失败。测试素材使用独立可写目录，勿通过源码模型 Junction 改写生产资源。

正式 EXE 构建必须嵌入前端：

```powershell
npm run build
cargo build --manifest-path src-tauri/Cargo.toml --release --features custom-protocol
```

## 便携包

准备完整资源目录后执行：

```powershell
.\scripts\package-portable.ps1 -ResourceRoot 'D:\illuTag-resources' -OutputRoot 'D:\illuTag-releases'
```

脚本读取当前项目版本，不覆盖已存在的包。可用 `-Build` 先构建，用 `-ReleaseExe` 指定 EXE，用 `-SkipZip` 只生成文件夹。必须包含 `tagger_config.json`、`tag_groups.json` 和 `ort_runtime.py`。模型、脚本和字典优先从 EXE 同级加载，避免快捷方式工作目录影响资源选择。

```text
illuTag-1.3.0/
  illutag.exe
  illutag.portable.json
  model-manifest.json
  release-manifest.json
  runtime/python/
  scripts/
    storage_optimize.py
    storage_cutover.py
    compact_schema.sql
    compact_indexes.sql
  wd-swinv2-tagger-v3/
  model/chinese-clip-vit-base-patch16/
  data/                         # 首次运行创建，不包含在程序包内
```

源码不包含大型模型权重、Python runtime、用户数据库及原图。

更换图标后需要重新构建 EXE，单独替换外部 ICO 不会更新 EXE 内嵌图标。图标生成与维护入口为 `scripts/update-app-icons.ps1`；开发缓存及测试产物的清理边界见 [开发与发布维护](docs/maintenance.md)。

## 维护结构

前端按图库、搜索、文件夹、标签、查看器、任务和设置划分；共享 IPC 客户端、类型契约和基础控件位于 `src/shared`。状态通过 composable 与 typed provide/inject 组织。

Rust 入口负责装配、窗口和命令注册，业务与 SQLite、模型进程、任务、备份恢复分开。DTO 经 ts-rs 生成 TypeScript 契约。元数据修改返回局部变更和逐项失败；任务进度限频，终态即时通知；普通修改不重载全库。

`scripts/refactor` 包含隔离原生验收、完整恢复、模型回归、故障和性能脚本。性能比较必须使用同机、同一数据库快照、同窗口与模型配置；5 次热启动，每次观察 90 秒。历史启动数据只作参考。

## 项目文档

| 文档 | 内容 |
|---|---|
| [更新记录](CHANGELOG.md) | 1.2.0 存储优化与此前画廊重构的发布说明 |
| [开发与发布维护](docs/maintenance.md) | 隔离测试、构建、资源打包及安全清理 |
| [Schema 3 存储维护](docs/storage-schema3.md) | 数据结构、离线升级、验收、恢复与硬链接 |
| [原比例瀑布流](docs/segmented-masonry.md) | 增量布局、按列可见区查找和分页行为 |
| [开发压力测试](docs/dev-stress-tools.md) | 开发测试入口及隔离要求 |
| [重复与近似图片检索](docs/duplicate-search.md) | 手动查重、分组比对、候选解释与可重建缓存 |

## 第三方资源

- Chinese-CLIP：https://github.com/OFA-Sys/Chinese-CLIP
- DanbooruTagQuery：https://github.com/realphongha/danbooru-tag-query
- Lucide 图标的许可证随便携包提供。模型许可证以各模型仓库说明为准。
