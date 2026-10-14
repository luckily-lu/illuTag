<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
import { imageUrl } from "../../shared/ipc";
import { bytes } from "./format";
import PanImage from "./PanImage.vue";
const props = defineProps<{
  members: DuplicateMember[];
  total: number;
  kept: Set<string>;
  selected: Set<string>;
  disabled: boolean;
  canMore: boolean;
  loadingMore: boolean;
  error?: string;
}>();
const emit = defineEmits<{
  close: [];
  keep: [member: DuplicateMember];
  select: [member: DuplicateMember];
  manage: [];
  more: [];
  next: [];
}>();
const root = ref<HTMLElement>(),
  activeId = ref(props.members[0]?.image.id),
  zoomed = ref<DuplicateMember | null>(null);
const width = ref(window.innerWidth);
const index = computed(() =>
  Math.max(
    0,
    props.members.findIndex((m) => m.image.id === activeId.value),
  ),
);
const current = computed(() => props.members[index.value]);
const columns = computed(() =>
  Math.max(
    1,
    Math.min(
      props.members.length,
      width.value < 720
        ? 2
        : Math.min(4, Math.ceil(Math.sqrt(props.members.length))),
    ),
  ),
);
const rows = computed(() =>
  Math.max(1, Math.ceil(props.members.length / columns.value)),
);
let previous: HTMLElement | null = null;
function resize() {
  width.value = window.innerWidth;
}
watch(
  () => props.members,
  (list, previousList) => {
    if (!list.some((m) => m.image.id === activeId.value)) {
      const previousIndex = Math.max(
        0,
        previousList.findIndex((m) => m.image.id === activeId.value),
      );
      activeId.value = list[Math.min(previousIndex, list.length - 1)]?.image.id;
    }
  },
);
async function move(delta: number) {
  const at = index.value + delta;
  if (at >= props.members.length && props.canMore && !props.loadingMore) {
    emit("more");
    return;
  }
  const member =
    props.members[Math.max(0, Math.min(props.members.length - 1, at))];
  if (!member) return;
  activeId.value = member.image.id;
  await nextTick();
  [...(root.value?.querySelectorAll<HTMLElement>("[data-review-image]") ?? [])]
    .find((el) => el.dataset.reviewImage === member.image.id)
    ?.scrollIntoView({ block: "nearest" });
}
function markAndManage() {
  const member = current.value;
  if (
    !member ||
    props.disabled ||
    member.stale ||
    props.kept.has(member.image.id)
  )
    return;
  if (!props.selected.has(member.image.id)) emit("select", member);
  emit("manage");
}
function key(event: KeyboardEvent) {
  if (
    event.isComposing ||
    document.querySelector(".soft-dialog,.select-popover")
  )
    return;
  const target = event.target instanceof HTMLElement ? event.target : null;
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopImmediatePropagation();
    if (zoomed.value) {
      zoomed.value = null;
      root.value?.focus();
    } else emit("close");
    return;
  }
  if (event.key === "Tab") {
    const controls = [
      ...(root.value?.querySelectorAll<HTMLElement>(
        'button:not(:disabled),input:not(:disabled),[tabindex="0"]',
      ) ?? []),
    ].filter((el) => el.getClientRects().length);
    if (event.shiftKey && document.activeElement === controls[0]) {
      event.preventDefault();
      controls.at(-1)?.focus();
    } else if (!event.shiftKey && document.activeElement === controls.at(-1)) {
      event.preventDefault();
      controls[0]?.focus();
    }
    return;
  }
  if (target?.closest("input,textarea,select,[contenteditable='true']")) return;
  if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
    event.preventDefault();
    if (!props.disabled) emit("manage");
    return;
  }
  if (event.ctrlKey || event.metaKey || event.altKey) return;
  if (
    !zoomed.value &&
    ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)
  ) {
    event.preventDefault();
    void move(
      (
        {
          ArrowLeft: -1,
          ArrowRight: 1,
          ArrowUp: -columns.value,
          ArrowDown: columns.value,
        } as Record<string, number>
      )[event.key],
    );
  } else if (
    event.key.toLowerCase() === "k" &&
    current.value &&
    !props.disabled
  ) {
    event.preventDefault();
    emit("keep", current.value);
  } else if (event.key === "Delete" || event.key === "Backspace") {
    event.preventDefault();
    markAndManage();
  } else if (
    event.key === " " &&
    !target?.closest("button") &&
    current.value &&
    !props.disabled
  ) {
    event.preventDefault();
    emit("select", current.value);
  } else if (
    event.key === "Enter" &&
    !target?.closest("button") &&
    current.value
  ) {
    event.preventDefault();
    zoomed.value = current.value;
  }
}
onMounted(() => {
  previous = document.activeElement as HTMLElement;
  root.value?.focus();
  window.addEventListener("keydown", key, true);
  window.addEventListener("resize", resize);
});
onUnmounted(() => {
  window.removeEventListener("keydown", key, true);
  window.removeEventListener("resize", resize);
  if (previous?.isConnected) previous.focus({ preventScroll: true });
});
</script>
<template>
  <section
    ref="root"
    class="duplicate-multi-review"
    role="dialog"
    aria-modal="true"
    aria-label="同组多图审核"
    tabindex="-1"
  >
    <header class="duplicate-multi-toolbar">
      <button
        class="icon-button"
        aria-label="关闭多图审核"
        @click="emit('close')"
      >
        ×
      </button>
      <strong>近似图片审核</strong
      ><span>{{ members.length }} / {{ total }} 张</span><span class="spacer" />
      <button
        class="soft-button compact"
        :disabled="disabled || !selected.size"
        @click="emit('manage')"
      >
        回收选中 {{ selected.size }} 张
      </button>
      <button
        class="text-button compact"
        :disabled="disabled"
        @click="emit('next')"
      >
        暂不处理，下一组
      </button>
    </header>
    <p class="duplicate-multi-help">
      方向键浏览 · K 保留 · Space 标记 · Delete 标记并回收 · Enter 放大 ·
      Ctrl+Enter 回收选中 · Esc 返回
    </p>
    <p v-if="error" class="error-message" role="alert">{{ error }}</p>
    <div v-if="zoomed" class="duplicate-multi-zoom">
      <button
        class="text-button"
        @click="
          zoomed = null;
          root?.focus();
        "
      >
        返回同组多图</button
      ><PanImage :member="zoomed" label="当前图片" />
    </div>
    <div
      v-else
      class="duplicate-multi-grid"
      :style="{ '--review-columns': columns, '--review-rows': rows }"
    >
      <article
        v-for="member in members"
        :key="member.image.id"
        :data-review-image="member.image.id"
        class="duplicate-multi-card"
        :class="{
          active: current?.image.id === member.image.id,
          kept: kept.has(member.image.id),
          selected: selected.has(member.image.id),
        }"
        @click="activeId = member.image.id"
      >
        <button
          class="duplicate-multi-image"
          :aria-label="'放大审核：' + member.image.fileName"
          @click="
            activeId = member.image.id;
            zoomed = member;
          "
        >
          <img
            :src="imageUrl(member.image.thumbnailPath || member.image.path)"
            :alt="member.image.fileName"
            loading="lazy"
          />
        </button>
        <strong class="truncate" :title="member.image.path">{{
          member.image.fileName
        }}</strong>
        <small
          >{{ member.image.width }} × {{ member.image.height }} ·
          {{ bytes(member.image.fileSize) }}</small
        >
        <div class="duplicate-card-actions">
          <button
            class="soft-button compact"
            :aria-label="'保留：' + member.image.fileName"
            :aria-pressed="kept.has(member.image.id)"
            :disabled="disabled || member.stale"
            @click="emit('keep', member)"
          >
            {{ kept.has(member.image.id) ? "已保留" : "保留" }}</button
          ><label
            ><input
              type="checkbox"
              :aria-label="'待处理：' + member.image.fileName"
              :checked="selected.has(member.image.id)"
              :disabled="disabled || kept.has(member.image.id) || member.stale"
              @change="emit('select', member)"
            />待处理</label
          >
        </div>
        <small v-if="member.stale" class="error-text"
          >文件已变化，请更新结果</small
        >
      </article>
    </div>
    <footer v-if="canMore" class="duplicate-multi-footer">
      <button class="text-button" :disabled="loadingMore" @click="emit('more')">
        {{ loadingMore ? "读取中…" : "显示本组更多图片" }}</button
      ><span>大组分批载入，已显示图片同时保留在审核区。</span>
    </footer>
  </section>
</template>
