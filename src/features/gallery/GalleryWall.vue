<script setup lang="ts">
import AppIcon from "../../shared/ui/AppIcon.vue";
import { computed, onMounted, onUnmounted, ref, shallowRef, watch } from "vue";
import { useWorkspace } from "../../shared/workspace";
import { imageUrl } from "../../shared/ipc";
import { MasonryLayout } from "./masonry";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
const props = defineProps<{ thumbnails: boolean }>();
const { gallery, selection } = useWorkspace();
const emit = defineEmits<{
  open: [image: GalleryImage];
  favorite: [id: string];
  drag: [id: string, event: PointerEvent];
}>();
const root = ref<HTMLElement>(),
  width = ref(0),
  top = ref(0),
  height = ref(500),
  items = shallowRef(new MasonryLayout());
let observer: ResizeObserver | undefined;
let resizeFrame = 0;
function measure() {
  const node = root.value;
  if (!node) return;
  const style = getComputedStyle(node);
  width.value = Math.max(
    1,
    node.clientWidth -
      parseFloat(style.paddingLeft) -
      parseFloat(style.paddingRight),
  );
  height.value = node.clientHeight;
  top.value = node.scrollTop;
}
function resize() {
  cancelAnimationFrame(resizeFrame);
  resizeFrame = requestAnimationFrame(measure);
}
const rendered = computed(() => {
  const buffer = Math.max(480, height.value * 0.8);
  return items.value.visible(
    top.value - buffer,
    top.value + height.value + buffer,
  );
});
const byId = computed(
  () => new Map(gallery.images.value.map((i) => [i.id, i])),
);
const scopeScroll = new Map<string, number>();
let lastScope = "all";
const scopeKey = computed(
  () => `${gallery.query.value.scope}:${gallery.query.value.folderId}`,
);
watch(
  [gallery.images, width, () => props.thumbnails],
  () => {
    const layout = items.value;
    layout.update(
      gallery.images.value.map((i) =>
        props.thumbnails ? i : { ...i, thumbnailPath: null },
      ),
      width.value,
      imageUrl,
    );
    items.value = Object.assign(
      Object.create(Object.getPrototypeOf(layout)),
      layout,
    );
  },
  { immediate: true },
);
watch(scopeKey, (key) => {
  if (root.value) {
    scopeScroll.set(lastScope, root.value.scrollTop);
    root.value.scrollTop = scopeScroll.get(key) ?? 0;
  }
  lastScope = key;
});
function fallbackImage(event: Event, id: string) {
  const node = event.target as HTMLImageElement;
  const path = byId.value.get(id)?.path;
  if (path && node.src !== imageUrl(path)) node.src = imageUrl(path);
  else node.classList.add("missing-image");
}
function scroll() {
  if (!root.value) return;
  top.value = root.value.scrollTop;
  if (top.value + height.value > items.value.height - 1200) void gallery.more();
}
function click(id: string) {
  if (selection.active.value) selection.toggle(id);
  else {
    const image = byId.value.get(id);
    if (image) emit("open", image);
  }
}
let sweep = false;
function enter(id: string, event: PointerEvent) {
  if (
    selection.active.value &&
    event.buttons === 1 &&
    sweep &&
    !selection.isSelected(id)
  )
    selection.toggle(id);
}
function down(id: string, event: PointerEvent) {
  if (selection.active.value) {
    sweep = true;
    return;
  }
  emit("drag", id, event);
}
function up() {
  sweep = false;
}
onMounted(() => {
  measure();
  observer = new ResizeObserver(measure);
  if (root.value) observer.observe(root.value);
  window.addEventListener("resize", resize);
  window.addEventListener("pointerup", up);
});
onUnmounted(() => {
  cancelAnimationFrame(resizeFrame);
  observer?.disconnect();
  window.removeEventListener("resize", resize);
  window.removeEventListener("pointerup", up);
});
</script>
<template>
  <section
    ref="root"
    class="gallery-wall"
    aria-label="图片图库"
    :aria-busy="gallery.loading.value"
    @scroll.passive="scroll"
  >
    <div v-if="gallery.error.value" class="empty-state" role="alert">
      <h2>无法加载图片</h2>
      <p>{{ gallery.error.value }}</p>
      <button class="soft-button" @click="gallery.search">重试</button>
    </div>
    <div
      v-else-if="!gallery.images.value.length && !gallery.loading.value"
      class="empty-state"
    >
      <span class="empty-mark"><AppIcon name="images" :size="36" /></span>
      <h2>
        {{
          gallery.query.value.tags.length ||
          gallery.query.value.naturalLanguage ||
          gallery.query.value.fileName
            ? "没有匹配的图片"
            : "从你的图库开始"
        }}
      </h2>
      <p>在设置中登记本地来源，然后到任务中心扫描图片。</p>
    </div>
    <div
      v-else
      class="masonry"
      :style="{ height: Math.max(0, items.height) + 'px' }"
    >
      <div
        v-for="item in rendered"
        :key="item.id"
        class="image-tile"
        :class="{ 'is-selected': selection.isSelected(item.id) }"
        :data-gallery-image-id="item.id"
        :style="{
          left: item.x + 'px',
          top: item.y + 'px',
          width: item.width + 'px',
          height: item.height + 'px',
        }"
        @pointerenter="enter(item.id, $event)"
      >
        <button
          class="image-open"
          :aria-label="
            selection.active.value
              ? '选择 ' + byId.get(item.id)?.fileName
              : '查看 ' + byId.get(item.id)?.fileName
          "
          :aria-pressed="
            selection.active.value ? selection.isSelected(item.id) : undefined
          "
          @click="click(item.id)"
          @pointerdown="down(item.id, $event)"
          @contextmenu.prevent="selection.toggle(item.id)"
        >
          <img
            :src="item.thumbnailUrl"
            :alt="byId.get(item.id)?.fileName"
            draggable="false"
            loading="lazy"
            @error="fallbackImage($event, item.id)"
          />
        </button>
        <button
          v-if="
            !selection.active.value && gallery.query.value.scope !== 'trash'
          "
          class="tile-action tile-select"
          aria-label="选择图片"
          @click.stop="selection.toggle(item.id)"
        >
          <AppIcon name="check" :size="17" />
        </button>
        <button
          v-if="gallery.query.value.scope !== 'trash'"
          class="tile-action tile-favorite"
          :class="{ 'is-active': byId.get(item.id)?.isFavorite }"
          :aria-label="byId.get(item.id)?.isFavorite ? '取消收藏' : '收藏图片'"
          @click.stop="emit('favorite', item.id)"
        >
          <AppIcon
            name="heart"
            :size="18"
            :filled="byId.get(item.id)?.isFavorite"
          />
        </button>
        <span
          v-if="selection.isSelected(item.id)"
          class="selection-check"
          aria-hidden="true"
          ><AppIcon name="check" :size="17"
        /></span>
      </div>
    </div>
    <div
      v-if="gallery.loading.value || gallery.pageLoading.value"
      class="gallery-loading"
      role="status"
    >
      正在加载…
    </div>
  </section>
</template>
