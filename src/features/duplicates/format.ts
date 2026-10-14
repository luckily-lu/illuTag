export function bytes(value: number) {
  if (value < 1024) return `${value} B`;
  const unit = Math.min(3, Math.floor(Math.log(value) / Math.log(1024)));
  return `${(value / 1024 ** unit).toFixed(unit === 1 ? 0 : 2)} ${["B", "KiB", "MiB", "GiB"][unit]}`;
}
export function checkedDate(value: number | null) {
  if (!value) return "尚未检查";
  return new Date(value < 1e12 ? value * 1000 : value).toLocaleString("zh-CN");
}
export function duration(value: number) {
  const seconds = Math.round(Math.max(0, value) / 100) / 10;
  if (seconds < 60) return `${seconds} 秒`;
  const rounded = Math.round(seconds);
  return `${Math.floor(rounded / 60)} 分 ${rounded % 60} 秒`;
}
export const sensitivityNames: Record<string, string> = {
  strict: "严格",
  standard: "标准",
  loose: "宽松",
};
export const stateNames: Record<string, string> = {
  empty: "尚无索引",
  missing: "尚无索引",
  ready: "已完成",
  complete: "已完成",
  completed: "已完成",
  stale: "需要更新",
  invalid: "缓存需要重建",
  partial: "部分完成",
  running: "处理中",
  cancelled: "已取消",
  interrupted: "已中断",
  failed: "失败",
  error: "检查失败",
  queued: "排队中",
  paused: "已暂停",
  cancelling: "正在停止",
};
