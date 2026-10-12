<script lang="ts">
let sessionInfoOpen = false;
</script>
<script setup lang="ts">
import TagTable from "./TagTable.vue";
import SoftSelect from "../../shared/ui/SoftSelect.vue";
import AppIcon from "../../shared/ui/AppIcon.vue";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { api, errorMessage, imageUrl, invoke } from "../../shared/ipc";
import { copyImageToSystemClipboard } from "../../composables/useImageClipboard";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
import type { ImageDetail } from "../../shared/contracts/ImageDetail";
import type { UserFolder } from "../../shared/contracts/UserFolder";
const props = defineProps<{
  image: GalleryImage;
  images: GalleryImage[];
  folders: UserFolder[];
}>();
const emit = defineEmits<{
  close: [];
  change: [image: GalleryImage];
  favorite: [id: string];
  tagSearch: [tag: string];
  metadataChanged: [];
}>();
const detail = ref<ImageDetail>(),
  info = ref(sessionInfoOpen),
  scale = ref(1),
  x = ref(0),
  y = ref(0),
  error = ref(""),
  tag = ref(""),
  slideshow = ref(false),
  interval = ref(5),
  tagKind = ref("custom");
let timer: ReturnType<typeof setInterval> | undefined,
  serial = 0,
  restoreFocus: HTMLElement | null = null;
const root = ref<HTMLElement>();
const index = computed(() =>
  props.images.findIndex((i) => i.id === props.image.id),
);
const autoTags = computed(() => [
  ...(detail.value?.autoTags.characterTags ?? []),
  ...(detail.value?.autoTags.generalTags ?? []),
]);
watch(info, (v) => (sessionInfoOpen = v));
watch(
  () => props.image.id,
  async () => {
    scale.value = 1;
    x.value = 0;
    y.value = 0;
    detail.value = undefined;
    error.value = "";
    const token = ++serial;
    try {
      const d = await api.detail(props.image.id);
      if (token === serial) detail.value = d;
    } catch (e) {
      if (token === serial) error.value = errorMessage(e);
    }
  },
  { immediate: true },
);
function step(delta: number) {
  const n = props.images.length;
  if (n) emit("change", props.images[(index.value + delta + n) % n]);
}
watch([slideshow, interval], () => {
  clearInterval(timer);
  if (slideshow.value)
    timer = setInterval(() => step(1), Math.max(1, interval.value) * 1000);
});
function reset() {
  scale.value = 1;
  x.value = 0;
  y.value = 0;
}
let pan: { x: number; y: number; px: number; py: number } | null = null;
function startPan(e: PointerEvent) {
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
function wheel(e: WheelEvent) {
  scale.value = Math.max(
    0.1,
    Math.min(12, scale.value * (e.deltaY < 0 ? 1.15 : 1 / 1.15)),
  );
}
async function action(command: string, args: Record<string, unknown>) {
  try {
    await invoke(command, args);
    detail.value = await api.detail(props.image.id);
    emit("metadataChanged");
  } catch (e) {
    error.value = errorMessage(e);
  }
}
async function addTag() {
  if (!tag.value.trim()) return;
  await action(
    tagKind.value === "custom"
      ? "add_image_user_custom_tag_command"
      : "add_image_user_supplement_tag_command",
    tagKind.value === "custom"
      ? { imageId: props.image.id, tagText: tag.value.trim() }
      : { imageId: props.image.id, tagEn: tag.value.trim(), tagZh: null },
  );
  tag.value = "";
}
async function copy() {
  try {
    await copyImageToSystemClipboard(props.image.id);
  } catch (e) {
    error.value = errorMessage(e);
  }
}
function key(e: KeyboardEvent) {
  if (document.querySelector(".select-popover,.soft-dialog")) return;
  if (
    e.key !== "Escape" &&
    e.key !== "Tab" &&
    (e.target as HTMLElement)?.matches("input,textarea,select")
  )
    return;
  if (e.key === "Escape") {
    e.stopImmediatePropagation();
    e.preventDefault();
    if (slideshow.value) slideshow.value = false;
    else emit("close");
  } else if (e.key === "ArrowRight") step(1);
  else if (e.key === "ArrowLeft") step(-1);
  else if (e.key === "0") reset();
  else if (e.key === "Tab") {
    const nodes = Array.from(
      root.value?.querySelectorAll<HTMLElement>(
        'button,input,select,[tabindex="0"]',
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
  restoreFocus = document.activeElement as HTMLElement;
  root.value?.focus();
  window.addEventListener("keydown", key, true);
});
onUnmounted(() => {
  serial++;
  clearInterval(timer);
  window.removeEventListener("keydown", key, true);
  restoreFocus?.focus();
});
</script>
<template>
  <div
    ref="root"
    class="viewer"
    role="dialog"
    aria-modal="true"
    aria-label="大图查看器"
    tabindex="-1"
  >
    <header class="viewer-toolbar">
      <button
        class="icon-button"
        @click="emit('close')"
        aria-label="关闭查看器"
      >
        <AppIcon name="close" /></button
      ><span class="truncate">{{ image.fileName }}</span
      ><span class="spacer" /><button
        class="soft-button"
        @click="emit('favorite', image.id)"
      >
        <AppIcon name="heart" :filled="image.isFavorite" :size="18" />{{
          image.isFavorite ? "已收藏" : "收藏"
        }}</button
      ><button class="soft-button" @click="info = !info" :aria-pressed="info">
        <AppIcon name="info" :size="18" />信息</button
      ><button
        class="soft-button"
        @click="slideshow = !slideshow"
        :aria-pressed="slideshow"
      >
        <AppIcon :name="slideshow ? 'pause' : 'play'" :size="18" />{{
          slideshow ? "停止轮播" : "轮播"
        }}</button
      ><SoftSelect
        v-if="slideshow"
        :model-value="interval"
        @update:model-value="interval = Number($event)"
        :options="[3, 5, 10].map((value) => ({ value, label: value + ' 秒' }))"
        aria-label="轮播间隔"
      />
    </header>
    <div class="viewer-body" :class="{ 'with-info': info }">
      <div
        class="viewer-stage"
        @wheel.prevent="wheel"
        @pointerdown="startPan"
        @pointermove="move"
        @pointerup="pan = null"
        @pointercancel="pan = null"
        @dblclick="reset"
      >
        <img
          :src="imageUrl(image.path)"
          :alt="image.fileName"
          draggable="false"
          :style="{ transform: `translate(${x}px,${y}px) scale(${scale})` }"
        />
        <div class="viewer-pagination">
          <button
            class="icon-button"
            @pointerdown.stop
            @click.stop="step(-1)"
            aria-label="上一张"
          >
            <AppIcon name="left" /></button
          ><button class="soft-button" @pointerdown.stop @click.stop="reset">
            {{ Math.round(scale * 100) }}% · 适应</button
          ><button
            class="icon-button"
            @pointerdown.stop
            @click.stop="step(1)"
            aria-label="下一张"
          >
            <AppIcon name="right" />
          </button>
        </div>
      </div>
      <aside v-if="info" class="viewer-info">
        <h2>图片信息</h2>
        <p>
          {{ image.width }} × {{ image.height }} ·
          {{ (image.fileSize / 1048576).toFixed(2) }} MB
        </p>
        <p class="path-text">{{ image.path }}</p>
        <div class="button-row">
          <button class="soft-button" @click="copy">
            <AppIcon name="copy" :size="17" />复制原图</button
          ><button
            class="soft-button"
            @click="
              invoke('open_gallery_image_with_default_app_command', {
                imageId: image.id,
              }).catch((e) => (error = errorMessage(e)))
            "
          >
            <AppIcon name="external" :size="17" />外部打开
          </button>
        </div>
        <p v-if="error" class="error-text" role="alert">{{ error }}</p>
        <h3>虚拟分类</h3>
        <div class="chips">
          <span v-for="id in detail?.folderIds" :key="id" class="chip"
            >{{ folders.find((f) => f.id === id)?.name ?? id
            }}<button
              aria-label="移除分类"
              @click="
                action('remove_image_from_user_folder_command', {
                  imageId: image.id,
                  folderId: id,
                })
              "
            >
              <AppIcon name="close" :size="14" /></button
          ></span>
        </div>
        <SoftSelect
          :model-value="null"
          :options="folders.map((f) => ({ value: f.id, label: f.name }))"
          placeholder="添加分类…"
          aria-label="添加到分类"
          @change="
            action('assign_image_to_user_folder_command', {
              imageId: image.id,
              folderId: Number($event),
            })
          "
        />
        <h3>手动标签</h3>
        <div class="chips">
          <span
            v-for="t in detail?.userTags.customTags"
            :key="t.tagText"
            class="chip"
            >{{ t.tagText
            }}<button
              aria-label="删除标签"
              @click="
                action('remove_image_user_custom_tag_command', {
                  imageId: image.id,
                  tagText: t.tagText,
                })
              "
            >
              <AppIcon name="close" :size="14" /></button></span
          ><span
            v-for="t in detail?.userTags.supplementTags"
            :key="t.tagEn"
            class="chip"
            >{{ t.tagZh || t.tagEn
            }}<button
              aria-label="删除补充标签"
              @click="
                action('remove_image_user_supplement_tag_command', {
                  imageId: image.id,
                  tagEn: t.tagEn,
                })
              "
            >
              <AppIcon name="close" :size="14" /></button
          ></span>
        </div>
        <form class="inline-form" @submit.prevent="addTag">
          <SoftSelect
            :model-value="tagKind"
            @update:model-value="tagKind = String($event)"
            :options="[
              { value: 'custom', label: '自定义' },
              { value: 'supplement', label: '补充模型标签' },
            ]"
            aria-label="标签类型"
          /><input
            v-model="tag"
            placeholder="新增标签"
            aria-label="新增标签"
          /><button class="soft-button" type="submit">添加</button>
        </form>
        <section class="info-tag-section">
          <h3>
            自动标签 <span class="section-count">{{ autoTags.length }}</span>
          </h3>
          <p v-if="!autoTags.length">尚未自动打标</p>
          <template
            v-for="group in [
              { label: '角色标签', tags: detail?.autoTags.characterTags ?? [] },
              { label: '通用标签', tags: detail?.autoTags.generalTags ?? [] },
            ]"
            :key="group.label"
          >
            <div v-if="group.tags.length" class="tag-table-group">
              <h4>
                {{ group.label }}
                <span class="section-count">{{ group.tags.length }}</span>
              </h4>
              <TagTable
                :tags="group.tags"
                :label="group.label"
                @search="emit('tagSearch', $event)"
              />
            </div>
          </template>
        </section>
      </aside>
    </div>
  </div>
</template>
