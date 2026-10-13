# Schema 3 存储维护

illuTag 1.2.0 将图片关联改成内部整数键，对外继续使用既有字符串图片 ID。
`images.image_pk` 是显式整数主键，`images.id` 和 `images.path` 均保留。
扫描使用 UPSERT 保持既有主键；禁止用 REPLACE 更新图片，否则会触发关联删除。

## 表与查询

- `storage_auto_tags` 使用整数图片、模型、类别、标签与历史译名引用，复合主键采用 WITHOUT ROWID。
- `storage_strings` 驻留原始文本，采用二进制区分；NULL 译名与空译名保持不同。
- `storage_clip_models` 集中保存模型 ID/version；`storage_clip_embeddings` 保留普通行表和原始 float32 BLOB。
- 其他图片关联保存在 `storage_image_*` / `storage_reference_board_items`，外键引用 `images.image_pk`。
- 原表名的只读视图还原字符串 ID，支持详情、历史数据与旧 JSON 的读取契约。写入必须使用实际存储表及明确的主键解析。
- 检索与作用域候选使用整数键；输出与同分排序继续使用原字符串 ID。

数据库页大小为 16 KiB。2 万图片样本比较结果：旧格式 580,194,304 字节，
紧凑格式 4/8/16 KiB 页分别为 147,509,248 / 120,094,720 / 112,721,920 字节。
向量迁移不得调用会再次归一化的常规写入函数。

标签显示继续优先当前字典，回退历史译名；自动分类继续读取历史记录。
已知标签统计按 image/model/tag 去重，跨 category 出现时只计一次。

## 升级与恢复

普通连接只初始化空库，拒绝把 schema 1/2 自动升级后交给新查询。
耗时转换由 `src-tauri/scripts/storage_optimize.py` 执行：源库只读，目标必须是显式路径下的新文件。
生产转换取得与应用一致的 Windows 独占租约，转换前后验证源文件 SHA-256。
每批比较全部逻辑字段及向量字节，最后核对总数、外键、完整性并生成小型验证状态。
默认目标上限为 10 GiB，工作区累计产物上限为 18 GiB，磁盘至少保留 5 GiB 空闲；
SQLite 临时文件放在显式工作区的 `sqlite-temp`。达到限制时失败，保留源库，不能
继续部署未完成的目标文件。

完整生产转换命令如下；路径是示例，工作区需提前准备且目标文件不得存在：

```powershell
python -B src-tauri/scripts/storage_optimize.py convert `
  --source 'D:\illuTag-production\illutag.sqlite' `
  --target 'D:\illuTag-storage-work\schema3.sqlite' `
  --workspace 'D:\illuTag-storage-work' `
  --schema-root 'src-tauri/src/library' `
  --production
```

默认只接受 D 盘工作区。恢复到其他显式选择的磁盘可用 `--allow-other-drive`。
`convert` 支持 schema 1/2；`--production` 才取得源目录租约、做转换前后源哈希校验，
并生成目标旁的 `.verified.json`。切换工具需要这个验证记录，不以文件存在或体积
变小代替校验。`sample --images 20000` 仅生成对比样本，不能替代全库迁移。

`storage_cutover.py` 支持 prepare/install/recover/accept。源数据库重命名为唯一回滚库，
程序回滚仅保存 EXE 和小型清单文件，模型、运行时和原图不复制。
各阶段持久化切换状态，启动时未完成的切换阻止普通写入。只读验收完成后 accept；
accept 后禁止直接回滚，避免丢失已经提交的新数据。

切换仅支持 schema 2 → 3，目标和部署目录必须在同一卷，路径及祖先不能是目录链接。
先正常退出所有使用该目录的应用，每一步均取得独占租约：

```powershell
python -B src-tauri/scripts/storage_cutover.py prepare `
  --root 'D:\illuTag-production' `
  --target 'D:\illuTag-storage-work\schema3.sqlite' `
  --exe 'D:\illuTag-build\release\illutag.exe'
python -B src-tauri/scripts/storage_cutover.py install --root 'D:\illuTag-production'
```

`prepare` 校验源库、目标、EXE 和验证记录，目标以重命名方式移入 staging。
`install` 将原库改名为 `illutag.schema2.rollback.sqlite`、目标改名为正式
`illutag.sqlite`，并替换 EXE。脚本**不会自动部署整套 Python 脚本、配置和发布清单**；
它们必须由发布流程与候选 EXE 配套安排。`storage-program-rollback` 只覆盖脚本
`FILES` 列出的 EXE 和已有小型清单，不等同于完整部署快照。

安装后状态为 `newInstalled`，只允许 `ILLUTAG_READ_ONLY=1` 的新程序进行验收。
只读验收结束并正常退出后，执行：

```powershell
python -B src-tauri/scripts/storage_cutover.py accept --root 'D:\illuTag-production'
```

`accept` 再次核对库和 EXE 哈希，写入 `accepted` 后允许正常写入。尚未接受且切换
中断时，可运行 `recover --root ...` 恢复原库和记录中的旧程序；若新库已改变、旧库
身份不匹配或已 accepted，则拒绝恢复。不要手工删除日志来绕过维护状态。

accepted 不会自动删除旧库。确认不再需要旧版本恢复后，可清理回滚库、小型程序
回滚目录及测试工作区；保留正式 `illutag.sqlite` 和 `illutag.storage-switch.json`。
不要重新运行 prepare 来“再次压缩”已接受的 schema 3 库。

旧完整恢复包先校验原包，直接从包内旧数据库生成紧凑目标；避免再复制一份旧库。
新恢复包仍通过 SQLite Backup API 创建。JSON 的 schemaVersion=1 是交换格式版本，
不含自动标签及向量，不能用于完整恢复。

路径映射更新图片公开 ID/path 与真正路径字段；内部整数关联保持稳定。
旧程序不得打开 schema 3。未知更高数据库版本一律拒绝写入。

## 验证入口

- 仓库根目录 `cargo test --release --manifest-path src-tauri/Cargo.toml`：数据、路径、分类、备份恢复、精确 top-600 与组合查询。
- `src-tauri/scripts/test_storage_cutover.py`：六个实际进程终止位置及接受升级后的回滚保护，必须设置独立 `ILLUTAG_STORAGE_TEST_ROOT`。
- `scripts/refactor/storage-native-benchmark.mjs`：显式程序、数据库、模型资源和工作区；只读数据引用，每轮固定窗口，五次热启动观察首 90 秒。参数及路径限制见 [维护指南](maintenance.md#自动化脚本边界)。

测试 TEMP/TMP、WebView、日志和 CUDA 缓存必须隔离。数据库只读引用属于 NTFS 硬链接，
不复制数据；测试期间源库也必须停止写入。清理回滚库时必须同时移除测试引用，
最后一个链接删除后磁盘空间才释放。生产路径仍保留时，删除测试链接不影响正式库。

## 1.2.0 存储升级验收记录

本次升级在 2026-10-13 完成，以下是该次验收记录，不是每次文档更新重新运行的结果。
原库 29,254,098,944 字节，紧凑库 4,846,419,968 字节，体积减少约 83.4%。
校验覆盖 809,230 张图片、41,254,917 条自动标签、809,229 条图片向量，逻辑字段、
向量原始字节、固定组合查询与详情保持一致。46 项 Rust 测试和 9 项前端测试通过，
正式 release 构建及切换故障测试通过。

同机、同库、同资源、固定窗口下，各五次热启动的新版/基线比值：启动 P50 84.8%、
P90 84.9%、峰值内存 98.3%；标签/文件名/自然语言查询 P95 分别为 62.8%、105.1%、
107.1%，均符合该次验收上限。旧报告的 1.76 秒不能替代这组基线。
原始测试产物不属于正式运行依赖；清理后可按脚本重新建立独立测试。
