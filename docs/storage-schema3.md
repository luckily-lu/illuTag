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
默认目标上限为 10 GiB，工作区预留空间；SQLite 临时文件必须明确放入指定工作区。

`storage_cutover.py` 支持 prepare/install/recover/accept。源数据库重命名为唯一回滚库，
程序回滚仅保存 EXE 和小型清单文件，模型、运行时和原图不复制。
各阶段持久化切换状态，启动时未完成的切换阻止普通写入。只读验收完成后 accept；
accept 后禁止直接回滚，避免丢失已经提交的新数据。

旧完整恢复包先校验原包，直接从包内旧数据库生成紧凑目标；避免再复制一份旧库。
新恢复包仍通过 SQLite Backup API 创建。JSON 的 schemaVersion=1 是交换格式版本，
不含自动标签及向量，不能用于完整恢复。

路径映射更新图片公开 ID/path 与真正路径字段；内部整数关联保持稳定。
旧程序不得打开 schema 3。未知更高数据库版本一律拒绝写入。

## 验证入口

- `cargo test --release`：数据、路径、分类、备份恢复、精确 top-600 与组合查询。
- `test_storage_cutover.py`：六个实际进程终止位置及接受升级后的回滚保护。
- `storage-native-benchmark.mjs`：显式程序、数据库、模型资源和工作区；只读数据引用，每轮固定窗口，五次热启动观察首 90 秒。

测试 TEMP/TMP、WebView、日志和 CUDA 缓存必须隔离。数据库只读引用属于 NTFS 硬链接，
不复制数据；清理回滚库时必须同时移除测试引用，最后一个链接删除后磁盘空间才释放。
