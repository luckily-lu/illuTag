<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import type { DuplicateReviewMember } from "../../shared/contracts/DuplicateReviewMember";
import PanImage from "./PanImage.vue";
import AppIcon from "../../shared/ui/AppIcon.vue";
const props = defineProps<{
  members: DuplicateReviewMember[];
  imageId: string;
}>();
const emit = defineEmits<{ close: []; change: [id: string] }>();
const root = ref<HTMLElement>();
const index = computed(() =>
  props.members.findIndex((m) => m.member.image.id === props.imageId),
);
const selected = computed(() => props.members[index.value]);
let previous: HTMLElement | null = null;
function step(delta: number) {
  const next = props.members[index.value + delta];
  if (next) emit("change", next.member.image.id);
}
function key(e: KeyboardEvent) {
  if (document.querySelector(".soft-dialog,.select-popover")) return;
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopImmediatePropagation();
    emit("close");
  } else if (["ArrowLeft", "ArrowRight"].includes(e.key)) {
    e.preventDefault();
    e.stopImmediatePropagation();
    step(e.key === "ArrowLeft" ? -1 : 1);
  } else if (e.key === "Tab") {
    const nodes = [
      ...(root.value?.querySelectorAll<HTMLElement>(
        'button:not(:disabled),[tabindex="0"]',
      ) ?? []),
    ].filter((n) => n.getClientRects().length);
    const outside = !root.value?.contains(document.activeElement);
    if (e.shiftKey && (outside || document.activeElement === nodes[0])) {
      e.preventDefault();
      nodes.at(-1)?.focus();
    } else if (
      !e.shiftKey &&
      (outside || document.activeElement === nodes.at(-1))
    ) {
      e.preventDefault();
      nodes[0]?.focus();
    }
  } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    e.stopImmediatePropagation();
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
  <Teleport to="body">
    <section
      ref="root"
      class="duplicate-review-viewer"
      role="dialog"
      aria-modal="true"
      aria-label="查看分组原图"
      @keydown.stop
      @keyup.stop
      @click.stop
    >
      <header>
        <button
          class="icon-button"
          aria-label="关闭原图"
          @click="emit('close')"
        >
          <AppIcon name="close" />
        </button>
        <strong>查看原图</strong
        ><span>{{ index + 1 }} / {{ members.length }}</span
        ><i />
        <button
          class="text-button"
          :disabled="index <= 0"
          aria-label="上一张图片"
          @click="step(-1)"
        >
          <AppIcon name="left" />上一张
        </button>
        <button
          class="text-button"
          :disabled="index >= members.length - 1"
          aria-label="下一张图片"
          @click="step(1)"
        >
          下一张<AppIcon name="right" />
        </button>
      </header>
      <PanImage v-if="selected" :member="selected.member" label="原图" />
    </section>
  </Teleport>
</template>
<style scoped>
.duplicate-review-viewer {
  position: fixed;
  inset: 0;
  z-index: 200;
  background: #151918;
  color: #f4f5f4;
  display: flex;
  flex-direction: column;
  overscroll-behavior: contain;
  --text: #f4f5f4;
  --muted: #b9c3be;
  --hover: #2b3230;
  --border: #39423e;
}
header {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 8px 14px;
  border-bottom: 1px solid #303834;
  min-height: 54px;
  flex-wrap: wrap;
}
header i {
  flex: 1;
}
header span {
  font-size: 12px;
  color: var(--muted);
}
header .text-button {
  color: inherit;
  display: inline-flex;
  align-items: center;
}
:deep(.duplicate-pan-panel) {
  flex: 1;
  min-height: 0;
  background: transparent;
  border: 0;
  padding: 12px 20px;
  display: flex;
  flex-direction: column;
}
:deep(.duplicate-pan-stage) {
  flex: 1;
  min-height: 0;
  background: transparent;
}
:deep(.duplicate-pan-stage img) {
  object-fit: contain;
}
:deep(.duplicate-pan-controls) {
  flex-shrink: 0;
  flex-wrap: wrap;
  color: inherit;
}
:deep(.duplicate-pan-path) {
  margin: 0;
  font-size: 11px;
  line-height: 1.5;
  overflow-wrap: anywhere;
  max-height: 3em;
  overflow: auto;
  color: var(--muted);
}
@media (max-width: 600px) {
  header {
    gap: 6px;
  }
  header strong {
    display: none;
  }
  :deep(.duplicate-pan-panel) {
    padding: 8px;
  }
}
</style>
