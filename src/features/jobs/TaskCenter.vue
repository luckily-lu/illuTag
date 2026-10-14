<script setup lang="ts">
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";
defineProps<{ rows: JobRecord[]; error: string; thumbnails: boolean }>();
const emit = defineEmits<{
  start: [kind: string, duplicateOptions?: DuplicateOptions | null];
  duplicates: [];
  control: [id: string, action: string];
}>();
const names: Record<string, string> = {
  pipeline: "一键处理",
  scan: "扫描来源",
  thumbnails: "生成缩略图",
  semantic: "自然语言图片索引",
  tagging: "自动打标",
  cleanup: "清理缺失索引",
  rules: "重新应用分类规则",
  duplicates: "重复与近似图片检索",
};
const states: Record<string, string> = {
  queued: "排队中",
  running: "处理中",
  paused: "已暂停",
  cancelling: "正在停止",
  cancelled: "已取消",
  interrupted: "已中断",
  completed: "已完成",
  partial: "部分完成",
  failed: "失败",
};
</script>
<template>
  <section class="page task-center">
    <header class="page-header">
      <div>
        <p class="page-kicker">图库维护</p>
        <h1>任务中心</h1>
        <p>按需处理你的图片。任务在切换页面后继续运行。</p>
      </div>
      <button class="soft-button primary" @click="emit('start', 'pipeline')">
        一键处理
      </button>
      <button class="soft-button" @click="emit('duplicates')">重复检索</button>
    </header>
    <div class="task-launchers">
      <button
        v-for="kind in ['scan', 'thumbnails', 'semantic', 'tagging', 'rules']"
        :key="kind"
        class="soft-button"
        @click="emit('start', kind)"
      >
        {{ names[kind] }}
      </button>
    </div>
    <p v-if="!thumbnails" class="muted">
      当前未启用缩略图显示，一键处理将跳过缩略图阶段。
    </p>
    <p v-if="error" class="error-message" role="alert">{{ error }}</p>
    <div v-if="!rows.length" class="empty-state">
      <h2>暂时没有任务</h2>
      <p>登记来源后，可以先扫描，也可以一次完成索引和打标。</p>
    </div>
    <article v-for="job in rows" :key="job.id" class="job-row">
      <div class="job-main">
        <div>
          <h3>{{ names[job.request.kind] || job.request.kind }}</h3>
          <p>
            {{ states[job.state] || job.state }} ·
            {{ names[job.stage] || job.stage }}
          </p>
        </div>
        <div class="job-actions">
          <button
            v-if="job.state === 'running' && job.stage !== 'cleanup'"
            class="soft-button compact"
            @click="emit('control', job.id, 'pause')"
          >
            暂停</button
          ><button
            v-if="job.state === 'paused'"
            class="soft-button compact"
            @click="emit('control', job.id, 'resume')"
          >
            继续</button
          ><button
            v-if="
              ['queued', 'running', 'paused'].includes(job.state) &&
              job.stage !== 'cleanup'
            "
            class="soft-button compact"
            @click="emit('control', job.id, 'cancel')"
          >
            取消</button
          ><button
            v-if="
              ['interrupted', 'partial', 'cancelled', 'failed'].includes(
                job.state,
              )
            "
            class="soft-button compact"
            @click="
              emit('start', job.request.kind, job.request.duplicateOptions)
            "
          >
            重新运行
          </button>
        </div>
      </div>
      <progress v-if="job.total > 0" :value="job.processed" :max="job.total" />
      <p class="job-stats">
        {{ job.processed.toLocaleString()
        }}<template v-if="job.total">
          / {{ job.total.toLocaleString() }}</template
        >
        已处理<span v-if="job.failures"> · {{ job.failures }} 项失败</span>
      </p>
      <details v-if="job.errors.length">
        <summary>查看错误</summary>
        <p v-for="(error, index) in job.errors" :key="index">{{ error }}</p>
      </details>
    </article>
  </section>
</template>
