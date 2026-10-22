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

输出在 `D:\Ai\illuTag-refactor-work\perf-million-hires\<run-id>`：`environment.json`、`run-config.json`、`resources.jsonl`、结果 JSON、`exitcode.txt`、`summary.json/md`。runner 失败会抛错并以非零码退出；`summary.json` 的 `passed/exit_code/failure_reason` 必须一起检查。子进程退出码从 cmd 写出的 `exitcode.txt` 读取，避免 `Start-Process` 重定向时 Windows PowerShell 的 null `ExitCode`。

## 隔离与资源

- 冷跑要求全新的 run 根；`-Reuse` 明确禁用。路径检查有分隔符边界和每层 reparse point 检查；runner 写入 `.perf-isolated`，所有 H/X/native 入口共用 `perf_support` 验证 marker、范围和冷子目录。
- 启动门禁：单次预计数据 + 已有 QA 总量 ≤120 GiB；D 盘保留 max(40 GiB, 卷 5%)；空闲物理内存 ≥8 GiB。
- 运行中每 5 秒递归统计 QA 目录**实际字节**（不是整盘空闲差），并检查 QA 上限、D 盘保留和系统空闲内存；每 100ms/1s 采样测试进程。
- private bytes 默认 4 GiB、wall time 默认 240 分钟；越限写 `STOP`，10 秒后按已见 PID+创建时间结束 cmd 全进程树（含构建 stage），确认无残留才写 `stopped=true`；有残留则失败并报告 PID。
- `-TestStopAfterSeconds`、`-TestBuildSleepSeconds`、`-TestMaxPrivateGb` 仅用于故障注入；`-ForceFailure`/`-ForceCrash` 验证失败传播。

## 正确性

H 层 manifest 包含 family、expected relation、byte digest 和 JPEG quality。测试按 family 检查最终 `cascade_pairs`：exact 必须 exact、near 必须 near、similar 必须 similar；同时遍历全部 exact/near 边，跨 family 的 exact/near 也失败。不一致即 panic，runner 非零退出。

JPEG quality 95 是独立通行档；默认质量下的 similar 仍是已知限制，不能用 quality95 的通过声称一般重压缩识别已修复。

M-numeric 只报告索引回调/分布，已明确标注未实现 M1 离线查询真值；不能当作完整百万验收。
