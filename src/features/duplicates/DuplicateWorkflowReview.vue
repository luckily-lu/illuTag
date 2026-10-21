<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import AppIcon from "../../shared/ui/AppIcon.vue";
import { imageUrl } from "../../shared/ipc";
import { bytes } from "./format";
import type { useDuplicateWorkflow } from "./useDuplicateWorkflow";
import DuplicateReviewViewer from "./DuplicateReviewViewer.vue";
const props = defineProps<{
  workflow: ReturnType<typeof useDuplicateWorkflow>;
  busy: boolean;
  blockedReason?: string;
  notice?: string;
  undoable: boolean;
  undoBusy: boolean;
}>();
const emit = defineEmits<{ close: []; undo: [] }>();
const w = props.workflow;
const root = ref<HTMLElement>(),
  viewport = ref<HTMLElement>();
const viewerId = ref<string | null>(null);
const names: Record<string, string> = {
  recycle: "可回收图片",
  retained: "保留图片",
  suggested: "有损副本建议",
  exact: "完全相同图片",
  candidate: "待复检近似图片",
  near: "近似图片",
  similar: "类似图片",
  uncertain: "无法确认的图片",
};
const labels: Record<string, string> = {
  keep: "保留",
  clean: "待清理",
  protected: "人工保留",
  manual: "人工待清理",
  pending: "待复检",
};
const retained = computed(
  () =>
    w.current.value?.members.filter(
      (m) => !["clean", "manual"].includes(m.disposition),
    ).length ?? 0,
);
const cleanup = computed(
  () =>
    w.current.value?.members.filter((m) =>
      ["clean", "manual"].includes(m.disposition),
    ).length ?? 0,
);
const disabled = computed(
  () => props.busy || w.loading.value || w.editing.value,
);
const navigationDisabled = computed(() => w.loading.value || w.editing.value);
let previous: HTMLElement | null = null;
const positions = new Map<string, number>();
function saveScroll() {
  const id = w.current.value?.id;
  if (id && viewport.value) positions.set(id, viewport.value.scrollTop);
}
async function step(delta: number) {
  saveScroll();
  await w.step(delta);
  await nextTick();
  if (viewport.value)
    viewport.value.scrollTop = positions.get(w.current.value?.id ?? "") ?? 0;
  root.value
    ?.querySelector<HTMLElement>(".review-group-heading")
    ?.focus({ preventScroll: true });
}
async function confirm() {
  saveScroll();
  await w.confirmNext();
  await nextTick();
  if (viewport.value)
    viewport.value.scrollTop = positions.get(w.current.value?.id ?? "") ?? 0;
}
function key(e: KeyboardEvent) {
  if (viewerId.value || document.querySelector(".soft-dialog,.select-popover"))
    return;
  if (
    e.target instanceof Element &&
    e.target.closest('input,textarea,select,[contenteditable="true"]')
  )
    return;
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopImmediatePropagation();
    emit("close");
  } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
    e.preventDefault();
    e.stopImmediatePropagation();
    if (!navigationDisabled.value) void step(e.key === "ArrowLeft" ? -1 : 1);
  } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    e.stopImmediatePropagation();
    if (!disabled.value) void confirm();
  }
}
watch(
  () => w.current.value?.id,
  () => {
    viewerId.value = null;
  },
);
onMounted(() => {
  previous = document.activeElement as HTMLElement;
  root.value?.querySelector<HTMLElement>("button")?.focus();
  window.addEventListener("keydown", key, true);
});
onUnmounted(() => {
  window.removeEventListener("keydown", key, true);
  if (previous?.isConnected) previous.focus({ preventScroll: true });
});
</script>
<template>
  <section ref="root" class="workflow-review" aria-label="重复图片审查">
    <header class="review-topbar">
      <button class="text-button" @click="emit('close')">
        <AppIcon name="left" :size="17" />返回结果
      </button>
      <span class="review-divider" />
      <h1>{{ names[w.kind.value] || "查看图片" }}</h1>
      <small>人工调整可选，修改仅加入整理计划</small>
    </header>
    <div v-if="undoable" class="review-undo-note" role="status">
      <span>含已整理结果。需要调整保留项，可先撤销该批次。</span>
      <button class="text-button" :disabled="undoBusy" @click="emit('undo')">
        撤销本次整理并继续审查
      </button>
    </div>
    <div v-if="w.current.value" class="review-group-heading" tabindex="-1">
      <div>
        <strong>第 {{ w.position.value.toLocaleString() }} 组</strong
        ><span> / {{ w.totalGroups.value.toLocaleString() }} 组</span
        ><span class="review-group-size"
          >{{ w.current.value.totalMembers.toLocaleString() }} 张图片</span
        ><span v-if="w.current.value.confirmed" class="review-confirmed"
          ><AppIcon name="check" :size="14" />已确认</span
        >
      </div>
      <nav aria-label="切换分组">
        <button
          class="text-button"
          :disabled="navigationDisabled || !w.canPrevious.value"
          @click="step(-1)"
        >
          <AppIcon name="left" :size="16" />上一组</button
        ><button
          class="text-button"
          :disabled="navigationDisabled || !w.canNext.value"
          @click="step(1)"
        >
          下一组<AppIcon name="right" :size="16" />
        </button>
      </nav>
    </div>
    <p v-if="busy || notice" class="review-hint" role="status">
      {{
        busy
          ? blockedReason ||
            "正在执行图库变更，暂不可修改；仍可浏览分组与原图。"
          : notice
      }}
    </p>
    <p v-if="w.kind.value === 'candidate'" class="review-hint">
      这些图片尚未复检，可先设为保留；完成第二步后才可加入清理。
    </p>
    <p
      v-else-if="w.status.value?.policy === 'illuTag-three-pass-v1'"
      class="review-hint"
    >
      {{
        w.kind.value === "near"
          ? "近似版本不代表全部像素相同。可保留多张或更换保留基准，返回结果后再一键整理。"
          : w.kind.value === "similar"
            ? "这些图片存在内容或细节差异，默认保留，不会批量整理。"
            : w.kind.value === "uncertain"
              ? "暂未获得可靠结论，默认保留；无法确认不代表内容不同。"
              : "尺寸与全部像素一致，可按需更换保留基准或保留更多图片。"
      }}
    </p>
    <p
      v-else-if="
        ['similar', 'uncertain', 'suggested', 'retained'].includes(w.kind.value)
      "
      class="review-hint"
    >
      这些图片默认保留；未通过验证不等于已确认内容不同。有损建议未经真实样本校准，人工“待清理”仅代表你的决定。
    </p>
    <div
      ref="viewport"
      class="review-images-viewport"
      :aria-busy="w.loading.value"
    >
      <div
        v-if="w.current.value"
        class="review-images-grid"
        :class="{ 'review-many': w.current.value.totalMembers >= 4 }"
      >
        <article
          v-for="row in w.current.value.members"
          :key="row.member.image.id"
          class="review-image"
          :class="{
            'is-keeper': row.isKeeper,
            'is-clean': ['clean', 'manual'].includes(row.disposition),
            'is-protected': row.disposition === 'protected',
          }"
          :data-image-id="row.member.image.id"
        >
          <button
            class="review-image-open"
            :aria-label="'放大查看：' + row.member.image.fileName"
            @click="viewerId = row.member.image.id"
          >
            <img
              :src="
                imageUrl(
                  row.member.image.thumbnailPath || row.member.image.path,
                )
              "
              :alt="row.member.image.fileName"
              loading="lazy"
            />
            <span class="review-image-status"
              ><AppIcon v-if="row.isKeeper" name="check" :size="13" />{{
                row.member.image.trashed
                  ? "已移入回收站"
                  : row.isKeeper
                    ? "保留基准"
                    : labels[row.disposition] || row.disposition
              }}</span
            >
          </button>
          <div class="review-image-info">
            <strong :title="row.member.image.fileName">{{
              row.member.image.fileName
            }}</strong
            ><span
              >{{ row.member.image.width }} × {{ row.member.image.height }} ·
              {{ bytes(row.member.image.fileSize) }}</span
            ><small :title="row.member.image.id">{{
              row.member.image.id
            }}</small>
            <p v-if="row.reason" class="review-image-reason">
              {{ row.reason }}
            </p>
            <p v-if="row.member.stale" class="error-text">
              文件已变化，当前不会自动清理。
            </p>
          </div>
          <div class="review-image-actions">
            <button
              class="text-button"
              :disabled="
                disabled ||
                row.isKeeper ||
                row.member.stale ||
                row.member.image.trashed
              "
              @click="w.edit('keeper', row.member.image.id)"
            >
              {{ row.isKeeper ? "保留基准" : "设为保留基准" }}
            </button>
            <button
              class="text-button"
              :class="{ 'review-selected': row.disposition === 'protected' }"
              :disabled="
                disabled || row.member.stale || row.member.image.trashed
              "
              @click="w.edit('keep', row.member.image.id)"
            >
              保留
            </button>
            <button
              v-if="
                w.status.value?.policy !== 'illuTag-three-pass-v1' ||
                ['exact', 'near'].includes(w.kind.value)
              "
              class="text-button danger"
              :class="{
                'review-selected': ['clean', 'manual'].includes(
                  row.disposition,
                ),
              }"
              :disabled="
                disabled ||
                !w.canClean.value ||
                row.isKeeper ||
                row.member.stale ||
                row.member.image.trashed
              "
              @click="w.edit('clean', row.member.image.id)"
            >
              待清理
            </button>
          </div>
        </article>
      </div>
      <div v-else-if="!w.loading.value" class="review-empty">
        <AppIcon name="check" :size="24" /><strong
          >当前分类没有可查看的分组</strong
        ><span>可返回结果选择其他分类。</span>
      </div>
      <p v-if="w.loading.value" class="review-loading" role="status">
        正在读取图片分组…
      </p>
      <button
        v-if="w.current.value?.nextCursor"
        class="text-button review-more"
        :disabled="navigationDisabled"
        @click="w.moreMembers()"
      >
        继续显示本组图片（已显示 {{ w.current.value.members.length }} /
        {{ w.current.value.totalMembers }}）
      </button>
    </div>
    <footer v-if="w.current.value" class="review-footer">
      <span
        >本组已显示 <b>{{ retained }}</b> 张保留 ·
        <b>{{ cleanup }}</b> 张待清理<small v-if="w.current.value.nextCursor"
          >（其余成员按需展开）</small
        ></span
      ><span class="review-keyboard">← → 切组 · Ctrl+Enter 确认</span
      ><button
        class="soft-button primary"
        :disabled="disabled"
        @click="confirm()"
      >
        {{
          w.editing.value
            ? "保存中…"
            : w.canNext.value
              ? "确认本组，下一组"
              : "确认本组"
        }}
      </button>
    </footer>
    <DuplicateReviewViewer
      v-if="viewerId && w.current.value"
      :members="w.current.value.members"
      :image-id="viewerId"
      @close="viewerId = null"
      @change="viewerId = $event"
    />
  </section>
</template>
<style scoped>
.workflow-review {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  gap: 0;
}
.review-topbar {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 4px 0 22px;
  flex-wrap: wrap;
  flex-shrink: 0;
}
.review-topbar .text-button,
.review-group-heading .text-button {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.review-divider {
  height: 18px;
  border-left: 1px solid var(--border);
}
.review-topbar h1 {
  font-size: 20px;
  margin: 0;
}
.review-topbar small {
  color: var(--muted);
  margin-left: auto;
}
.review-undo-note {
  background: var(--hover);
  padding: 12px 14px;
  display: flex;
  align-items: center;
  gap: 12px;
  justify-content: space-between;
  margin-bottom: 12px;
  font-size: 12px;
}
.review-undo-note .text-button {
  font-weight: 600;
  text-decoration: underline;
  text-underline-offset: 3px;
}
.review-group-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 0;
  border-top: 1px solid var(--border);
  border-bottom: 1px solid var(--border);
  font-size: 13px;
  flex-shrink: 0;
}
.review-group-heading:focus {
  outline: none;
}
.review-group-heading span {
  color: var(--muted);
}
.review-group-size {
  margin-left: 18px;
}
.review-confirmed {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-left: 12px;
  color: #34734c !important;
}
.review-group-heading nav {
  display: flex;
  gap: 12px;
}
.review-hint {
  font-size: 12px;
  color: var(--muted);
  margin: 12px 0 0;
}
.review-images-viewport {
  min-height: 0;
  flex: 1;
  overflow: auto;
  padding: 20px 2px;
  scrollbar-gutter: stable;
}
.review-images-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 22px;
  align-items: start;
}
.review-many {
  grid-template-columns: repeat(auto-fit, minmax(min(240px, 100%), 1fr));
}
.review-image {
  border: 1px solid var(--border);
  border-radius: 6px;
  min-width: 0;
  overflow: hidden;
  background: var(--surface, #fff);
}
.review-image.is-keeper {
  border-color: #343b37;
}
.review-image.is-protected {
  border-color: #678a71;
}
.review-image-open {
  display: block;
  position: relative;
  width: 100%;
  height: clamp(210px, 35vh, 430px);
  padding: 10px;
  background: #f0f1ef;
  border: 0;
  cursor: zoom-in;
}
.review-many .review-image-open {
  height: clamp(170px, 26vh, 300px);
}
.review-image-open img {
  width: 100%;
  height: 100%;
  object-fit: contain;
}
.review-image-status {
  position: absolute;
  top: 10px;
  left: 10px;
  background: #fffef0ed;
  color: #3b453d;
  font-size: 11px;
  border-radius: 3px;
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 4px 7px;
}
.is-clean .review-image-status {
  background: #fff1ed;
  color: #9b3f32;
}
.is-keeper .review-image-status {
  background: #252c28;
  color: white;
}
.review-image-info {
  padding: 12px 12px 8px;
  display: flex;
  flex-direction: column;
  gap: 5px;
}
.review-image-info strong {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 13px;
}
.review-image-info span,
.review-image-info small {
  font-size: 11px;
  color: var(--muted);
}
.review-image-info small {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.review-image-reason {
  font-size: 11px;
  line-height: 1.5;
  color: var(--muted);
  margin: 2px 0;
}
.review-image-actions {
  border-top: 1px solid var(--border);
  display: flex;
  gap: 4px;
  align-items: center;
  flex-wrap: wrap;
  padding: 5px 7px;
}
.review-image-actions .text-button {
  font-size: 11px;
  padding: 6px;
}
.review-selected {
  text-decoration: underline;
  text-underline-offset: 4px;
}
.review-more {
  display: block;
  margin: 18px auto 0;
}
.review-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
  border-top: 1px solid var(--border);
  padding: 15px 0 0;
  font-size: 12px;
  flex-shrink: 0;
}
.review-footer small,
.review-keyboard {
  color: var(--muted);
  font-size: 11px;
}
.review-empty {
  min-height: 220px;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  color: var(--muted);
  font-size: 13px;
}
.review-loading {
  text-align: center;
  font-size: 12px;
  color: var(--muted);
  padding: 20px;
}
@media (min-width: 1500px) {
  .review-many {
    grid-template-columns: repeat(auto-fit, minmax(270px, 1fr));
  }
}
@media (max-width: 850px) {
  .review-keyboard,
  .review-topbar small {
    display: none;
  }
  .review-many {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .review-group-heading nav {
    gap: 2px;
  }
  .review-group-size {
    margin-left: 8px;
  }
  .review-footer {
    gap: 8px;
  }
  .review-images-grid {
    gap: 12px;
  }
  .review-undo-note {
    align-items: flex-start;
    flex-direction: column;
  }
}
@media (max-width: 580px) {
  .review-topbar {
    gap: 8px;
    padding-bottom: 14px;
  }
  .review-topbar h1 {
    font-size: 17px;
  }
  .review-group-heading {
    align-items: flex-start;
    flex-direction: column;
  }
  .review-images-grid,
  .review-many {
    grid-template-columns: minmax(0, 1fr);
  }
  .review-image-open,
  .review-many .review-image-open {
    height: 260px;
  }
  .review-footer > .soft-button {
    margin-left: auto;
  }
}
</style>
