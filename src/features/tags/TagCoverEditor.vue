<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
import AppIcon from "../../shared/ui/AppIcon.vue";
import { api, errorMessage, imageUrl } from "../../shared/ipc";
import type { TagCover } from "../../shared/contracts/TagCover";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
const props = defineProps<{ tag: string; label: string; cover?: TagCover }>();
const emit = defineEmits<{ close: []; saved: [cover: TagCover] }>();
const image = ref<GalleryImage | null>(props.cover?.image ?? null),
  candidates = ref<GalleryImage[]>([]),
  total = ref(0);
const x = ref(props.cover?.x ?? 50),
  y = ref(props.cover?.y ?? 50),
  zoom = ref(props.cover?.zoom ?? 1),
  busy = ref(false),
  loading = ref(false),
  error = ref("");
const source = (item: GalleryImage) =>
  imageUrl(item.thumbnailPath || item.path);
const style = computed(() => ({
  objectPosition: `${x.value}% ${y.value}%`,
  transform: `scale(${zoom.value})`,
  transformOrigin: `${x.value}% ${y.value}%`,
}));
let drag: {
  x: number;
  y: number;
  startX: number;
  startY: number;
  id: number;
} | null = null;
function start(e: PointerEvent) {
  if (e.button !== 0 || !image.value) return;
  e.preventDefault();
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  drag = {
    x: e.clientX,
    y: e.clientY,
    startX: x.value,
    startY: y.value,
    id: e.pointerId,
  };
}
function move(e: PointerEvent) {
  if (!drag || drag.id !== e.pointerId) return;
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
  x.value = Math.max(
    0,
    Math.min(100, drag.startX - ((e.clientX - drag.x) * 100) / r.width),
  );
  y.value = Math.max(
    0,
    Math.min(100, drag.startY - ((e.clientY - drag.y) * 100) / r.height),
  );
}
function choose(item: GalleryImage) {
  image.value = item;
  x.value = 50;
  y.value = 50;
  zoom.value = 1;
}
async function more() {
  if (loading.value) return;
  loading.value = true;
  try {
    const p = await api.tagCoverCandidates(props.tag, candidates.value.length, 24);
    candidates.value = [...candidates.value, ...p.images];
    total.value = p.totalImageCount;
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    loading.value = false;
  }
}
async function save(reset = false) {
  if (busy.value) return;
  busy.value = true;
  try {
    const cover = await api.saveTagCover({
      tag: props.tag,
      imageId: reset ? null : (image.value?.id ?? null),
      x: x.value,
      y: y.value,
      zoom: zoom.value,
    });
    emit("saved", cover);
    emit("close");
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}
onMounted(() => void more());
</script>
<template>
  <SoftDialog :title="`设置标签封面 · ${label}`" @close="emit('close')">
    <div class="tag-cover-editor">
      <div
        class="tag-cover-stage"
        aria-label="拖动调整封面焦点"
        @pointerdown="start"
        @pointermove="move"
        @pointerup="drag = null"
        @pointercancel="drag = null"
      >
        <img
          v-if="image"
          :src="source(image)"
          :style="style"
          alt="封面效果"
          draggable="false"
        />
        <span v-else>请从下方选择图片</span>
      </div>
      <p>拖动调整焦点，也可使用下方滑块。只改变封面展示，原图保持不变。</p>
      <p>仅显示此标签置信度至少 99%、未被其他标签用作封面的图片。</p>
      <label
        >水平位置<input v-model.number="x" type="range" min="0" max="100"
      /></label>
      <label
        >垂直位置<input v-model.number="y" type="range" min="0" max="100"
      /></label>
      <label
        >缩放 {{ zoom.toFixed(1) }}×<input
          v-model.number="zoom"
          type="range"
          min="1"
          max="3"
          step="0.05"
      /></label>
      <div class="tag-cover-choices">
        <button
          v-for="item in candidates"
          :key="item.id"
          :class="{ selected: image?.id === item.id }"
          :aria-pressed="image?.id === item.id"
          :aria-label="`选用封面：${item.fileName}`"
          @click="choose(item)"
        >
          <img
            :src="source(item)"
            :alt="item.fileName"
            loading="lazy"
            decoding="async"
          />
        </button>
      </div>
      <button
        v-if="candidates.length < total"
        class="text-button"
        :disabled="loading"
        @click="more"
      >
        {{ loading ? "加载中…" : "更多图片" }}
      </button>
      <p v-if="!loading && !candidates.length">暂无满足 99% 置信度且未被其他标签占用的图片。</p>
      <p v-if="error" class="inline-error" role="alert">{{ error }}</p>
    </div>
    <template #actions
      ><button class="text-button" :disabled="busy" @click="save(true)">
        恢复自动选择</button
      ><span class="spacer" /><button
        class="text-button"
        :disabled="busy"
        @click="emit('close')"
      >
        取消</button
      ><button
        class="soft-button primary"
        :disabled="busy || !image"
        @click="save()"
      >
        {{ busy ? "保存中…" : "固定此封面" }}
      </button></template
    >
  </SoftDialog>
</template>
