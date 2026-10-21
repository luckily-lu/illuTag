<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";
import type { DuplicateRoundStatus } from "../../shared/contracts/DuplicateRoundStatus";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type { UserFolder } from "../../shared/contracts/UserFolder";
import AppIcon from "../../shared/ui/AppIcon.vue";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
import ErrorDialog from "../../shared/ui/ErrorDialog.vue";
import SoftSelect from "../../shared/ui/SoftSelect.vue";
import DuplicateOperationPanel from "./DuplicateOperationPanel.vue";
import DuplicateWorkflowReview from "./DuplicateWorkflowReview.vue";
import type { DuplicateBatches } from "./useDuplicateBatches";
import { useDuplicateOperations } from "./useDuplicateOperations";
import { useDuplicateWorkflow } from "./useDuplicateWorkflow";
import type { useDuplicates } from "./useDuplicates";
import { bytes, checkedDate, stateName } from "./format";
import { api, errorMessage } from "../../shared/ipc";
const props = defineProps<{
  jobs: JobRecord[];
  refreshVersion: number;
  taskError: string;
  startJob: (options: DuplicateOptions) => Promise<boolean>;
  workspace: ReturnType<typeof useDuplicates>;
  batches: DuplicateBatches;
  folders: UserFolder[];
  controlJob: (id: string, action: string) => Promise<void>;
}>();
const emit = defineEmits<{ tasks: []; jobCreated: [job: JobRecord] }>();
const w = useDuplicateWorkflow(),
  batches = props.batches;
const section = ref<HTMLElement>(),
  advanced = ref(false),
  force = ref(false);
const starting = ref(false),
  submitting = ref(false);
const preparingKind = ref<"exact" | "near" | null>(null);
const confirmClear = ref(false),
  confirmRecords = ref(false),
  localError = ref(""),
  notice = ref("");
const refreshingResults = ref(false);
const sampleMode = !isTauri();
let overviewScroll = 0;
const management = useDuplicateOperations(props.workspace, batches, {
  jobs: () => props.jobs,
  created: (job) => emit("jobCreated", job),
});
const activeJob = computed(() =>
  props.jobs.find(
    (job) =>
      job.request.kind === "duplicates" &&
      ["queued", "running", "pausing", "paused", "cancelling"].includes(
        job.state,
      ),
  ),
);
const activeWork = computed(() =>
  props.jobs.some(
    (job) =>
      ["duplicates", "duplicateManagement", "duplicateUndo"].includes(
        job.request.kind,
      ) &&
      ["queued", "running", "pausing", "paused", "cancelling"].includes(
        job.state,
      ),
  ),
);
const activeOperation = computed(() => {
  const id = props.jobs.find(
    (job) =>
      job.request.duplicateOperationId &&
      ["queued", "running", "pausing", "paused", "cancelling"].includes(
        job.state,
      ),
  )?.request.duplicateOperationId;
  return (
    batches.operations.value.get(
      id ?? batches.currentOperationId.value ?? "",
    ) ?? null
  );
});
const lastOperation = computed(
  () =>
    batches.operations.value.get(w.status.value?.lastOperationId ?? "") ??
    (activeOperation.value?.preview.groupId.startsWith("workflow:")
      ? activeOperation.value
      : null),
);
const undoable = computed(() => !!lastOperation.value?.status?.undoable);
const operationRunning = computed(
  () =>
    !!activeOperation.value &&
    ["queued", "running", "pausing", "paused", "cancelling"].includes(
      props.jobs.find((job) => job.id === activeOperation.value?.jobId)
        ?.state ??
        activeOperation.value.status?.state ??
        activeOperation.value.jobState,
    ),
);
const busy = computed(
  () =>
    starting.value ||
    submitting.value ||
    w.statusLoading.value ||
    ["loading", "error"].includes(w.status.value?.stage ?? "") ||
    !!activeJob.value ||
    management.busy.value ||
    operationRunning.value,
);
// Refinement writes unpublished classifications. Manual choices are serialized
// by the backend and merged when that classification is published.
const reviewBlockedReason = computed(() => {
  if (
    submitting.value ||
    management.submitting.value ||
    management.preparing.value ||
    operationRunning.value ||
    props.jobs.some(
      (job) =>
        ["duplicateManagement", "duplicateUndo"].includes(job.request.kind) &&
        ["queued", "running", "pausing", "paused", "cancelling"].includes(
          job.state,
        ),
    )
  )
    return "正在整理或撤销图片，暂不可修改保留项；仍可切组和查看原图。";
  const mode = activeJob.value?.request.duplicateOptions?.verificationMode;
  const cascadeReviewReady = mode === "cascade" &&
    ["第一轮", "第二轮", "第三轮", "生成分类整理计划"].some((prefix) =>
      activeJob.value?.stage?.startsWith(prefix),
    );
  if (activeJob.value && !(["refine", "organize"].includes(mode ?? "") || cascadeReviewReady))
    return "全库检索正在更新图片指纹，暂不可修改保留项；仍可浏览已发布结果。";
  if (starting.value) return "正在启动任务，请稍后修改保留项。";
  return "";
});
const reviewNotice = computed(() =>
  ["refine", "organize", "cascade"].includes(
    activeJob.value?.request.duplicateOptions?.verificationMode ?? "",
  )
    ? "检索在后台进行，可浏览已保存结果；最终发布会合并最新人工选择。"
    : "",
);
const scan = computed(() => w.status.value?.scan),
  stage = computed(() => w.status.value?.stage ?? "empty");
const currentPolicy = computed(
  () => w.status.value?.policy === "illuTag-three-pass-v1",
);
const rows = computed(() =>
  ["exact", "near", "similar", "uncertain"].map((kind) => {
    const item = w.status.value?.categories.find((row) => row.kind === kind);
    return {
      kind,
      images: item?.images ?? 0,
      groups: item?.groups ?? 0,
      cleanupCount: item?.cleanupCount ?? 0,
    };
  }),
);
const names: Record<string, string> = {
  exact: "完全相同",
  near: "近似图片",
  similar: "类似图片",
  uncertain: "无法确认",
};
const descriptions: Record<string, string> = {
  exact: "尺寸与全部像素一致，每组保留一张。",
  near: "重压缩、等比例缩放等近似版本，可先查看并调整保留项。",
  similar: "存在内容或局部细节差异，默认保留。",
  uncertain: "读取失败或证据不足，保留图片，不视为内容不同。",
};
const roundNames = ["宽筛候选", "收紧内容差异", "严格确认相同"];
const liveRound = computed(() => {
  if (!activeJob.value) return null;
  const stage = activeJob.value.stage;
  if (/第三轮|第3轮|严格|零容差|完整像素/.test(stage)) return 3;
  if (/第二轮|第2轮|收紧|容差复检|内容复检/.test(stage)) return 2;
  return 1;
});
const rounds = computed(() =>
  roundNames.map((name, index) => {
    const round = index + 1;
    const saved = currentPolicy.value
      ? w.status.value?.rounds?.find(
          (r: DuplicateRoundStatus) => r.round === round,
        )
      : undefined;
    if (activeJob.value)
      return {
        round,
        name,
        counts: null,
        state:
          round < liveRound.value!
            ? "completed"
            : round === liveRound.value
              ? activeJob.value.state
              : "pending",
      };
    return {
      round,
      name,
      state:
        saved?.state ??
        (currentPolicy.value && stage.value === "refined"
          ? "completed"
          : "pending"),
      counts: saved ?? null,
    };
  }),
);
const roundState = (value: string) =>
  value === "pending" ? "待开始" : stateName(value);
const recentAttempt = computed(() => {
  const job = props.jobs.find((row) => row.request.kind === "duplicates");
  if (
    job &&
    ["partial", "failed", "cancelled", "interrupted"].includes(job.state)
  )
    return {
      state: job.state,
      errors: job.errors,
      label:
        job.request.duplicateOptions?.verificationMode === "cascade"
          ? "三轮检索"
          : job.request.duplicateOptions?.verificationMode === "refine"
            ? "近似图片复检"
            : "全库检索",
    };
  if (job?.state === "completed") return null;
  return scan.value?.lastAttempt
    ? { ...scan.value.lastAttempt, label: "全库检索" }
    : null;
});
const failed = (state: string) =>
  ["partial", "failed", "interrupted"].includes(state);
const errors = computed(() => {
  const jobs = ["duplicates", "duplicateManagement", "duplicateUndo"]
    .map((kind) => props.jobs.find((job) => job.request.kind === kind))
    .filter((job) => job && failed(job.state));
  const operations = new Set([activeOperation.value, batches.latest.value]);
  return [
    localError.value,
    w.error.value,
    management.error.value,
    props.taskError,
    ...(w.status.value?.errors ?? []),
    ...jobs.flatMap((job) =>
      job!.errors.length
        ? job!.errors
        : ["重复图片任务未完成，请到任务中心查看详情。"],
    ),
    ...(recentAttempt.value && failed(recentAttempt.value.state)
      ? recentAttempt.value.errors
      : []),
    ...[...operations].flatMap((operation) =>
      operation
        ? [
            operation.error,
            ...(failed(operation.status?.state ?? operation.jobState)
              ? (operation.status?.errors ?? [])
              : []),
          ]
        : [],
    ),
  ];
});
const errorDescription = computed(() =>
  errors.value.some((message) =>
    /database or disk is full|缓存.*容量|磁盘.*空间/i.test(message),
  )
    ? "查重缓存暂时无法继续写入。已保存的结果和检查记录保留，详情中可查看容量或磁盘原因。"
    : errors.value.some((message) =>
    /invalid uri authority|unable to open database file/i.test(message),
  )
    ? "暂时无法读取图库数据。请确认数据目录可访问，再刷新结果。"
    : "操作遇到问题。可展开查看详情，或刷新已保存的结果。",
);
const operationOptions = computed(() =>
  [...batches.operations.value.values()].reverse().map((op) => ({
    value: op.preview.operationId,
    label: `${op.preview.action === "restore" ? "撤销整理" : op.preview.groupId.startsWith("workflow:") && op.preview.groupId.endsWith(":exact") ? "整理完全相同" : op.preview.groupId.startsWith("workflow:") && op.preview.groupId.endsWith(":near") ? "整理近似图片" : "历史整理"} · ${stateName(op.status?.state ?? op.jobState)}`,
  })),
);
async function hydrateLast() {
  const id = w.status.value?.lastOperationId;
  if (!id || batches.operations.value.has(id)) return;
  try {
    const status = await api.duplicateOperationStatus(id);
    const job = props.jobs.find(
      (job) => job.request.duplicateOperationId === id,
    ) ?? {
      id,
      request: {
        kind:
          status.action === "restore" ? "duplicateUndo" : "duplicateManagement",
        thumbnails: false,
        sourceFolderId: null,
        duplicateOptions: null,
        duplicateOperationId: id,
      },
      state: status.state,
      stage: status.stage,
      processed: status.processed,
      total: status.total,
      failures: status.failed,
      errors: status.errors,
      updatedAt: status.updatedAt,
    };
    batches.register(
      {
        operationId: id,
        generation: status.generation,
        groupId: status.groupId,
        action: status.action,
        targetCount: status.total,
        totalBytes: 0,
        protectedCount: 0,
        skippedCount: status.skipped,
        metadata: {
          favoriteCount: 0,
          classifiedCount: 0,
          customTaggedCount: 0,
          supplementTaggedCount: 0,
        },
        sample: [],
        warnings: [],
        expiresAt: 0,
        requiresSystemConfirmation: false,
      },
      job,
      false,
    );
    const op = batches.operations.value.get(id)!;
    op.status = status;
    op.handled = ![
      "queued",
      "running",
      "pausing",
      "paused",
      "cancelling",
    ].includes(status.state);
  } catch (e) {
    localError.value = errorMessage(e);
  }
}
let refreshPending: Promise<void> | null = null;
async function refresh(force = false, reload = false, retry = false) {
  // Progress already reaches the job row and batch event store. A status query
  // aggregates the full candidate pool, so only terminal/external changes reload it.
  if (!force && activeWork.value) return;
  if (refreshPending) return refreshPending;
  refreshPending = (async () => {
    await management.syncAll();
    await w.refresh(reload, retry);
    await hydrateLast();
  })().finally(() => {
    refreshPending = null;
  });
  return refreshPending;
}
async function refreshResults() {
  if (refreshingResults.value || activeWork.value) return;
  refreshingResults.value = true;
  localError.value = "";
  management.error.value = "";
  try {
    // Only read saved state. Never repeat a scan, review edit, cleanup or undo.
    await refresh(true, true, w.status.value?.stage === "error");
  } finally {
    refreshingResults.value = false;
  }
}
watch(
  [() => props.refreshVersion, () => batches.version.value],
  () => void refresh(),
);
watch(
  () => batches.activeOperationId.value,
  () => void management.syncAll(),
);
watch(
  () =>
    props.jobs
      .filter((job) =>
        ["duplicates", "duplicateManagement", "duplicateUndo"].includes(
          job.request.kind,
        ),
      )
      .map((job) => `${job.id}:${job.state}`)
      .join("|"),
  () => {
    if (activeWork.value) void management.syncAll();
    else void refresh();
  },
);
onMounted(async () => {
  await refresh(true);
  await nextTick();
  if (section.value) section.value.scrollTop = batches.scrollTop.value;
});
onUnmounted(() => {
  batches.scrollTop.value = section.value?.scrollTop ?? 0;
  w.dispose();
});
async function start() {
  if (busy.value) return;
  starting.value = true;
  localError.value = "";
  notice.value = "";
  try {
    if (await props.startJob(w.options("cascade", "standard", force.value))) {
      await w.refresh();
    }
  } catch (e) {
    localError.value = errorMessage(e);
  } finally {
    starting.value = false;
  }
}
async function cleanup(kind: "exact" | "near") {
  const snapshot = w.status.value;
  if (
    !snapshot?.generation ||
    !currentPolicy.value ||
    snapshot.stage !== "refined" ||
    busy.value ||
    w.editing.value
  )
    return;
  submitting.value = true;
  preparingKind.value = kind;
  localError.value = "";
  notice.value = "";
  try {
    const preview = await api.prepareDuplicateWorkflow(
      {
        generation: snapshot.generation,
        revision: snapshot.revision,
      },
      kind,
    );
    if (!preview.targetCount) {
      notice.value = "当前没有需要清理的图片。";
      await w.refresh();
      return;
    }
    const job = await api.applyDuplicateAction({
      operationId: preview.operationId,
      confirmed: true,
    });
    batches.register(preview, job);
    emit("jobCreated", job);
    await management.sync(batches.operations.value.get(preview.operationId)!);
    await w.refresh(true);
  } catch (e) {
    localError.value = errorMessage(e);
    await w.refresh(true);
  } finally {
    submitting.value = false;
    preparingKind.value = null;
  }
}
async function undo() {
  const op = lastOperation.value;
  if (!op || busy.value) return;
  await management.undo(op);
  await w.refresh(true);
}
async function openReview(kind: string) {
  overviewScroll = section.value?.scrollTop ?? 0;
  await w.open(kind);
}
async function closeReview() {
  w.close();
  await nextTick();
  if (section.value) section.value.scrollTop = overviewScroll;
  section.value
    ?.querySelector<HTMLElement>(`[data-review-kind="${w.kind.value}"]`)
    ?.focus({ preventScroll: true });
}
async function clearCache() {
  confirmClear.value = false;
  try {
    await api.clearDuplicateCache();
    await w.refresh(true);
    notice.value = "查重缓存已清理，图库原图与整理记录保留。";
  } catch (e) {
    localError.value = errorMessage(e);
  }
}
async function clearRecords() {
  confirmRecords.value = false;
  try {
    await api.clearDuplicateManagementRecords();
    batches.operations.value = new Map();
    batches.activeOperationId.value = null;
    batches.currentOperationId.value = null;
    notice.value = "整理记录已清理。";
    await w.refresh();
  } catch (e) {
    localError.value = errorMessage(e);
  }
}
</script>
<template>
  <section
    ref="section"
    class="page duplicate-page duplicate-workflow-page"
    :class="{ 'is-review': w.reviewOpen.value }"
  >
    <DuplicateWorkflowReview
      v-if="w.reviewOpen.value"
      :workflow="w"
      :busy="!!reviewBlockedReason"
      :blocked-reason="reviewBlockedReason"
      :notice="reviewNotice"
      :undoable="undoable"
      :undo-busy="busy"
      @close="closeReview"
      @undo="undo"
    />
    <template v-else>
      <header class="workflow-heading">
        <div>
          <h1>重复检索</h1>
          <p>宽筛候选，收紧差异，再严格确认完全相同。</p>
        </div>
        <button
          class="text-button"
          :aria-expanded="advanced"
          @click="advanced = !advanced"
        >
          <AppIcon name="more" :size="19" />更多
        </button>
      </header>
      <p v-if="sampleMode" class="workflow-fixture-note">
        布局预览 · 仅合成示例
      </p>
      <div class="workflow-run-summary">
        <span
          >{{
            scan?.coverage.processed?.toLocaleString() ?? "0"
          }}
          张已检查</span
        >
        <button
          v-if="currentPolicy || activeJob"
          class="soft-button primary"
          :disabled="busy"
          @click="start()"
        >
          {{ starting ? "准备中…" : "重新三轮检索" }}
        </button>
      </div>
      <ol class="workflow-rounds" aria-label="三轮检索流程">
        <li
          v-for="round in rounds"
          :key="round.round"
          :data-round="round.round"
          :class="{
            'is-complete': ['completed', 'complete'].includes(round.state),
            'is-active': [
              'running',
              'pausing',
              'paused',
              'cancelling',
            ].includes(round.state),
          }"
        >
          <span class="workflow-round-number"
            ><AppIcon
              v-if="['completed', 'complete'].includes(round.state)"
              name="check"
              :size="14"
            /><template v-else>{{ round.round }}</template></span
          >
          <div>
            <strong
              >第{{ ["一", "二", "三"][round.round - 1] }}轮 ·
              {{ round.name }}</strong
            >
            <small>{{ roundState(round.state) }}</small>
            <p v-if="round.counts">
              通过 {{ round.counts.passedPairs.toLocaleString() }} /
              {{ round.counts.inputPairs.toLocaleString() }} 对<span
                >排除 {{ round.counts.filteredPairs.toLocaleString() }} ·
                无法确认
                {{ round.counts.uncertainPairs.toLocaleString() }}</span
              >
            </p>
          </div>
        </li>
      </ol>
      <section
        v-if="activeJob"
        class="workflow-progress"
        aria-label="当前检索任务"
      >
        <div>
          <strong>三轮检索 · {{ stateName(activeJob.state) }}</strong
          ><span>{{ activeJob.stage }}</span
          ><small v-if="activeJob.total"
            >{{ activeJob.processed.toLocaleString() }} /
            {{ activeJob.total.toLocaleString() }}</small
          ><small v-else-if="activeJob.processed"
            >{{ activeJob.processed.toLocaleString() }}
            {{
              activeJob.meta?.unit === "pairs" ? "对已检查" : "张已检查"
            }}</small
          >
        </div>
        <progress
          v-if="activeJob.total"
          :value="activeJob.processed"
          :max="activeJob.total"
        /><progress v-else />
        <div class="workflow-progress-actions">
          <button
            v-if="activeJob.state === 'running'"
            class="text-button"
            @click="controlJob(activeJob.id, 'pause')"
          >
            暂停</button
          ><button
            v-if="activeJob.state === 'paused'"
            class="text-button"
            @click="controlJob(activeJob.id, 'resume')"
          >
            继续</button
          ><button
            v-if="
              ['queued', 'running', 'pausing', 'paused'].includes(
                activeJob.state,
              )
            "
            class="text-button"
            @click="controlJob(activeJob.id, 'cancel')"
          >
            停止检索</button
          ><button class="text-button" @click="emit('tasks')">任务中心</button>
        </div>
      </section>
      <section
        v-if="(!w.status.value && w.statusLoading.value) || stage === 'loading'"
        class="workflow-empty"
        role="status"
        aria-live="polite"
      >
        <h2>正在读取检索结果</h2>
        <p>正在同步已保存的计数与计划，检索任务的进度会独立更新。</p>
        <progress aria-label="读取检索结果" />
      </section>
      <section
        v-else-if="stage === 'error'"
        class="workflow-empty"
        role="status"
      >
        <h2>检索结果暂未读取完成</h2>
        <p>已有指纹和完整结果仍然保留，可以重试读取。</p>
        <button
          class="soft-button"
          :disabled="w.statusLoading.value"
          @click="w.refresh(false, true)"
        >
          重试读取
        </button>
      </section>
      <section v-else-if="stage === 'empty'" class="workflow-empty">
        <AppIcon name="duplicates" :size="32" />
        <h2>从三轮检索开始</h2>
        <p>
          先广泛找出相近图片，再排除细节差异，最后确认尺寸与全部像素相同的图片。检索不会直接回收图片。
        </p>
        <button class="soft-button primary" :disabled="busy" @click="start()">
          开始三轮检索
        </button>
      </section>
      <section
        v-else-if="!currentPolicy"
        class="workflow-empty workflow-legacy"
        role="status"
      >
        <h2>已保存旧版检索结果</h2>
        <p>
          旧版分类不直接作为本轮整理依据。重新执行三轮检索会复用有效指纹，重新区分完全相同、近似与类似图片。
        </p>
        <button class="soft-button primary" :disabled="busy" @click="start()">
          开始三轮检索
        </button>
        <small>人工保留和整理撤销记录会保留。</small>
      </section>
      <template v-else>
        <header class="workflow-results-heading">
          <div>
            <h2>三轮检索结果</h2>
            <p>
              {{ checkedDate(scan?.checkedAt ?? null) }} ·
              分类分组含参考图，图片可能跨组出现，不将各类张数相加
            </p>
          </div>
        </header>
        <section class="workflow-results" aria-label="三轮分类结果">
          <article
            v-for="row in rows"
            :key="row.kind"
            class="workflow-result-row"
            :data-category="row.kind"
          >
            <div class="workflow-result-name">
              <AppIcon
                :name="row.kind === 'exact' ? 'copy' : 'images'"
                :size="19"
              /><strong>{{ names[row.kind] }}</strong>
            </div>
            <div class="workflow-result-count">
              <strong>{{ row.images.toLocaleString() }}</strong
              ><span>张</span
              ><small
                >{{ row.groups.toLocaleString() }} 组<template
                  v-if="row.kind === 'exact' || row.kind === 'near'"
                >
                  · 可整理 {{ row.cleanupCount.toLocaleString() }} 张</template
                ></small
              >
            </div>
            <p>{{ descriptions[row.kind] }}</p>
            <div class="workflow-view-actions">
              <button
                v-if="row.kind === 'exact' || row.kind === 'near'"
                class="soft-button workflow-cleanup-button"
                :data-cleanup-kind="row.kind"
                :aria-label="
                  row.kind === 'exact'
                    ? '一键整理完全相同图片'
                    : '一键整理近似图片'
                "
                :disabled="
                  busy ||
                  w.editing.value ||
                  stage !== 'refined' ||
                  !row.cleanupCount
                "
                @click="cleanup(row.kind === 'exact' ? 'exact' : 'near')"
              >
                {{ preparingKind === row.kind ? "准备中…" : "一键整理" }}
              </button>
              <button
                class="text-button"
                :data-review-kind="row.kind"
                :aria-label="'查看' + names[row.kind]"
                :disabled="w.loading.value || !row.groups"
                @click="openReview(row.kind)"
              >
                查看<AppIcon name="right" :size="15" />
              </button>
            </div>
          </article>
        </section>
        <p class="workflow-evidence-note">
          完全相同与近似图片分别整理；近似图片可先进入“查看”人工审查。类似与无法确认项默认保留。整理仅移入应用回收站，可整批撤销，原文件暂时保留。
        </p>
        <p v-if="w.status.value?.protectedCount" class="workflow-evidence-note">
          {{ w.status.value.protectedCount.toLocaleString() }}
          张人工保留，整理计划以你的选择为准。
        </p>
        <div v-if="undoable" class="workflow-completed" role="status">
          <AppIcon name="check" :size="18" /><span
            >已整理
            {{ (lastOperation?.status?.succeeded ?? 0).toLocaleString() }}
            张图片，结果已自动更新。</span
          ><button class="text-button" :disabled="busy" @click="undo">
            撤销本次整理
          </button>
        </div>
      </template>
      <DuplicateOperationPanel
        v-if="
          activeOperation &&
          (operationRunning ||
            ['partial', 'cancelled', 'interrupted', 'failed'].includes(
              activeOperation.status?.state ?? activeOperation.jobState,
            ))
        "
        :operation="activeOperation"
        :job="jobs.find((job) => job.id === activeOperation!.jobId)"
        :busy="busy"
        error-in-dialog
        @refresh="management.sync(activeOperation!, true)"
        @more="management.results(activeOperation!, true)"
        @undo="management.undo(activeOperation!)"
        @retry="management.retry(activeOperation!)"
        @control="controlJob(activeOperation!.jobId, $event)"
      />
      <p v-if="notice" class="workflow-notice" role="status">{{ notice }}</p>
      <div
        v-if="
          recentAttempt &&
          ['partial', 'failed', 'cancelled', 'interrupted'].includes(
            recentAttempt.state,
          )
        "
        class="workflow-attempt"
      >
        <strong
          >上次{{ recentAttempt.label
          }}{{ stateName(recentAttempt.state) }}</strong
        ><span>保留上一次完整结果及已提交的检查记录。</span>
        <details v-if="recentAttempt.errors.length">
          <summary>查看任务提示</summary>
          <p v-for="message in recentAttempt.errors" :key="message">
            {{ message }}
          </p>
        </details>
      </div>
      <section
        v-if="advanced"
        class="workflow-advanced"
        aria-label="更多检索选项"
      >
        <h2>检索选项</h2>
        <div class="workflow-options">
          <label
            ><input v-model="force" type="checkbox" />强制重新计算指纹</label
          ><button class="soft-button" :disabled="busy" @click="start()">
            重新三轮检索
          </button>
        </div>
        <p>默认复用有效指纹；强制重算只在再次检索时生效。</p>
        <div v-if="scan" class="workflow-diagnostics">
          <span>查重缓存 {{ bytes(scan?.cacheBytes ?? 0) }}</span
          ><span
            >失败 {{ scan?.coverage.failed ?? 0 }} · 跳过近似检查
            {{ scan?.coverage.skippedNear ?? 0 }}</span
          ><button
            class="text-button danger"
            :disabled="busy"
            @click="confirmClear = true"
          >
            清理查重缓存
          </button>
        </div>
        <p v-else>缓存信息待读取。</p>
        <details class="workflow-history">
          <summary>
            整理历史 <span>{{ batches.operations.value.size }} 个批次</span>
          </summary>
          <SoftSelect
            v-if="operationOptions.length"
            v-model="batches.activeOperationId.value"
            aria-label="选择整理批次"
            :options="operationOptions"
          /><DuplicateOperationPanel
            v-if="batches.latest.value"
            :operation="batches.latest.value"
            :job="jobs.find((job) => job.id === batches.latest.value!.jobId)"
            :busy="busy"
            error-in-dialog
            @refresh="management.sync(batches.latest.value!, true)"
            @more="management.results(batches.latest.value!, true)"
            @undo="management.undo(batches.latest.value!)"
            @retry="management.retry(batches.latest.value!)"
            @control="controlJob(batches.latest.value!.jobId, $event)"
          />
          <p v-else>暂无整理记录。</p>
          <button
            class="text-button danger"
            :disabled="busy"
            @click="confirmRecords = true"
          >
            清除整理记录…
          </button>
        </details>
      </section>
    </template>
    <SoftDialog
      v-if="confirmClear"
      title="清理查重缓存？"
      @close="confirmClear = false"
      ><p>
        只清理可重建的查重索引、分组与本轮人工选择；图库、原图和整理撤销记录保留。下次需要重新检索。
      </p>
      <template #actions
        ><button class="text-button" @click="confirmClear = false">取消</button
        ><button class="soft-button danger" @click="clearCache">
          清理缓存
        </button></template
      ></SoftDialog
    >
    <SoftDialog
      v-if="confirmRecords"
      title="清除整理记录？"
      @close="confirmRecords = false"
      ><p>
        清除后无法再通过这些批次撤销整理。图库图片和当前回收站状态保持不变。
      </p>
      <template #actions
        ><button class="text-button" @click="confirmRecords = false">
          取消</button
        ><button class="soft-button danger" @click="clearRecords">
          清除记录
        </button></template
      ></SoftDialog
    >
    <ErrorDialog
      :errors="errors"
      title="重复检索操作未完成"
      :description="errorDescription"
      retry-label="刷新结果"
      :retrying="refreshingResults"
      :retry-disabled="activeWork"
      @retry="refreshResults"
    />
  </section>
</template>
<style scoped>
@import "./workflow.css";
</style>
