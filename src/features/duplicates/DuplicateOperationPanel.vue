<script setup lang="ts">
import { computed } from "vue";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type { DuplicateBatch } from "./useDuplicateBatches";
import { batchStageNames, stateName } from "./format";
const props = defineProps<{
  operation: DuplicateBatch;
  job?: JobRecord;
  busy: boolean;
  errorInDialog?: boolean;
}>();
const emit = defineEmits<{
  refresh: [];
  more: [];
  undo: [];
  retry: [];
  control: [action: string];
}>();
const state = computed(
  () =>
    props.job?.state ??
    props.operation.status?.state ??
    props.operation.jobState,
);
const active = computed(() =>
  ["prepared", "queued", "running", "pausing", "paused", "cancelling"].includes(
    state.value,
  ),
);
const total = computed(() =>
  active.value
    ? (props.job?.total ?? props.operation.status?.total ?? 0)
    : (props.operation.status?.total ?? props.job?.total ?? 0),
);
const processed = computed(() =>
  active.value
    ? (props.job?.processed ?? props.operation.status?.processed ?? 0)
    : (props.operation.status?.processed ?? props.job?.processed ?? 0),
);
const stage = computed(
  () => props.operation.status?.stage || (total.value > 0 ? "applying" : ""),
);
const stageLabel = computed(() =>
  stage.value === "applying" && props.operation.preview.action === "restore"
    ? "恢复回收站图片"
    : (batchStageNames[stage.value] ?? "准备整理"),
);
const stageTotal = computed(() =>
  props.operation.status?.stage
    ? props.operation.status.stageTotal
    : total.value,
);
const stageProcessed = computed(() =>
  props.operation.status?.stage
    ? props.operation.status.stageProcessed
    : processed.value,
);
const percentage = computed(() =>
  stageTotal.value > 0
    ? Math.min(100, Math.round((stageProcessed.value / stageTotal.value) * 100))
    : 0,
);
const batchName = computed(() =>
  props.operation.preview.action === "restore"
    ? "撤销整理批次"
    : props.operation.preview.action === "systemTrash"
      ? "历史原文件清理"
      : props.operation.preview.groupId === "all-exact"
        ? "整理完全相同图片"
        : props.operation.preview.groupId === "all-near"
          ? "整理近似图片"
          : props.operation.preview.groupId.startsWith("workflow:") &&
              props.operation.preview.groupId.endsWith(":exact")
            ? "整理完全相同图片"
            : props.operation.preview.groupId.startsWith("workflow:") &&
                props.operation.preview.groupId.endsWith(":near")
              ? "整理近似图片"
              : props.operation.preview.groupId.startsWith("workflow:")
                ? "历史统一整理"
                : "整理图片",
);
const resumable = computed(
  () =>
    !active.value &&
    props.operation.preview.action === "trash" &&
    (["all-exact", "all-near"].includes(props.operation.preview.groupId) ||
      (props.operation.preview.groupId.startsWith("workflow:") &&
        /:(exact|near)$/.test(props.operation.preview.groupId))) &&
    (["partial", "cancelled", "interrupted", "failed"].includes(state.value) ||
      (props.operation.status?.failed ?? 0) > 0),
);
const incomplete = computed(() =>
  ["partial", "cancelled", "interrupted", "failed"].includes(state.value),
);
</script>
<template>
  <section class="duplicate-operation-panel" aria-label="整理批次进度">
    <header>
      <div>
        <span class="duplicate-task-kicker">{{
          active ? "当前任务" : "最近整理"
        }}</span>
        <h2>{{ batchName }}</h2>
      </div>
      <span class="duplicate-task-state">{{ stateName(state) }}</span>
    </header>
    <div v-if="active" class="duplicate-stage" role="status">
      <div class="duplicate-stage-heading">
        <span>{{ stageLabel }}</span
        ><span v-if="stageTotal > 0" class="duplicate-stage-numbers"
          >{{ stageProcessed.toLocaleString() }} /
          {{ stageTotal.toLocaleString() }}
          <small>{{ percentage }}%</small></span
        >
      </div>
      <progress
        v-if="stageTotal > 0"
        :value="stageProcessed"
        :max="stageTotal"
        :aria-label="stageLabel"
      />
      <progress v-else :aria-label="stageLabel" />
      <p
        v-if="
          ['snapshot', 'workflowSnapshot', 'planning', 'undoSnapshot'].includes(
            stage,
          ) || !stage
        "
        class="duplicate-stage-hint"
      >
        沿用已有检索结果，准备完成后自动继续。
      </p>
    </div>
    <p
      v-if="!active && state === 'completed' && !total"
      class="duplicate-batch-outcome"
    >
      {{
        operation.preview.action === "restore"
          ? "没有需要恢复的图片。"
          : "整理完成，没有需要回收的图片。"
      }}
    </p>
    <p
      v-else-if="
        operation.status && (operation.status.succeeded > 0 || !active)
      "
      class="duplicate-batch-outcome"
    >
      <template v-if="operation.status.succeeded > 0"
        >{{
          operation.preview.action === "restore"
            ? "已恢复"
            : operation.preview.action === "systemTrash"
              ? "已清理原文件"
              : "已移入应用回收站"
        }}
        <strong>{{ operation.status.succeeded.toLocaleString() }}</strong>
        张</template
      >
      <span v-if="operation.status.failed">
        · {{ operation.status.failed.toLocaleString() }} 项失败</span
      ><span v-if="operation.status.skipped">
        · {{ operation.status.skipped.toLocaleString() }} 项跳过</span
      >
    </p>
    <p v-if="!active && incomplete" class="duplicate-stage-hint">
      {{
        operation.preview.groupId.startsWith("workflow:")
          ? "已完成部分已保存。返回上方结果可继续整理剩余图片，或撤销本次已完成的整理。"
          : "已完成部分已保存，可继续原批次或撤销已完成的整理。"
      }}
    </p>
    <div class="duplicate-task-actions">
      <button
        v-if="state === 'running'"
        class="soft-button compact"
        @click="emit('control', 'pause')"
      >
        暂停
      </button>
      <button
        v-if="state === 'paused'"
        class="soft-button compact"
        @click="emit('control', 'resume')"
      >
        继续
      </button>
      <button
        v-if="['queued', 'running', 'pausing', 'paused'].includes(state)"
        class="text-button compact"
        @click="emit('control', 'cancel')"
      >
        停止本次整理
      </button>
      <button
        v-if="resumable"
        class="soft-button compact duplicate-resume"
        :disabled="busy"
        @click="emit('retry')"
      >
        继续未完成批次
      </button>
      <button
        v-if="!active && operation.status?.undoable"
        class="soft-button compact duplicate-resume"
        :disabled="busy"
        @click="emit('undo')"
      >
        撤销整个批次
      </button>
      <button
        class="text-button compact duplicate-refresh"
        :disabled="operation.loading"
        @click="emit('refresh')"
      >
        刷新
      </button>
    </div>
    <p
      v-if="operation.error && !errorInDialog"
      class="error-message"
      role="alert"
    >
      {{ operation.error }}
    </p>
    <details
      v-if="operation.items.length || operation.status?.errors.length"
      class="duplicate-failure-list"
    >
      <summary>
        查看处理提示{{
          operation.items.length ? `（${operation.items.length}）` : ""
        }}
      </summary>
      <p
        v-for="(message, index) in operation.status?.errors ?? []"
        :key="index"
        class="error-text"
      >
        {{ message }}
      </p>
      <article v-for="item in operation.items" :key="item.ordinal">
        <strong>{{ item.fileName || "未知文件" }}</strong>
        <p>
          {{ item.message || item.code
          }}<span v-if="item.fileMoved && !item.dbUpdated">
            · 原文件已移走，图库记录待修复。</span
          >
        </p>
        <small v-if="item.path" class="path-text">{{ item.path }}</small>
      </article>
      <button
        v-if="operation.nextCursor"
        class="soft-button compact"
        :disabled="operation.loading"
        @click="emit('more')"
      >
        {{ operation.loading ? "读取中…" : "查看更多提示（每次 50 项）" }}
      </button>
    </details>
  </section>
</template>
<style scoped>
.duplicate-operation-panel {
  border: 0;
  border-radius: 8px;
  background: var(--hover);
  padding: 20px 22px;
  margin: 0 0 22px;
}
.duplicate-operation-panel > header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}
.duplicate-task-kicker {
  display: block;
  font-size: 11px;
  color: var(--muted);
  margin-bottom: 7px;
}
.duplicate-operation-panel h2 {
  font-size: 15px;
  font-weight: 600;
  margin: 0;
}
.duplicate-task-state {
  font-size: 12px;
  color: var(--muted);
}
.duplicate-stage {
  margin-top: 19px;
}
.duplicate-stage-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 8px;
  font-size: 12px;
}
.duplicate-stage-numbers {
  font-variant-numeric: tabular-nums;
}
.duplicate-stage-numbers small {
  margin-left: 10px;
  color: var(--muted);
}
.duplicate-operation-panel progress {
  height: 5px;
  margin: 10px 0 0;
}
.duplicate-operation-panel .duplicate-stage-hint {
  color: var(--muted);
  font-size: 12px;
  margin: 10px 0 0;
  line-height: 1.7;
}
.duplicate-operation-panel .duplicate-batch-outcome {
  margin-top: 16px;
  font-size: 13px;
}
.duplicate-task-actions {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  margin: 14px -10px -4px;
}
.duplicate-task-actions .soft-button {
  background: var(--surface);
}
.duplicate-task-actions .duplicate-resume {
  border-color: var(--border);
  margin-left: 10px;
}
.duplicate-refresh {
  margin-left: auto;
  color: var(--muted);
}
.duplicate-failure-list {
  margin-top: 16px;
  font-size: 12px;
}
.duplicate-failure-list summary {
  cursor: pointer;
  color: var(--muted);
}
@media (max-width: 719px) {
  .duplicate-operation-panel {
    padding: 18px;
  }
}
</style>
