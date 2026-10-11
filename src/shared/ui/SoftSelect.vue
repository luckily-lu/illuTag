<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, useId } from "vue";
import AppIcon from "./AppIcon.vue";
defineOptions({ inheritAttrs: false });
type Value = string | number | null;
const props = defineProps<{
  modelValue?: Value;
  options: { value: Value; label: string; disabled?: boolean }[];
  placeholder?: string;
  disabled?: boolean;
}>();
const emit = defineEmits<{
  "update:modelValue": [value: Value];
  change: [value: Value];
}>();
const id = useId(),
  trigger = ref<HTMLButtonElement>(),
  popup = ref<HTMLElement>(),
  expanded = ref(false),
  active = ref(0),
  position = ref({ left: "0px", top: "0px", width: "0px", maxHeight: "240px" });
const label = computed(
  () =>
    props.options.find((o) => o.value === props.modelValue)?.label ??
    props.placeholder ??
    "请选择",
);
function locate() {
  if (!trigger.value) return;
  const r = trigger.value.getBoundingClientRect();
  const below = innerHeight - r.bottom - 12,
    above = r.top - 12,
    h = Math.min(300, Math.max(below, above));
  position.value = {
    left: `${Math.max(8, Math.min(r.left, innerWidth - r.width - 8))}px`,
    top: `${below >= Math.min(220, h) ? r.bottom + 8 : Math.max(8, r.top - h - 8)}px`,
    width: `${r.width}px`,
    maxHeight: `${Math.max(90, h)}px`,
  };
}
async function open() {
  if (props.disabled) return;
  expanded.value = true;
  active.value = Math.max(
    0,
    props.options.findIndex((o) => o.value === props.modelValue && !o.disabled),
  );
  locate();
  await nextTick();
  popup.value?.focus({ preventScroll: true });
}
function close(focus = true) {
  expanded.value = false;
  if (focus) trigger.value?.focus();
}
function choose(index: number) {
  const item = props.options[index];
  if (!item || item.disabled) return;
  emit("update:modelValue", item.value);
  emit("change", item.value);
  close();
}
function move(delta: number) {
  for (let n = 0; n < props.options.length; n++) {
    active.value =
      (active.value + delta + props.options.length) % props.options.length;
    if (!props.options[active.value]?.disabled) break;
  }
  nextTick(() =>
    popup.value
      ?.querySelector(`#${id}-${active.value}`)
      ?.scrollIntoView({ block: "nearest" }),
  );
}
function key(event: KeyboardEvent) {
  if (!expanded.value) return;
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopImmediatePropagation();
    close();
  } else if (
    ["ArrowDown", "ArrowUp", "Home", "End", "Enter", " "].includes(event.key)
  ) {
    event.preventDefault();
    event.stopImmediatePropagation();
    if (event.key === "Enter" || event.key === " ") choose(active.value);
    else if (event.key === "Home") {
      active.value = -1;
      move(1);
    } else if (event.key === "End") {
      active.value = 0;
      move(-1);
    } else move(event.key === "ArrowDown" ? 1 : -1);
  } else if (event.key === "Tab") close(false);
}
function outside(event: PointerEvent) {
  if (
    expanded.value &&
    !trigger.value?.contains(event.target as Node) &&
    !popup.value?.contains(event.target as Node)
  )
    close(false);
}
function scroll(event: Event) {
  if (expanded.value && !popup.value?.contains(event.target as Node))
    locate();
}
onMounted(() => {
  window.addEventListener("keydown", key, true);
  window.addEventListener("pointerdown", outside, true);
  window.addEventListener("resize", locate);
  window.addEventListener("scroll", scroll, true);
});
onUnmounted(() => {
  window.removeEventListener("keydown", key, true);
  window.removeEventListener("pointerdown", outside, true);
  window.removeEventListener("resize", locate);
  window.removeEventListener("scroll", scroll, true);
});
</script>
<template>
  <button
    ref="trigger"
    v-bind="$attrs"
    type="button"
    class="soft-select"
    role="combobox"
    :disabled="disabled"
    :aria-expanded="expanded"
    aria-haspopup="listbox"
    :aria-controls="id"
    @click="expanded ? close() : open()"
    @keydown.down.prevent="open()"
    @keydown.up.prevent="open()"
  >
    <span>{{ label }}</span
    ><AppIcon name="down" :size="16" />
  </button>
  <Teleport to="body"
    ><div
      v-if="expanded"
      :id="id"
      ref="popup"
      class="select-popover"
      :style="position"
      role="listbox"
      tabindex="-1"
      :aria-activedescendant="`${id}-${active}`"
      :aria-label="String($attrs['aria-label'] ?? '选择选项')"
    >
      <button
        v-for="(option, i) in options"
        :id="`${id}-${i}`"
        :key="String(option.value)"
        type="button"
        role="option"
        tabindex="-1"
        :aria-selected="option.value === modelValue"
        :disabled="option.disabled"
        :class="{ active: i === active }"
        @pointermove="active = i"
        @click="choose(i)"
      >
        <span>{{ option.label }}</span
        ><AppIcon v-if="option.value === modelValue" name="check" :size="16" />
      </button>
      <p v-if="!options.length">暂无可选项</p>
    </div></Teleport
  >
</template>
