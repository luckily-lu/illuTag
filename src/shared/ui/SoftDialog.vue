<script setup lang="ts">
import AppIcon from "./AppIcon.vue";
import { nextTick, onMounted, onUnmounted, ref } from "vue";
defineProps<{ title: string }>();
const emit = defineEmits<{ close: [] }>();
const panel = ref<HTMLElement>();
let previous: HTMLElement | null = null;
function key(event: KeyboardEvent) {
  if (document.querySelector(".select-popover")) return;
  if (event.key === "Escape") {
    event.stopImmediatePropagation();
    emit("close");
  }
  if (event.key === "Tab") {
    const nodes = Array.from(
      panel.value?.querySelectorAll<HTMLElement>(
        'button:not(:disabled),input,select,textarea,[tabindex="0"]',
      ) ?? [],
    );
    const first = nodes[0],
      last = nodes.at(-1);
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  }
}
onMounted(async () => {
  previous = document.activeElement as HTMLElement;
  await nextTick();
  panel.value?.querySelector<HTMLElement>("input,button")?.focus();
  window.addEventListener("keydown", key, true);
});
onUnmounted(() => {
  window.removeEventListener("keydown", key, true);
  previous?.focus();
});
</script>
<template>
  <div class="dialog-backdrop" @click.self="emit('close')">
    <section
      ref="panel"
      class="soft-dialog"
      role="dialog"
      aria-modal="true"
      :aria-label="title"
    >
      <header>
        <h2>{{ title }}</h2>
        <button
          class="icon-button"
          aria-label="关闭"
          @click="emit('close')"
        >
          <AppIcon name="close" :size="18" />
        </button>
      </header>
      <div class="dialog-body"><slot /></div>
      <footer v-if="$slots.actions"><slot name="actions" /></footer>
    </section>
  </div>
</template>
