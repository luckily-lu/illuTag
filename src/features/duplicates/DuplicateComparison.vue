<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
import AppIcon from "../../shared/ui/AppIcon.vue";
import PanImage from "./PanImage.vue";
const props = withDefaults(
  defineProps<{
    reference: DuplicateMember;
    candidate: DuplicateMember;
    kind: string;
    candidates?: DuplicateMember[];
    canMore?: boolean;
    loadingMore?: boolean;
  }>(),
  {
    candidates: () => [],
    canMore: false,
    loadingMore: false,
  },
);
const emit = defineEmits<{
  close: [];
  change: [member: DuplicateMember];
  more: [];
}>();
const index = computed(() =>
  props.candidates.findIndex(
    (member) => member.image.id === props.candidate.image.id,
  ),
);
function step(delta: number) {
  const next = index.value + delta;
  if (next >= 0 && next < props.candidates.length)
    emit("change", props.candidates[next]);
  else if (delta > 0 && props.canMore && !props.loadingMore) emit("more");
}
const root = ref<HTMLElement>();
let previous: HTMLElement | null = null;
function key(e: KeyboardEvent) {
  if (document.querySelector(".soft-dialog,.select-popover")) return;
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopImmediatePropagation();
    emit("close");
  } else if (
    (e.key === "ArrowLeft" || e.key === "ArrowRight") &&
    !(e.target as HTMLElement)?.closest(
      "input,textarea,select,[contenteditable='true']",
    )
  ) {
    e.preventDefault();
    step(e.key === "ArrowLeft" ? -1 : 1);
  } else if (e.key === "Tab") {
    const nodes = Array.from(
      root.value?.querySelectorAll<HTMLElement>(
        'button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],[tabindex="0"]',
      ) ?? [],
    ).filter((n) => n.getClientRects().length);
    const first = nodes[0],
      last = nodes.at(-1);
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last?.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first?.focus();
    }
  }
}
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
  <section
    ref="root"
    class="duplicate-comparison"
    role="dialog"
    aria-modal="true"
    aria-label="图片并排比对"
  >
    <header class="duplicate-comparison-toolbar">
      <button class="icon-button" aria-label="关闭比对" @click="emit('close')">
        <AppIcon name="close" />
      </button>
      <strong>图片比对</strong
      ><span class="duplicate-kind-label">{{
        kind === "exact" ? "文件完全相同" : "近似候选"
      }}</span
      ><span class="spacer" /><small>两张图片可独立缩放和平移</small>
      <button
        class="soft-button compact"
        :disabled="index <= 0"
        aria-label="上一张候选"
        @click="step(-1)"
      >
        上一张
      </button>
      <small v-if="candidates.length"
        >{{ Math.max(1, index + 1) }} / {{ candidates.length
        }}<template v-if="canMore"> 已加载</template></small
      >
      <button
        class="soft-button compact"
        :disabled="loadingMore || (index >= candidates.length - 1 && !canMore)"
        aria-label="下一张候选"
        @click="step(1)"
      >
        {{ loadingMore ? "读取中…" : "下一张" }}
      </button>
    </header>
    <p
      v-if="reference.stale || candidate.stale"
      class="duplicate-warning duplicate-comparison-notice"
    >
      文件已变化，本次结果可能失效。关闭比对后请更新结果。
    </p>
    <div class="duplicate-comparison-body">
      <PanImage :member="reference" label="参考图" />
      <PanImage :member="candidate" label="候选图" />
    </div>
    <footer v-if="kind === 'near'" class="duplicate-comparison-footer">
      候选筛选指标：pHash 距离 {{ candidate.metric.phashDistance ?? "—" }} ·
      dHash 距离 {{ candidate.metric.dhashDistance ?? "—" }} · 色彩差
      {{
        candidate.metric.colorDistance === null
          ? "—"
          : candidate.metric.colorDistance.toFixed(3)
      }}。这些指标不代表已经确认同图。
    </footer>
  </section>
</template>
