<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import AppIcon from "../../shared/ui/AppIcon.vue";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
import SoftSelect from "../../shared/ui/SoftSelect.vue";
import DuplicateImage from "./DuplicateImage.vue";
import DuplicateComparison from "./DuplicateComparison.vue";
import {
  bytes,
  checkedDate,
  duration,
  sensitivityNames,
  stateNames,
} from "./format";
import { useDuplicates } from "./useDuplicates";
const props = defineProps<{
  jobs: JobRecord[];
  refreshVersion: number;
  taskError: string;
  startJob: (options: DuplicateOptions) => Promise<boolean>;
}>();
const emit = defineEmits<{ tasks: [] }>();
const d = useDuplicates();
const section = ref<HTMLElement>(),
  confirmClear = ref(false),
  starting = ref(false);
const comparison = ref<{
  reference: DuplicateMember;
  candidate: DuplicateMember;
  kind: string;
} | null>(null);
const activeJob = computed(() =>
  props.jobs.find(
    (j) =>
      j.request.kind === "duplicates" &&
      ["queued", "running", "paused", "cancelling"].includes(j.state),
  ),
);
const busy = computed(() => starting.value || !!activeJob.value);
const sampleMode = !isTauri();
watch(
  () => props.refreshVersion,
  () => void d.refresh(),
);
onMounted(() => void d.refresh());
onUnmounted(d.dispose);
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
function compare(
  reference: DuplicateMember,
  candidate: DuplicateMember,
  kind: string,
) {
  comparison.value = { reference, candidate, kind };
}
</script>
<template>
  <section ref="section" class="page duplicate-page">
    <header class="page-header">
      <div>
        <p class="page-kicker">图库工具</p>
        <h1>重复检索</h1>
        <p>找到文件副本与近似画面，再并排查看原图。</p>
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
      近似组以参考图为中心，成员不一定彼此相似，部分图片可能出现在多组中。近似候选需要人工比对；本页不会删除、合并或折叠图库。
    </p>
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
      >
        <header>
          <strong>{{
            group.kind === "exact" ? "文件完全相同" : "近似候选"
          }}</strong
          ><small>共 {{ group.totalMembers.toLocaleString() }} 张</small>
        </header>
        <div class="duplicate-group-images">
          <DuplicateImage
            :member="group.reference"
            reference
            @compare="
              compare(
                group.reference,
                group.members.find(
                  (m) => m.image.id !== group.reference.image.id,
                ) ?? group.reference,
                group.kind,
              )
            "
          />
          <DuplicateImage
            v-for="member in group.members.filter(
              (m) => m.image.id !== group.reference.image.id,
            )"
            :key="member.image.id"
            :member="member"
            @compare="compare(group.reference, member, group.kind)"
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
      <DuplicateComparison
        v-if="comparison"
        v-bind="comparison"
        @close="comparison = null"
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
