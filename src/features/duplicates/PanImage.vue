<script setup lang="ts">
import { ref, watch } from "vue";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
import { imageUrl } from "../../shared/ipc";
import { bytes } from "./format";
const props = defineProps<{ member: DuplicateMember; label: string }>();
const scale = ref(1),
  x = ref(0),
  y = ref(0),
  failed = ref(false);
let pan: { x: number; y: number; px: number; py: number } | null = null;
function reset() {
  scale.value = 1;
  x.value = 0;
  y.value = 0;
}
watch(
  () => props.member.image.id,
  () => {
    reset();
    failed.value = false;
  },
);
function zoom(direction: number) {
  scale.value = Math.max(
    0.1,
    Math.min(12, scale.value * (direction > 0 ? 1.2 : 1 / 1.2)),
  );
}
function start(e: PointerEvent) {
  if (e.button !== 0) return;
  pan = { x: x.value, y: y.value, px: e.clientX, py: e.clientY };
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}
function move(e: PointerEvent) {
  if (pan) {
    x.value = pan.x + e.clientX - pan.px;
    y.value = pan.y + e.clientY - pan.py;
  }
}
function key(e: KeyboardEvent) {
  if (["+", "="].includes(e.key)) zoom(1);
  else if (e.key === "-") zoom(-1);
  else if (e.key === "0") reset();
  else return;
  e.preventDefault();
}
</script>
<template>
  <section class="duplicate-pan-panel" :aria-label="label">
    <header>
      <strong>{{ label }}</strong
      ><span class="truncate" :title="member.image.fileName">{{
        member.image.fileName
      }}</span>
    </header>
    <div
      class="duplicate-pan-stage"
      tabindex="0"
      :aria-label="label + '，滚轮缩放，拖动平移，0 键恢复适应'"
      @wheel.prevent="zoom($event.deltaY < 0 ? 1 : -1)"
      @pointerdown="start"
      @pointermove="move"
      @pointerup="pan = null"
      @pointercancel="pan = null"
      @lostpointercapture="pan = null"
      @keydown="key"
    >
      <img
        v-if="!failed"
        :src="imageUrl(member.image.path)"
        :alt="member.image.fileName"
        draggable="false"
        :style="{ transform: `translate(${x}px,${y}px) scale(${scale})` }"
        @error="failed = true"
      />
      <p v-else>无法读取原图。请检查来源目录是否在线。</p>
    </div>
    <div class="duplicate-pan-controls">
      <button
        class="icon-button"
        :aria-label="label + '缩小'"
        @click="zoom(-1)"
      >
        −
      </button>
      <button
        class="soft-button compact"
        :aria-label="label + '恢复适应'"
        @click="reset"
      >
        {{ Math.round(scale * 100) }}% · 适应
      </button>
      <button class="icon-button" :aria-label="label + '放大'" @click="zoom(1)">
        ＋
      </button>
      <small
        >{{ member.image.width }} × {{ member.image.height }} ·
        {{ bytes(member.image.fileSize) }}</small
      >
    </div>
    <p class="duplicate-pan-path" :title="member.image.id">
      {{ member.image.id }}
    </p>
    <slot name="review" />
  </section>
</template>
