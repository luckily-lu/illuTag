import type { JobRecord } from "../../shared/contracts/JobRecord";
export const activeStates = [
  "queued",
  "running",
  "pausing",
  "paused",
  "cancelling",
];
export const jobNames: Record<string, string> = {
  pipeline: "一键处理",
  scan: "扫描来源",
  thumbnails: "生成缩略图",
  semantic: "自然语言图片索引",
  tagging: "自动打标",
  cleanup: "清理缺失索引",
  rules: "重新应用分类规则",
  duplicates: "重复图片检索",
  duplicateManagement: "整理重复图片",
  duplicateUndo: "撤销重复图片整理",
  recycleRestore: "还原回收站图片",
  recycleDelete: "清理回收站原文件",
};
export const stateNames: Record<string, string> = {
  queued: "等待中",
  running: "处理中",
  pausing: "正在暂停",
  paused: "已暂停",
  cancelling: "正在停止",
  cancelled: "已停止",
  interrupted: "已中断",
  completed: "已完成",
  partial: "部分完成",
  failed: "失败",
};
export const isActive = (j: JobRecord) => activeStates.includes(j.state);
export function durationLabel(job: JobRecord, now = Date.now()) {
  const start = job.meta?.startedAt;
  const end = job.meta?.finishedAt ?? (isActive(job) ? now : undefined);
  if (start === undefined || end === undefined || end < start) return "未记录";
  const seconds = Math.floor((end - start) / 1000);
  return seconds >= 3600
    ? `${Math.floor(seconds / 3600)}时${Math.floor((seconds % 3600) / 60)}分`
    : seconds >= 60
      ? `${Math.floor(seconds / 60)}分${seconds % 60}秒`
      : `${seconds}秒`;
}
export const jobVersion = (j: JobRecord) => j.meta?.revision ?? j.updatedAt;
export function acceptsJob(previous: JobRecord | undefined, next: JobRecord) {
  if (!previous) return true;
  if (!isActive(previous) && isActive(next)) return false;
  if (next.meta && !previous.meta) return true;
  if (previous.meta && !next.meta) return false;
  if (jobVersion(next) !== jobVersion(previous))
    return jobVersion(next) > jobVersion(previous);
  return !(!isActive(previous) && isActive(next));
}
export function capability(
  job: JobRecord,
  action: "Pause" | "Resume" | "Cancel" | "Retry",
) {
  const supplied = job.meta?.[`can${action}`];
  if (supplied !== undefined) return supplied;
  if (action === "Pause")
    return job.state === "running" && job.stage !== "cleanup";
  if (action === "Resume") return job.state === "paused";
  if (action === "Cancel")
    return (
      ["queued", "running", "pausing", "paused"].includes(job.state) &&
      job.stage !== "cleanup"
    );
  return ["interrupted", "partial", "failed", "cancelled"].includes(job.state);
}
export function stageLabel(job: JobRecord) {
  return jobNames[job.stage] || job.stage;
}
export function unitLabel(job: JobRecord) {
  return (
    (
      {
        images: "张",
        image: "张",
        files: "个文件",
        file: "个文件",
        pairs: "对",
        pair: "对",
        items: "项",
        item: "项",
      } as Record<string, string>
    )[job.meta?.unit ?? ""] || "项"
  );
}
