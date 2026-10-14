<script setup lang="ts">
import { computed } from "vue";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type { ReviewOperation } from "./useDuplicateReview";
import { stateNames } from "./format";
const props = defineProps<{
  operation: ReviewOperation;
  job?: JobRecord;
  busy: boolean;
}>();
const emit = defineEmits<{
  refresh: [];
  more: [];
  undo: [];
  system: [];
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
  ["queued", "running", "paused", "cancelling"].includes(state.value),
);
</script>
<template>
  <section class="duplicate-operation-panel" aria-label="本次处理结果">
    <header>
      <strong>{{
        operation.preview.action === "restore"
          ? "撤销软回收"
          : operation.preview.action === "systemTrash"
            ? "清理原文件"
            : "本次软回收"
      }}</strong
      ><span>{{ stateNames[state] || state }}</span
      ><span class="spacer" /><button
        class="text-button compact"
        :disabled="operation.loading"
        @click="emit('refresh')"
      >
        刷新结果
      </button>
    </header>
    <p v-if="operation.status">
      成功 {{ operation.status.succeeded.toLocaleString() }} · 失败
      {{ operation.status.failed.toLocaleString() }} · 跳过
      {{ operation.status.skipped.toLocaleString() }} ·
      {{ operation.status.processed.toLocaleString() }} /
      {{ operation.status.total.toLocaleString() }} 已处理
    </p>
    <p v-else-if="job">
      {{ job.processed.toLocaleString() }} /
      {{ job.total.toLocaleString() }} 已处理
    </p>
    <progress
      v-if="active && (job?.total || operation.status?.total)"
      :value="job?.processed ?? operation.status?.processed ?? 0"
      :max="job?.total || operation.status?.total || 1"
    />
    <div class="button-row">
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
        v-if="['queued', 'running', 'paused'].includes(state)"
        class="soft-button compact"
        @click="emit('control', 'cancel')"
      >
        停止本次处理
      </button>
      <button
        v-if="!active && operation.status?.undoable"
        class="soft-button compact"
        :disabled="busy"
        @click="emit('undo')"
      >
        撤销本次软回收
      </button>
      <button
        v-if="
          !active &&
          operation.preview.action === 'trash' &&
          (operation.status?.succeeded ?? 0) > 0
        "
        class="soft-button compact danger"
        :disabled="busy || !operation.status?.undoable"
        @click="emit('system')"
      >
        清理原文件到系统回收站
      </button>
      <button
        v-if="
          !active &&
          operation.preview.action !== 'restore' &&
          (['partial', 'cancelled', 'interrupted', 'failed'].includes(state) ||
            (operation.status?.failed ?? 0) > 0)
        "
        class="soft-button compact"
        :disabled="busy"
        @click="emit('retry')"
      >
        重新检查未完成项
      </button>
    </div>
    <p
      v-if="
        !active &&
        ['partial', 'cancelled', 'interrupted', 'failed'].includes(state)
      "
      class="duplicate-warning"
    >
      成功项已提交，未完成项保留。本组不会自动进入下一组；可以查看原因后重新准备操作。
    </p>
    <p
      v-if="operation.status?.affectedGroupsTruncated"
      class="duplicate-warning"
    >
      关联分组较多，已重新核对当前加载的分组。后续分组会按需校验。
    </p>
    <p
      v-for="(message, index) in operation.status?.errors ?? []"
      :key="index"
      class="error-text"
    >
      {{ message }}
    </p>
    <p v-if="operation.error" class="error-message" role="alert">
      {{ operation.error }}
    </p>
    <details v-if="operation.items.length" class="duplicate-failure-list">
      <summary>失败与跳过项 · 已加载 {{ operation.items.length }} 条</summary>
      <article v-for="item in operation.items" :key="item.ordinal">
        <strong>{{ item.fileName || "未知文件" }}</strong>
        <p>
          {{ item.message || item.code
          }}<span v-if="item.fileMoved && !item.dbUpdated">
            · 原文件已移走，图库记录待修复；重试会复核状态。</span
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
        {{
          operation.loading ? "读取中…" : "继续查看失败与跳过项（每次 50 项）"
        }}
      </button>
    </details>
  </section>
</template>
