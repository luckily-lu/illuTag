<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { api, errorMessage } from "../../shared/ipc";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";
import type { LibraryFolder } from "../../shared/contracts/LibraryFolder";
import SoftSelect from "../../shared/ui/SoftSelect.vue";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
import ErrorDialog from "../../shared/ui/ErrorDialog.vue";
import AppIcon from "../../shared/ui/AppIcon.vue";
import {
  capability,
  isActive,
  jobNames,
  stateNames,
  stageLabel,
  unitLabel,
  acceptsJob,
  durationLabel,
} from "./jobPresentation";
import "./tasks.css";
const props = defineProps<{
  rows: JobRecord[];
  error: string;
  thumbnails: boolean;
  sources?: LibraryFolder[];
  controlling?: Set<string>;
}>();
const emit = defineEmits<{
  start: [kind: string, options?: DuplicateOptions | null];
  duplicates: [operationId?: string | null];
  control: [id: string, action: string];
  retry: [id: string];
  historyCleared: [];
  recycleBin: [];
  recycleRetry: [operationId: string, kind: string];
}>();
const current = computed(() =>
  props.rows.find((j) => isActive(j) && j.state !== "queued"),
);
const waiting = computed(() =>
  props.rows
    .filter((j) => j.state === "queued")
    .sort(
      (a, b) =>
        (a.meta?.createdAt ?? a.updatedAt) -
          (b.meta?.createdAt ?? b.updatedAt) || a.id.localeCompare(b.id),
    ),
);
const history = ref<JobRecord[]>([]),
  cursor = ref<string>(),
  filter = ref("all"),
  loading = ref(false),
  failure = ref(""),
  clearing = ref(false),
  confirmClear = ref(false),
  launcher = ref(false);
let epoch = 0,
  disposed = false,
  historyRefreshPending = false;
const launchKinds = ["scan", "thumbnails", "semantic", "tagging", "rules"];
const filters = [
  { value: "all", label: "全部历史" },
  ...["completed", "partial", "failed", "cancelled", "interrupted"].map(
    (value) => ({ value, label: stateNames[value] }),
  ),
];
function scope(j: JobRecord) {
  const id = j.request.sourceFolderId;
  if (id === null) return "全部图库";
  const source = props.sources?.find((f) => f.id === id);
  return source?.path ?? `来源 #${id}`;
}
function queueState(j: JobRecord) {
  if (j.meta?.persistenceError) return "等待保存当前结果";
  if (current.value?.state === "paused") return "等待当前任务继续";
  const stage = j.stage?.trim();
  return !stage || stage === "等待处理" ? "等待执行" : stage;
}
function queueWarning(j: JobRecord) {
  return queueState(j).startsWith("队列领取失败");
}
function duration(j: JobRecord) {
  return durationLabel(j);
}
function created(j: JobRecord) {
  return new Date(j.meta?.createdAt ?? j.updatedAt).toLocaleString();
}
function retryLabel(j: JobRecord) {
  return j.request.duplicateOperationId || j.request.recycleOperationId
    ? "继续原批次"
    : "重新运行";
}
async function readHistory(append = false) {
  if (append && (!cursor.value || loading.value)) return;
  const token = ++epoch;
  loading.value = true;
  failure.value = "";
  try {
    const page = await api.jobHistory({
      state: filter.value,
      limit: 20,
      ...(append ? { cursor: cursor.value } : {}),
    });
    if (disposed || token !== epoch) return;
    const map = new Map((append ? history.value : []).map((j) => [j.id, j]));
    for (const row of page.rows)
      if (acceptsJob(map.get(row.id), row)) map.set(row.id, row);
    history.value = [...map.values()];
    cursor.value = page.nextCursor;
  } catch (e) {
    if (!disposed && token === epoch) failure.value = errorMessage(e);
  } finally {
    if (token === epoch) {
      loading.value = false;
      if (historyRefreshPending && !disposed) {
        historyRefreshPending = false;
        void readHistory();
      }
    }
  }
}
async function clearHistory() {
  if (clearing.value) return;
  clearing.value = true;
  failure.value = "";
  try {
    await api.clearJobHistory();
    confirmClear.value = false;
    history.value = [];
    cursor.value = undefined;
    emit("historyCleared");
    await readHistory();
  } catch (e) {
    failure.value = errorMessage(e);
  } finally {
    clearing.value = false;
  }
}
watch(filter, () => readHistory());
watch(
  () =>
    props.rows
      .filter((j) => !isActive(j))
      .map((j) => `${j.id}:${j.meta?.revision ?? j.updatedAt}`)
      .join("|"),
  () => {
    if (loading.value) historyRefreshPending = true;
    else void readHistory();
  },
);
function outsideLauncher(event: Event) {
  if (event instanceof KeyboardEvent) {
    if (event.key === "Escape") launcher.value = false;
  } else if (!(event.target as HTMLElement)?.closest(".task-launch-menu"))
    launcher.value = false;
}
onMounted(() => {
  void readHistory();
  window.addEventListener("pointerdown", outsideLauncher);
  window.addEventListener("keydown", outsideLauncher);
});
onUnmounted(() => {
  window.removeEventListener("pointerdown", outsideLauncher);
  window.removeEventListener("keydown", outsideLauncher);
  disposed = true;
  ++epoch;
});
</script>
<template>
  <section class="page task-center task-workbench">
    <header class="task-heading">
      <div>
        <h1>任务中心</h1>
        <p>查看执行进度，管理等待队列与处理记录。</p>
      </div>
      <div class="task-launch-actions">
        <button class="soft-button primary" @click="emit('start', 'pipeline')">
          一键处理
        </button>
        <div class="task-launch-menu">
          <button
            class="soft-button"
            :aria-expanded="launcher"
            @click="launcher = !launcher"
          >
            其他任务<AppIcon name="down" :size="15" />
          </button>
          <div v-if="launcher" class="task-launch-popover">
            <button
              v-for="kind in launchKinds"
              :key="kind"
              class="text-button"
              @click="
                launcher = false;
                emit('start', kind);
              "
            >
              {{ jobNames[kind] }}</button
            ><button
              class="text-button"
              @click="
                launcher = false;
                emit('duplicates');
              "
            >
              重复检索
            </button>
          </div>
        </div>
      </div>
    </header>
    <p v-if="!thumbnails" class="muted task-preference-note">
      一键处理会按当前设置跳过缩略图阶段。
    </p>
    <section class="task-current" aria-label="当前任务">
      <header>
        <h2>当前任务</h2>
        <span>{{
          current?.meta?.persistenceError
            ? "结果待保存"
            : current
              ? stateNames[current.state]
              : "空闲"
        }}</span>
      </header>
      <template v-if="current"
        ><div class="task-current-main">
          <div>
            <h3>
              {{ jobNames[current.request.kind] || current.request.kind }}
            </h3>
            <p class="task-scope" :title="scope(current)">
              {{ scope(current) }}
            </p>
          </div>
          <div class="task-control-actions">
            <button
              v-if="
                current.meta?.persistenceError && capability(current, 'Retry')
              "
              class="soft-button"
              :disabled="controlling?.has(current.id)"
              @click="emit('control', current.id, 'retrySave')"
            >
              重试保存
            </button>
            <button
              v-if="capability(current, 'Pause')"
              class="soft-button"
              :disabled="controlling?.has(current.id)"
              @click="emit('control', current.id, 'pause')"
            >
              暂停</button
            ><button
              v-if="capability(current, 'Resume')"
              class="soft-button"
              :disabled="controlling?.has(current.id)"
              @click="emit('control', current.id, 'resume')"
            >
              继续</button
            ><button
              v-if="capability(current, 'Cancel')"
              class="text-button danger"
              :disabled="controlling?.has(current.id)"
              @click="emit('control', current.id, 'cancel')"
            >
              停止
            </button>
          </div>
        </div>
        <div class="task-phase">
          <span>{{ stageLabel(current) }}</span
          ><span
            >{{ current.processed.toLocaleString()
            }}<template v-if="current.total > 0">
              / {{ current.total.toLocaleString() }}</template
            >
            {{ unitLabel(current) }}</span
          >
        </div>
        <progress
          v-if="current.total > 0"
          :value="current.processed"
          :max="current.total"
        /><progress v-else aria-label="正在处理，尚无确定总量" />
        <div
          v-if="current.meta?.persistenceError"
          class="task-warning"
          role="status"
        >
          <p>
            处理已结束，结果尚未保存，等待队列已暂停。重试保存会继续提交结果，不会重复处理图片。
          </p>
          <details>
            <summary>技术详情</summary>
            <p>{{ current.meta.persistenceError }}</p>
          </details>
        </div>
        <p v-else-if="current.state === 'pausing'" class="muted">
          已请求暂停，等待当前安全处理点。
        </p>
        <p v-if="current.state === 'paused'" class="muted">
          当前任务已暂停，等待队列也会等待。继续或停止此任务后再执行后续任务。
        </p>
        <p v-if="current.state === 'cancelling'" class="muted">
          正在停止，已提交的处理结果会保留。
        </p>
        <p v-if="current.failures" class="task-warning">
          {{ current.failures.toLocaleString() }} 项未成功，结束后可查看详情。
        </p>
      </template>
      <p v-else class="muted">当前没有正在执行的任务。</p>
    </section>
    <section class="task-queue" aria-label="等待队列">
      <header>
        <h2>
          等待队列 <small>{{ waiting.length }}</small>
        </h2>
        <span>按加入顺序执行</span>
      </header>
      <p v-if="!waiting.length" class="muted task-empty-line">暂无等待任务。</p>
      <article
        v-for="(job, index) in waiting"
        :key="job.id"
        class="task-queue-row"
      >
        <span class="task-order">{{ index + 1 }}</span>
        <div>
          <strong>{{ jobNames[job.request.kind] || job.request.kind }}</strong>
          <p class="task-scope" :title="scope(job)">{{ scope(job) }}</p>
        </div>
        <span
          class="muted"
          :class="{ 'task-warning': queueWarning(job) }"
          role="status"
          >{{ queueState(job) }}</span
        ><button
          class="text-button"
          :disabled="controlling?.has(job.id)"
          @click="emit('control', job.id, 'cancel')"
        >
          取消排队
        </button>
      </article>
    </section>
    <section class="task-history" aria-label="历史记录">
      <header>
        <h2>历史记录</h2>
        <div class="task-history-actions">
          <SoftSelect
            v-model="filter"
            :options="filters"
            aria-label="筛选任务历史"
          /><button class="text-button danger" @click="confirmClear = true">
            清除记录…
          </button>
        </div>
      </header>
      <p
        v-if="loading && !history.length"
        class="muted task-empty-line"
        role="status"
      >
        正在读取历史记录…
      </p>
      <p v-else-if="!history.length" class="muted task-empty-line">
        暂无符合条件的任务记录。
      </p>
      <article
        v-for="job in history"
        :key="job.id"
        class="task-history-row"
        :data-job-id="job.id"
      >
        <div class="task-history-main">
          <div>
            <strong>{{
              jobNames[job.request.kind] || job.request.kind
            }}</strong>
            <p class="task-scope" :title="scope(job)">{{ scope(job) }}</p>
          </div>
          <span
            class="task-history-state"
            :class="{
              warning: ['failed', 'partial', 'interrupted'].includes(job.state),
            }"
            >{{ stateNames[job.state] || job.state }}</span
          ><span class="task-history-stat"
            >{{ job.processed.toLocaleString() }} {{ unitLabel(job)
            }}<small v-if="job.failures">{{ job.failures }} 项失败</small></span
          ><span class="task-duration">{{ duration(job) }}</span>
          <div class="task-control-actions">
            <button
              v-if="capability(job, 'Retry')"
              class="text-button"
              :disabled="controlling?.has(job.id)"
              @click="emit('retry', job.id)"
            >
              {{ retryLabel(job) }}</button
            ><button
              v-if="
                ['duplicates', 'duplicateManagement', 'duplicateUndo'].includes(
                  job.request.kind,
                )
              "
              class="text-button"
              @click="emit('duplicates', job.request.duplicateOperationId)"
            >
              查看结果</button
            ><button
              v-if="
                ['recycleRestore', 'recycleDelete'].includes(job.request.kind)
              "
              class="text-button"
              @click="emit('recycleBin')"
            >
              查看回收站
            </button>
          </div>
        </div>
        <details>
          <summary>
            详情<span v-if="job.errors.length">
              · {{ job.errors.length }} 条提示</span
            >
          </summary>
          <div class="task-detail">
            <p>创建于 {{ created(job) }} · 用时 {{ duration(job) }}</p>
            <p>{{ stageLabel(job) }}</p>
            <p v-for="(line, index) in job.errors" :key="index">{{ line }}</p>
          </div>
        </details>
      </article>
      <button
        v-if="cursor"
        class="text-button task-history-more"
        :disabled="loading"
        @click="readHistory(true)"
      >
        {{ loading ? "读取中…" : "加载更多记录" }}
      </button>
    </section>
    <Teleport to="body"
      ><SoftDialog
        v-if="confirmClear"
        title="清除任务历史记录"
        @close="!clearing && (confirmClear = false)"
        ><p>
          清除已完成、失败、停止及中断的任务记录。正在执行和排队的任务会保留，图片、整理撤销记录均会保留。
        </p>
        <p>此操作无法撤销。</p>
        <footer>
          <button
            class="soft-button"
            :disabled="clearing"
            @click="confirmClear = false"
          >
            取消</button
          ><button
            class="soft-button danger"
            :disabled="clearing"
            @click="clearHistory"
          >
            {{ clearing ? "正在清除…" : "清除记录" }}
          </button>
        </footer></SoftDialog
      ></Teleport
    >
    <ErrorDialog
      :errors="[error, failure].filter(Boolean)"
      title="任务操作未完成"
      @close="failure = ''"
    />
  </section>
</template>
