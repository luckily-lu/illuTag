# 百万性能 runner

默认不运行，所有测试 `#[ignore]`。只使用合成数据；不启动 Tauri/WebView，不访问正式图库。

## 用法

```powershell
# H 层：真实文件扫描 + 生产三轮（2MP 小档示例）
.\tools\perf\run-tier.ps1 -Layer H -Count 1000 -Width 1600 -Height 1200 -RunId run-h1000-2mp

# M 层：十万/百万合成指纹
.\tools\perf\run-tier.ps1 -Layer M -Count 1000000 -RunId run-m1m

# X2：不同内容的密集候选
.\tools\perf\run-tier.ps1 -Layer X -Count 1000 -RunId run-x1000

# native 16.7MP/64MiB 边界（真实三轮 native 路径）
.\tools\perf\run-tier.ps1 -Layer native -Count 1 -RunId run-native-boundary
```

输出在 `D:\Ai\illuTag-refactor-work\perf-million-hires\<run-id>`：`environment.json`、`run-config.json`、`resources.jsonl`、结果 JSON、`summary.json/md`。runner 失败会抛错并以非零码退出；`summary.json` 的 `passed/exit_code/failure_reason` 必须一起检查。

## 隔离与资源

- 冷跑要求全新的 run 根；已存在目录默认拒绝，`-Reuse` 需匹配 marker/config。路径检查有分隔符边界和每层 reparse point 检查。
- 启动门禁：单次预计数据 + 已有 QA 总量 ≤120 GiB；D 盘保留 max(40 GiB, 卷 5%)；空闲物理内存 ≥8 GiB。
- 运行中每秒检查磁盘保留、QA 总量、private bytes（默认 4 GiB）和 wall time（默认 240 分钟）；越限写 `STOP`，测试在 control/生成循环中协作退出，10 秒后只终止记录的测试 PID（校验创建时间和进程名）。
- `-TestStopAfterSeconds` 与 `-TestMaxPrivateGb` 仅用于故障注入；`-ForceFailure`/`-ForceCrash` 验证 runner 失败传播。

## 正确性

H 层 manifest 包含 family、expected relation 和 byte digest。测试按 family 对最终 `cascade_pairs` 做逐图关系校验：exact 必须 exact，near 必须 near，similar 必须 similar；不一致即 panic，runner 非零退出。不能仅比较 `status.near_groups`，该字段包含非 exact 分组。

M-numeric 只报告索引回调/分布，已明确标注未实现 M1 离线查询真值；不能当作完整百万验收。
