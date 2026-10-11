<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, useId } from "vue";
import AppIcon from "./AppIcon.vue";
const props = defineProps<{
  label: string;
  actions: { id: string; label: string; danger?: boolean }[];
}>();
const emit = defineEmits<{ choose: [id: string] }>();
const id = useId(),
  expanded = ref(false),
  trigger = ref<HTMLButtonElement>(),
  popup = ref<HTMLElement>(),
  position = ref({ left: "0px", top: "0px", maxHeight: "300px" });
let active = 0;
let anchorPosition: { left: number; top: number } | undefined;
function close(focus = true) {
  expanded.value = false;
  if (focus) trigger.value?.focus({ preventScroll: true });
}
function locate() {
  if (!trigger.value || !popup.value) return;
  const anchor = trigger.value.getBoundingClientRect(),
    menu = popup.value.getBoundingClientRect();
  anchorPosition = { left: anchor.left, top: anchor.top };
  const left =
    anchor.right + 8 + menu.width <= innerWidth - 10
      ? anchor.right + 8
      : anchor.left - menu.width - 8;
  position.value = {
    left: `${Math.max(10, Math.min(left, innerWidth - menu.width - 10))}px`,
    top: `${Math.max(10, Math.min(anchor.top, innerHeight - menu.height - 10))}px`,
    maxHeight: `${Math.max(100, innerHeight - 20)}px`,
  };
}
async function open(last = false) {
  expanded.value = true;
  active = last ? props.actions.length - 1 : 0;
  await nextTick();
  locate();
  popup.value
    ?.querySelectorAll<HTMLButtonElement>("button")
    [active]?.focus({ preventScroll: true });
}
function choose(action: string) {
  close();
  emit("choose", action);
}
function key(e: KeyboardEvent) {
  if (!expanded.value) return;
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopImmediatePropagation();
    close();
  } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) {
    e.preventDefault();
    e.stopImmediatePropagation();
    active =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? props.actions.length - 1
          : (active + (e.key === "ArrowDown" ? 1 : -1) + props.actions.length) %
            props.actions.length;
    popup.value?.querySelectorAll<HTMLButtonElement>("button")[active]?.focus();
  } else if (e.key === "Tab") {
    close();
  }
}
function outside(e: PointerEvent) {
  if (
    expanded.value &&
    !trigger.value?.contains(e.target as Node) &&
    !popup.value?.contains(e.target as Node)
  )
    close(false);
}
function scroll(e: Event) {
  if (!expanded.value || popup.value?.contains(e.target as Node)) return;
  const anchor = trigger.value?.getBoundingClientRect();
  if (
    anchor &&
    anchorPosition &&
    (Math.abs(anchor.left - anchorPosition.left) > 0.5 ||
      Math.abs(anchor.top - anchorPosition.top) > 0.5)
  )
    close(false);
}
function focusOutside(e: FocusEvent) {
  if (
    expanded.value &&
    !trigger.value?.contains(e.target as Node) &&
    !popup.value?.contains(e.target as Node)
  )
    close(false);
}
onMounted(() => {
  window.addEventListener("pointerdown", outside, true);
  window.addEventListener("focusin", focusOutside, true);
  window.addEventListener("keydown", key, true);
  window.addEventListener("resize", locate);
  window.addEventListener("scroll", scroll, true);
});
onUnmounted(() => {
  window.removeEventListener("pointerdown", outside, true);
  window.removeEventListener("focusin", focusOutside, true);
  window.removeEventListener("keydown", key, true);
  window.removeEventListener("resize", locate);
  window.removeEventListener("scroll", scroll, true);
});
</script>
<template>
  <button
    ref="trigger"
    type="button"
    class="icon-button folder-menu-trigger"
    :aria-label="label"
    aria-haspopup="menu"
    :aria-expanded="expanded"
    :aria-controls="id"
    @click="expanded ? close() : open()"
    @keydown.down.prevent="open()"
    @keydown.up.prevent="open(true)"
  >
    <AppIcon name="more" :size="18" />
  </button>
  <Teleport to="body">
    <div
      v-if="expanded"
      :id="id"
      ref="popup"
      class="menu-popover action-menu"
      :style="position"
      role="menu"
      :aria-label="label"
    >
      <button
        v-for="(action, index) in actions"
        :key="action.id"
        type="button"
        role="menuitem"
        :class="{ danger: action.danger }"
        @focus="active = index"
        @click="choose(action.id)"
      >
        {{ action.label }}
      </button>
    </div>
  </Teleport>
</template>
