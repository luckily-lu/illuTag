<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
import AppIcon from "../../shared/ui/AppIcon.vue";
import PanImage from "./PanImage.vue";
defineProps<{
  reference: DuplicateMember;
  candidate: DuplicateMember;
  kind: string;
}>();
const emit = defineEmits<{ close: [] }>();
const root = ref<HTMLElement>();
let previous: HTMLElement | null = null;
function key(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopImmediatePropagation();
    emit("close");
  } else if (e.key === "Tab") {
    const nodes = Array.from(
      root.value?.querySelectorAll<HTMLElement>(
        'button:not(:disabled),[tabindex="0"]',
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
    </header>
    <p
      v-if="reference.stale || candidate.stale"
      class="duplicate-warning duplicate-comparison-notice"
    >
      文件已变化，本次结果可能失效。关闭比对后请更新结果。
    </p>
    <div class="duplicate-comparison-body">
      <PanImage :member="reference" label="参考图" /><PanImage
        :member="candidate"
        label="候选图"
      />
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
