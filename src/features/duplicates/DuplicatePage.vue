<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type { DuplicateGroup } from "../../shared/contracts/DuplicateGroup";
import type { ImageDetail } from "../../shared/contracts/ImageDetail";
import type { UserFolder } from "../../shared/contracts/UserFolder";
import type { LibraryFolder } from "../../shared/contracts/LibraryFolder";
import AppIcon from "../../shared/ui/AppIcon.vue";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
import SoftSelect from "../../shared/ui/SoftSelect.vue";
import DuplicateImage from "./DuplicateImage.vue";
import DuplicateComparison from "./DuplicateComparison.vue";
import DuplicateReviewGrid from "./DuplicateReviewGrid.vue";
import DuplicateActionDialog from "./DuplicateActionDialog.vue";
import DuplicateOperationPanel from "./DuplicateOperationPanel.vue";
import type { DuplicateReview } from "./useDuplicateReview";
import { useDuplicateOperations } from "./useDuplicateOperations";
import {
  bytes,
  checkedDate,
  duration,
  sensitivityNames,
  stateNames,
} from "./format";
import type { useDuplicates } from "./useDuplicates";
import { api, errorMessage } from "../../shared/ipc";
const props = defineProps<{
  jobs: JobRecord[];
  refreshVersion: number;
  taskError: string;
  startJob: (options: DuplicateOptions) => Promise<boolean>;
  workspace: ReturnType<typeof useDuplicates>;
  review: DuplicateReview;
  folders: UserFolder[];
  sources: LibraryFolder[];
  controlJob: (id: string, action: string) => Promise<void>;
}>();
const emit = defineEmits<{ tasks: []; jobCreated: [job: JobRecord] }>();
const d = props.workspace,
  review = props.review;
const section = ref<HTMLElement>(),
  confirmClear = ref(false),
  confirmRecords = ref(false),
  starting = ref(false);
const comparison = ref<{
  reference: DuplicateMember;
  candidate: DuplicateMember;
  kind: string;
  groupId: string;
} | null>(null);
const auditGroupId = ref<string | null>(null);
const auditGroup = computed(() =>
  d.groups.value.find((group) => group.id === auditGroupId.value),
);
const auditMembers = computed(() =>
  auditGroup.value
    ? [
        ...new Map(
          [auditGroup.value.reference, ...auditGroup.value.members]
            .filter((member) => !review.processed.value.has(member.image.id))
            .map((member) => [member.image.id, member]),
        ).values(),
      ]
    : [],
);
const details = ref<Record<string, ImageDetail>>({}),
  detailLoading = ref(new Set<string>()),
  detailErrors = ref<Record<string, string>>({});
const management = useDuplicateOperations(d, review, {
  jobs: () => props.jobs,
  created: (job) => emit("jobCreated", job),
  advance: nextGroup,
});
const activeJob = computed(() =>
  props.jobs.find(
    (j) =>
      j.request.kind === "duplicates" &&
      ["queued", "running", "paused", "cancelling"].includes(j.state),
  ),
);
const busy = computed(
  () => starting.value || !!activeJob.value || management.busy.value,
);
const comparisonGroup = computed(() =>
  d.groups.value.find((group) => group.id === comparison.value?.groupId),
);
const comparisonCandidates = computed(
  () =>
    comparisonGroup.value?.members.filter(
      (member) =>
        member.image.id !== comparison.value?.reference.image.id &&
        !review.processed.value.has(member.image.id),
    ) ?? [],
);
const sampleMode = !isTauri();
watch(
  () => props.refreshVersion,
  async () => {
    await d.refresh();
    await management.syncAll();
  },
);
watch(
  () => review.version.value,
  () => void management.syncAll(),
);
watch(
  () => review.activeOperationId.value,
  () => void management.syncAll(),
);
watch(
  () => d.groups.value,
  (groups) => {
    review.reconcileActive(
      groups.flatMap((group) => [group.reference, ...group.members]),
    );
    if (
      auditGroupId.value &&
      !groups.some((group) => group.id === auditGroupId.value)
    )
      auditGroupId.value = null;
    if (comparison.value) {
      const group = groups.find(
        (value) => value.id === comparison.value!.groupId,
      );
      const candidate = group?.members.find(
        (member) => member.image.id === comparison.value!.candidate.image.id,
      );
      if (
        !group ||
        group.reference.image.id !== comparison.value.reference.image.id ||
        !candidate
      )
        comparison.value = null;
      else {
        comparison.value.reference = group.reference;
        comparison.value.candidate = candidate;
      }
    }
  },
);
function pageKey(event: KeyboardEvent) {
  if (
    event.defaultPrevented ||
    event.key !== "Escape" ||
    comparison.value ||
    auditGroupId.value ||
    confirmClear.value ||
    management.confirmation.value ||
    document.querySelector(".select-popover")
  )
    return;
  const id = review.activeGroupId.value;
  if (id && review.selection(id).size) {
    review.clearSelection(id);
    event.preventDefault();
    event.stopImmediatePropagation();
  }
}
onMounted(async () => {
  await d.refresh();
  await management.syncAll();
  await nextTick();
  section.value?.scrollTo({ top: review.scrollTop.value });
  window.addEventListener("keydown", pageKey);
});
onUnmounted(() => {
  review.scrollTop.value = section.value?.scrollTop ?? 0;
  window.removeEventListener("keydown", pageKey);
});
async function start(regroupOnly = false) {
  if (busy.value) return;
  starting.value = true;
  try {
    if (await props.startJob(d.options(regroupOnly))) await d.refresh();
  } finally {
    starting.value = false;
  }
}
async function filter(kind: string) {
  section.value?.scrollTo({ top: 0 });
  await d.setKind(kind);
}
async function clear() {
  confirmClear.value = false;
  await d.clear();
}
async function clearRecords() {
  confirmRecords.value = false;
  try {
    await api.clearDuplicateManagementRecords();
    review.operations.value = new Map();
    review.activeOperationId.value = null;
    management.error.value = "";
    management.notice.value = "处理记录已清理，已回收图片的状态不变。";
  } catch (error) {
    management.error.value = errorMessage(error);
  }
}
function compare(
  reference: DuplicateMember,
  candidate: DuplicateMember,
  kind: string,
  groupId: string,
) {
  review.activeGroupId.value = groupId;
  if (kind === "near") {
    void openAudit(groupId);
    return;
  }
  comparison.value = { reference, candidate, kind, groupId };
}
async function openAudit(groupId: string) {
  review.activeGroupId.value = groupId;
  auditGroupId.value = groupId;
  const group = d.groups.value.find((value) => value.id === groupId);
  if (group?.nextCursor) await d.moreMembers(groupId);
}
async function manageAudit() {
  const group = auditGroup.value;
  if (!group) return;
  if (
    !auditMembers.value.some((member) => review.kept.value.has(member.image.id))
  ) {
    management.error.value = "先按 K 或点击保留，明确留下至少一张图片。";
    return;
  }
  await management.prepare(group, "selected", false);
}
function keptCount(group: DuplicateGroup) {
  return [
    ...new Set(
      [group.reference, ...group.members]
        .filter((member) => review.kept.value.has(member.image.id))
        .map((member) => member.image.id),
    ),
  ].length;
}
function select(group: DuplicateGroup, member: DuplicateMember) {
  review.activeGroupId.value = group.id;
  review.toggle(group.id, member);
}
async function loadDetail(member: DuplicateMember) {
  const id = member.image.id;
  if (detailLoading.value.has(id) || details.value[id]) return;
  detailLoading.value = new Set([...detailLoading.value, id]);
  try {
    details.value = { ...details.value, [id]: await api.detail(id) };
  } catch (error) {
    detailErrors.value = { ...detailErrors.value, [id]: errorMessage(error) };
  } finally {
    detailLoading.value = new Set(
      [...detailLoading.value].filter((value) => value !== id),
    );
  }
}
function folderLabels(id: string) {
  return (
    details.value[id]?.folderIds.map(
      (folderId) =>
        props.folders.find((folder) => folder.id === folderId)?.name ??
        `分类 ${folderId}`,
    ) ?? []
  );
}
async function nextGroup(groupId: string, followingIds: string[] = []) {
  const reviewing = auditGroupId.value !== null;
  comparison.value = null;
  auditGroupId.value = null;
  const index = d.groups.value.findIndex((group) => group.id === groupId);
  const ordered = followingIds.length
    ? followingIds
    : d.groups.value.slice(index + 1).map((group) => group.id);
  let next = ordered
    .map((id) => d.groups.value.find((group) => group.id === id))
    .find((group) => group && !review.decision(group.id));
  if (!next && d.nextCursor.value) {
    const known = new Set(d.groups.value.map((group) => group.id));
    await d.loadGroups(true);
    next = d.groups.value.find(
      (group) => !known.has(group.id) && !review.decision(group.id),
    );
  }
  if (!next) {
    management.notice.value =
      "当前加载的分组已查看完，可以加载更多、切换筛选或更新结果。";
    return;
  }
  review.activeGroupId.value = next.id;
  if (reviewing && next.kind === "near") {
    await openAudit(next.id);
    return;
  }
  await nextTick();
  const element = [
    ...(section.value?.querySelectorAll<HTMLElement>(
      "[data-duplicate-group-id]",
    ) ?? []),
  ].find((node) => node.dataset.duplicateGroupId === next!.id);
  element?.scrollIntoView({ block: "start" });
  element?.focus({ preventScroll: true });
}
async function skip(group: DuplicateGroup) {
  review.decide(group.id, "skipped");
  await nextGroup(group.id);
}
async function moreComparison() {
  const groupId = comparison.value?.groupId;
  if (!groupId) return;
  const count = comparisonCandidates.value.length;
  await d.moreMembers(groupId);
  const next = comparisonCandidates.value[count];
  if (comparison.value && next) comparison.value.candidate = next;
}
</script>
<template>
  <section ref="section" class="page duplicate-page">
    <header class="page-header">
      <div>
        <p class="page-kicker">图库工具</p>
        <h1>重复检索</h1>
        <p>找出副本与近似画面，选择保留后逐组处理。</p>
      </div>
      <button
        class="soft-button primary"
        :disabled="busy || d.loading.value"
        @click="start()"
      >
        <AppIcon name="duplicates" :size="18" />{{
          busy
            ? "任务处理中"
            : d.status.value?.generation
              ? "更新结果"
              : "开始查重"
        }}
      </button>
    </header>
    <p v-if="sampleMode" class="duplicate-preview-note">
      布局预览 · 当前是虚构示例，不会检索或修改真实图库。
    </p>
    <div class="duplicate-controls">
      <button
        class="soft-button"
        :disabled="
          busy || !d.status.value?.generation || !d.status.value.exactGroups
        "
        @click="management.autoExact()"
      >
        一键整理完全相同图片
      </button>
      <div class="duplicate-type-filter" role="group" aria-label="结果类型">
        <button
          v-for="[key, label] in [
            ['all', '全部'],
            ['exact', '文件完全相同'],
            ['near', '近似候选'],
          ]"
          :key="key"
          class="soft-button compact"
          :aria-pressed="d.kind.value === key"
          @click="filter(key)"
        >
          {{ label }}
        </button>
      </div>
      <label class="duplicate-sensitivity"
        ><span>近似筛选</span
        ><SoftSelect
          :model-value="d.sensitivity.value"
          :options="[
            { value: 'strict', label: '严格' },
            { value: 'standard', label: '标准' },
            { value: 'loose', label: '宽松' },
          ]"
          aria-label="近似筛选严格度"
          @update:model-value="d.sensitivity.value = String($event)"
      /></label>
      <button
        class="soft-button"
        :disabled="busy || !d.status.value?.generation || d.loading.value"
        @click="start(true)"
      >
        重新检索
      </button>
      <label class="duplicate-force"
        ><input
          v-model="d.forceRecompute.value"
          type="checkbox"
          :disabled="busy"
        />强制重新计算</label
      >
    </div>
    <p
      v-if="d.pendingSensitivity.value"
      class="duplicate-state-note"
      role="status"
    >
      已选{{ sensitivityNames[d.sensitivity.value] }}，当前结果仍使用{{
        sensitivityNames[d.status.value?.sensitivity || "standard"]
      }}。点击“重新检索”后生效。
    </p>
    <p v-if="d.forceRecompute.value" class="duplicate-state-note">
      强制重新计算将重新读取原图，用于检查大小和修改时间未变的文件。任务不会删除或合并图片。
    </p>
    <div
      v-if="d.status.value"
      class="duplicate-summary"
      aria-label="检索覆盖统计"
    >
      <span
        ><strong>{{
          d.status.value.coverage.processed.toLocaleString()
        }}</strong>
        / {{ d.status.value.coverage.total.toLocaleString() }} 张已检查</span
      >
      <span>失败 {{ d.status.value.coverage.failed.toLocaleString() }}</span>
      <span
        >跳过近似
        {{ d.status.value.coverage.skippedNear.toLocaleString() }}</span
      >
      <span
        >低信息
        {{ d.status.value.coverage.lowInformation.toLocaleString() }}</span
      >
      <span>过密 {{ d.status.value.coverage.tooDense.toLocaleString() }}</span>
      <span>缓存 {{ bytes(d.status.value.cacheBytes) }}</span>
      <span>{{ checkedDate(d.status.value.checkedAt) }}</span>
      <span v-if="d.status.value.durationMs !== null"
        >上次用时 {{ duration(d.status.value.durationMs) }}</span
      >
      <button
        class="text-button compact"
        :disabled="busy || d.loading.value || !d.status.value.cacheBytes"
        @click="confirmClear = true"
      >
        清理查重缓存
      </button>
      <button
        class="text-button compact"
        :disabled="busy"
        @click="confirmRecords = true"
      >
        清理处理记录
      </button>
    </div>
    <div v-if="activeJob" class="duplicate-state-note" role="status">
      <div class="button-row">
        <span
          >{{ stateNames[activeJob.state] || activeJob.state }} ·
          {{ activeJob.stage }} · {{ activeJob.processed.toLocaleString()
          }}<template v-if="activeJob.total">
            / {{ activeJob.total.toLocaleString() }}</template
          ></span
        ><button class="text-button" @click="emit('tasks')">到任务中心</button>
      </div>
      <progress
        v-if="activeJob.total"
        :value="activeJob.processed"
        :max="activeJob.total"
      />
      <p>切换页面不会停止任务。新结果完成前，保留上一次结果。</p>
    </div>
    <p
      v-if="d.status.value?.needsUpdate"
      class="duplicate-warning"
      role="status"
    >
      图库或源文件已变化，结果需要更新。点击“更新结果”检查缺失或失效项；不会自动开始任务。
    </p>
    <p
      v-if="
        d.status.value &&
        ['invalid', 'partial', 'failed', 'error'].includes(d.status.value.state)
      "
      class="duplicate-warning"
      role="status"
    >
      {{ stateNames[d.status.value.state] }}：当前结果不能代表全库检查完成。
    </p>
    <details
      v-if="d.status.value?.lastAttempt && !activeJob"
      class="duplicate-last-attempt"
    >
      <summary>
        最近一次任务：{{
          stateNames[d.status.value.lastAttempt.state] ||
          d.status.value.lastAttempt.state
        }}
        · {{ checkedDate(d.status.value.lastAttempt.updatedAt) }}
      </summary>
      <p>
        {{ d.status.value.lastAttempt.coverage.processed.toLocaleString() }} /
        {{ d.status.value.lastAttempt.coverage.total.toLocaleString() }}
        张已处理 ·
        {{ d.status.value.lastAttempt.coverage.failed.toLocaleString() }} 项失败
        ·
        {{ d.status.value.lastAttempt.coverage.skippedNear.toLocaleString() }}
        项跳过近似
      </p>
      <p
        v-for="(message, index) in d.status.value.lastAttempt.errors"
        :key="index"
        class="error-text"
      >
        {{ message }}
      </p>
    </details>
    <p v-if="d.error.value || taskError" class="error-message" role="alert">
      {{ d.error.value || taskError }}
      <button v-if="d.error.value" class="text-button" @click="d.refresh()">
        重试读取结果
      </button>
    </p>
    <details
      v-if="d.status.value?.errors.length"
      class="duplicate-last-attempt"
    >
      <summary>查看检查提示（{{ d.status.value.errors.length }}）</summary>
      <p
        v-for="(message, index) in d.status.value.errors"
        :key="index"
        class="error-text"
      >
        {{ message }}
      </p>
    </details>
    <p class="duplicate-explanation">
      近似组以参考图为中心，成员不一定彼此相似，部分图片可能出现在多组中。保留标记保护所有分组中的对应图片；近似候选只处理你明确选中的项。
    </p>
    <div class="duplicate-review-summary" aria-label="会话审核进度">
      <strong>本次会话</strong
      ><span>已处理 {{ review.reviewedCount.value }} 组</span
      ><span>暂不处理 {{ review.skippedCount.value }} 组</span
      ><span>保护 {{ review.kept.value.size }} 张</span
      ><span class="muted">审核草稿切换页面后保留</span>
    </div>
    <p v-if="management.error.value" class="error-message" role="alert">
      {{ management.error.value }}
    </p>
    <p v-else-if="management.notice.value" class="inline-notice" role="status">
      {{ management.notice.value }}
    </p>
    <label
      v-if="review.operations.value.size > 1"
      class="duplicate-operation-history"
      >查看操作<SoftSelect
        :model-value="review.latest.value?.preview.operationId ?? null"
        :options="
          [...review.operations.value.values()].reverse().map((operation) => ({
            value: operation.preview.operationId,
            label: `${operation.preview.action === 'restore' ? '撤销' : operation.preview.action === 'systemTrash' ? '原文件清理' : '软回收'} · ${operation.status?.total ?? operation.preview.targetCount} 项 · ${stateNames[operation.status?.state ?? operation.jobState] || operation.jobState}`,
          }))
        "
        aria-label="选择查重处理记录"
        @update:model-value="review.activeOperationId.value = String($event)"
    /></label>
    <DuplicateOperationPanel
      v-if="review.latest.value"
      :operation="review.latest.value"
      :job="jobs.find((job) => job.id === review.latest.value?.jobId)"
      :busy="busy"
      @refresh="management.sync(review.latest.value!, true)"
      @more="management.results(review.latest.value!, true)"
      @undo="management.undo(review.latest.value!)"
      @system="management.system(review.latest.value!)"
      @retry="management.retry(review.latest.value!)"
      @control="controlJob(review.latest.value!.jobId, $event)"
    />
    <div v-if="d.loading.value" class="empty-state" role="status">
      <p>正在读取分组…</p>
    </div>
    <div v-else-if="!d.status.value?.generation" class="empty-state">
      <AppIcon name="duplicates" :size="36" />
      <h2>{{ busy ? "正在建立内容索引" : "开始一次全库检查" }}</h2>
      <p>
        文件完全相同通过 SHA-256
        检查；近似候选使用画面指纹。首次任务需要读取原图，大图库可能耗时较长。
      </p>
      <p>
        回收站图片不参与检索。动态
        GIF／WebP、超大图、低信息图或解码失败的图片只进行文件相同检查，并显示跳过数量。
      </p>
    </div>
    <div v-else-if="!d.groups.value.length" class="empty-state">
      <h2>当前筛选没有结果</h2>
      <p
        v-if="d.status.value.needsUpdate || d.status.value.state === 'partial'"
      >
        结果需要更新或仅部分完成，不能据此确认图库没有重复图片。
      </p>
      <p v-else>
        本次检查没有发现符合{{
          d.kind.value === "exact"
            ? "文件完全相同"
            : d.kind.value === "near"
              ? "近似候选"
              : "当前"
        }}条件的分组。
      </p>
    </div>
    <div v-else class="duplicate-group-list">
      <p class="duplicate-result-count">
        {{ d.totalGroups.value.toLocaleString() }} 组 ·
        {{ d.status.value.exactGroups.toLocaleString() }} 组文件完全相同 ·
        {{ d.status.value.nearGroups.toLocaleString() }} 组近似候选
      </p>
      <article
        v-for="group in d.groups.value"
        :key="group.id"
        class="duplicate-group"
        :class="{
          'duplicate-group-active': review.activeGroupId.value === group.id,
        }"
        :data-duplicate-group-id="group.id"
        tabindex="-1"
      >
        <header>
          <strong>{{
            group.kind === "exact" ? "文件完全相同" : "近似候选"
          }}</strong
          ><small>共 {{ group.totalMembers.toLocaleString() }} 张</small>
          <span
            v-if="review.decision(group.id)"
            class="duplicate-group-decision"
            >{{
              review.decision(group.id) === "handled"
                ? "本次已处理"
                : "暂不处理"
            }}</span
          >
          <span class="spacer" />
          <span
            >已保护 {{ keptCount(group) }} 张 · 待处理
            {{ review.selection(group.id).size }} 张</span
          >
        </header>
        <p v-if="group.needsReview" class="duplicate-warning">
          原参考图已移出。本组幸存图片与新参考图不一定直接匹配，请逐张重新目视审核；未知距离显示为“—”。
        </p>
        <div class="duplicate-group-toolbar">
          <button
            v-if="group.kind === 'exact'"
            class="soft-button compact"
            :disabled="busy"
            @click="management.autoExact(group.id)"
          >
            一键保留一张
          </button>
          <button
            v-else
            class="soft-button compact primary"
            :disabled="busy"
            @click="openAudit(group.id)"
          >
            多图键盘审核
          </button>
          <button
            v-if="group.kind === 'exact'"
            class="soft-button compact primary"
            :disabled="busy || keptCount(group) === 0"
            :aria-label="
              '保留所选并处理整组其他图片：' + group.reference.image.fileName
            "
            @click="management.prepare(group, 'exactOthers')"
          >
            保留这些，处理整组其他
          </button>
          <button
            class="soft-button compact"
            :class="group.kind === 'near' ? 'primary' : ''"
            :disabled="busy || review.selection(group.id).size === 0"
            :aria-label="'处理选中项：' + group.reference.image.fileName"
            @click="management.prepare(group, 'selected')"
          >
            处理选中项
          </button>
          <button
            class="text-button compact"
            :disabled="busy || !review.selection(group.id).size"
            @click="review.clearSelection(group.id)"
          >
            清空本组选择
          </button>
          <button
            class="text-button compact"
            :disabled="busy"
            @click="skip(group)"
          >
            暂不处理，下一组
          </button>
          <span v-if="group.kind === 'exact'" class="muted"
            >整组操作包含尚未展开的图片</span
          >
          <span v-else class="muted">先比对，再勾选明确不需要的候选</span>
        </div>
        <div class="duplicate-group-images">
          <DuplicateImage
            :member="group.reference"
            reference
            :kept="review.kept.value.has(group.reference.image.id)"
            :selected="review.selection(group.id).has(group.reference.image.id)"
            :processed="review.processed.value.has(group.reference.image.id)"
            :disabled="busy"
            :detail="details[group.reference.image.id]"
            :detail-loading="detailLoading.has(group.reference.image.id)"
            :detail-error="detailErrors[group.reference.image.id]"
            :folder-labels="folderLabels(group.reference.image.id)"
            @keep="review.protect(group.reference)"
            @select="select(group, group.reference)"
            @details="loadDetail(group.reference)"
            @compare="
              compare(
                group.reference,
                group.members.find(
                  (m) => m.image.id !== group.reference.image.id,
                ) ?? group.reference,
                group.kind,
                group.id,
              )
            "
          />
          <DuplicateImage
            v-for="member in group.members.filter(
              (m) => m.image.id !== group.reference.image.id,
            )"
            :key="member.image.id"
            :member="member"
            :kept="review.kept.value.has(member.image.id)"
            :selected="review.selection(group.id).has(member.image.id)"
            :processed="review.processed.value.has(member.image.id)"
            :disabled="busy"
            :detail="details[member.image.id]"
            :detail-loading="detailLoading.has(member.image.id)"
            :detail-error="detailErrors[member.image.id]"
            :folder-labels="folderLabels(member.image.id)"
            @keep="review.protect(member)"
            @select="select(group, member)"
            @details="loadDetail(member)"
            @compare="compare(group.reference, member, group.kind, group.id)"
          />
        </div>
        <button
          v-if="group.nextCursor"
          class="soft-button compact"
          :disabled="d.memberLoading.value.has(group.id)"
          @click="d.moreMembers(group.id)"
        >
          {{
            d.memberLoading.value.has(group.id)
              ? "读取中…"
              : "展开更多图片（每次 50 张）"
          }}
        </button>
      </article>
      <button
        v-if="d.nextCursor.value"
        class="soft-button duplicate-more"
        :disabled="d.pageLoading.value"
        @click="d.loadGroups(true)"
      >
        {{ d.pageLoading.value ? "读取中…" : "加载更多分组（每次 20 组）" }}
      </button>
    </div>
    <Teleport to="body">
      <SoftDialog
        v-if="confirmRecords"
        title="清理处理记录"
        @close="confirmRecords = false"
        ><p>
          仅清理查重管理的操作记录，并放弃这些批次的撤销能力。原图、图库回收状态和指纹缓存不变；未修复的系统文件移动记录不能清理。
        </p>
        <template #actions
          ><button class="text-button" @click="confirmRecords = false">
            取消</button
          ><button class="soft-button primary" @click="clearRecords">
            清理记录
          </button></template
        ></SoftDialog
      >
      <DuplicateReviewGrid
        v-if="auditGroup"
        :members="auditMembers"
        :total="auditGroup.totalMembers"
        :kept="review.kept.value"
        :selected="review.selection(auditGroup.id)"
        :disabled="busy"
        :can-more="!!auditGroup.nextCursor"
        :loading-more="d.memberLoading.value.has(auditGroup.id)"
        :error="management.error.value"
        @keep="review.protect($event)"
        @select="select(auditGroup, $event)"
        @manage="manageAudit()"
        @more="d.moreMembers(auditGroup.id)"
        @next="skip(auditGroup)"
        @close="auditGroupId = null"
      />
      <DuplicateComparison
        v-if="comparison"
        v-bind="comparison"
        :candidates="comparisonCandidates"
        :reference-kept="review.kept.value.has(comparison.reference.image.id)"
        :candidate-kept="review.kept.value.has(comparison.candidate.image.id)"
        :reference-selected="
          review
            .selection(comparison.groupId)
            .has(comparison.reference.image.id)
        "
        :candidate-selected="
          review
            .selection(comparison.groupId)
            .has(comparison.candidate.image.id)
        "
        :reference-processed="
          review.processed.value.has(comparison.reference.image.id)
        "
        :candidate-processed="
          review.processed.value.has(comparison.candidate.image.id)
        "
        :disabled="busy"
        :can-more="!!comparisonGroup?.nextCursor"
        :loading-more="d.memberLoading.value.has(comparison.groupId)"
        @change="comparison.candidate = $event"
        @keep="review.protect($event)"
        @select="comparisonGroup && select(comparisonGroup, $event)"
        @more="moreComparison"
        @manage="
          comparisonGroup && management.prepare(comparisonGroup, 'selected')
        "
        @close="comparison = null"
      />
      <DuplicateActionDialog
        v-if="management.confirmation.value"
        :preview="management.confirmation.value.preview"
        :mode="management.confirmation.value.request.mode"
        :advance="management.confirmation.value.advance"
        :busy="management.submitting.value"
        :error="management.error.value"
        @close="management.confirmation.value = null"
        @confirm="management.apply"
        @retry="management.retryConfirmation"
      />
      <SoftDialog
        v-if="confirmClear"
        title="清理查重缓存"
        @close="confirmClear = false"
        ><p>
          仅清理内容指纹和查重分组。图片、标签、分类、收藏及主数据库保持完整。下次查重需要重新读取原图。
        </p>
        <template #actions
          ><button class="text-button" @click="confirmClear = false">
            取消</button
          ><button class="soft-button primary" @click="clear">
            清理缓存
          </button></template
        ></SoftDialog
      >
    </Teleport>
  </section>
</template>
