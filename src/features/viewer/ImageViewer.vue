<script lang="ts">
let sessionInfoOpen = false;
let sessionFilmOpen = false;
</script>
<script setup lang="ts">
import TagTable from "./TagTable.vue";
import SoftSelect from "../../shared/ui/SoftSelect.vue";
import AppIcon from "../../shared/ui/AppIcon.vue";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { fitScale, zoomAround } from "./imageGeometry";
import "./lightbox.css";
import { api, errorMessage, imageUrl, invoke } from "../../shared/ipc";
import { copyImageToSystemClipboard } from "../../composables/useImageClipboard";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
import type { ImageDetail } from "../../shared/contracts/ImageDetail";
import type { UserFolder } from "../../shared/contracts/UserFolder";
const props = defineProps<{
  image: GalleryImage;
  images: GalleryImage[];
  folders: UserFolder[];
  readOnly?: boolean;
  total?: number;
  loadingMore?: boolean;
  canLoadMore?: boolean;
  loadError?: string;
  removalBusy?: boolean;
  removalError?: string;
  canUndoRemoval?: boolean;
}>();
const emit = defineEmits<{
  close: [];
  change: [image: GalleryImage];
  favorite: [id: string];
  tagSearch: [tag: string];
  metadataChanged: [];
  loadMore: [];
  trash: [];
  undoTrash: [];
}>();
const removal = ref<{
  command: string;
  args: Record<string, unknown>;
  label: string;
} | null>(null);
function requestRemoval(
  command: string,
  args: Record<string, unknown>,
  label: string,
) {
  removal.value = { command, args, label };
}
async function confirmRemoval() {
  const current = removal.value;
  if (!current) return;
  removal.value = null;
  await action(current.command, current.args);
}
const detail = ref<ImageDetail>(),
  info = ref(sessionInfoOpen),
  scale = ref(1),
  mode = ref<"fit" | "actual" | "custom">("fit"),
  film = ref(sessionFilmOpen),
  moreOpen = ref(false),
  imageReady = ref(false),
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
const root = ref<HTMLElement>(),
  stage = ref<HTMLElement>(),
  filmstrip = ref<HTMLElement>(),
  infoButton = ref<HTMLButtonElement>();
const viewport = ref({ width: 1, height: 1 }),
  dimensions = ref({ width: props.image.width, height: props.image.height });
const fittedScale = computed(() =>
  fitScale(
    dimensions.value.width,
    dimensions.value.height,
    Math.max(1, viewport.value.width - 32),
    Math.max(1, viewport.value.height - 24),
  ),
);
const pixelScale = computed(() =>
  mode.value === "fit"
    ? fittedScale.value
    : mode.value === "actual"
      ? 1
      : scale.value,
);
const imageStyle = computed(() => ({
  width: `${dimensions.value.width * pixelScale.value}px`,
  height: `${dimensions.value.height * pixelScale.value}px`,
  transform: `translate(${x.value}px,${y.value}px)`,
}));
let resize: ResizeObserver | undefined,
  advanceAfterLoad = false,
  pan: { x: number; y: number; px: number; py: number } | null = null;
const index = computed(() =>
  props.images.findIndex((i) => i.id === props.image.id),
);
const filmImages = computed(() =>
  props.images.slice(
    Math.max(0, index.value - 8),
    Math.min(props.images.length, index.value + 9),
  ),
);
const totalCount = computed(() =>
  Math.max(props.total ?? 0, props.images.length),
);
const autoTags = computed(() => [
  ...(detail.value?.autoTags.characterTags ?? []),
  ...(detail.value?.autoTags.generalTags ?? []),
]);
watch(info, (v) => (sessionInfoOpen = v));
watch(film, (v) => (sessionFilmOpen = v));
watch(
  () => props.image.id,
  async () => {
    scale.value = 1;
    mode.value = "fit";
    dimensions.value = { width: props.image.width, height: props.image.height };
    imageReady.value = false;
    moreOpen.value = false;
    advanceAfterLoad = false;
    pan = null;
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
  if (!n) return;
  if (delta > 0 && index.value + 1 >= n && props.canLoadMore) {
    advanceAfterLoad = true;
    if (!props.loadingMore) emit("loadMore");
    return;
  }
  advanceAfterLoad = false;
  emit("change", props.images[(index.value + delta + n) % n]);
}
watch(
  () => props.images.length,
  (length, previous) => {
    if (advanceAfterLoad && length > previous) {
      advanceAfterLoad = false;
      if (props.images[previous]) emit("change", props.images[previous]);
    }
  },
);
watch([index, film], async () => {
  await nextTick();
  filmstrip.value
    ?.querySelector('[aria-current="true"]')
    ?.scrollIntoView({ block: "nearest", inline: "center" });
});
watch([slideshow, interval], () => {
  clearInterval(timer);
  if (slideshow.value)
    timer = setInterval(() => step(1), Math.max(1, interval.value) * 1000);
});
function reset() {
  scale.value = 1;
  mode.value = "fit";
  x.value = 0;
  y.value = 0;
}
function zoomTo(value: number, anchorX = 0, anchorY = 0) {
  const next = Math.max(Math.min(0.05, fittedScale.value), Math.min(16, value));
  const point = zoomAround(
    pixelScale.value,
    next,
    x.value,
    y.value,
    anchorX,
    anchorY,
  );
  x.value = point.x;
  y.value = point.y;
  scale.value = next;
  mode.value = "custom";
}
function actual() {
  mode.value = "actual";
  x.value = 0;
  y.value = 0;
}
function doubleClick(event: MouseEvent) {
  if ((event.target as HTMLElement).closest("button")) return;
  if (mode.value === "fit") actual();
  else reset();
}
function loaded(event: Event) {
  const image = event.target as HTMLImageElement;
  if (image.naturalWidth && image.naturalHeight)
    dimensions.value = {
      width: image.naturalWidth,
      height: image.naturalHeight,
    };
  imageReady.value = true;
}
function startPan(e: PointerEvent) {
  if (e.button !== 0) return;
  if ((e.target as HTMLElement).closest("button")) return;
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
  const rect = stage.value?.getBoundingClientRect();
  if (!rect) return;
  zoomTo(
    pixelScale.value * (e.deltaY < 0 ? 1.15 : 1 / 1.15),
    e.clientX - rect.left - rect.width / 2,
    e.clientY - rect.top - rect.height / 2,
  );
}
function outside(e: PointerEvent) {
  if (!(e.target as HTMLElement).closest(".viewer-more"))
    moreOpen.value = false;
}
function dragWindow(e: PointerEvent) {
  if (
    e.button !== 0 ||
    (e.target as HTMLElement).closest("button,input,select,[role=combobox]")
  )
    return;
  void invoke("window_start_dragging_command").catch(
    (e) => (error.value = errorMessage(e)),
  );
}
function closeInfo() {
  info.value = false;
  nextTick(() => infoButton.value?.focus());
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
  moreOpen.value = false;
  try {
    await copyImageToSystemClipboard(props.image.id);
  } catch (e) {
    error.value = errorMessage(e);
  }
}
function key(e: KeyboardEvent) {
  if (document.querySelector(".select-popover,.soft-dialog,.error-dialog"))
    return;
  if (
    e.key !== "Escape" &&
    e.key !== "Tab" &&
    e.target instanceof Element &&
    e.target.closest(
      'input,textarea,select,[contenteditable]:not([contenteditable="false"]),[role="textbox"]',
    )
  )
    return;
  if (
    e.key === "Delete" &&
    !e.shiftKey &&
    !e.ctrlKey &&
    !e.altKey &&
    !e.metaKey
  ) {
    e.preventDefault();
    e.stopImmediatePropagation();
    if (!e.repeat && !props.readOnly && !props.removalBusy) {
      slideshow.value = false;
      emit("trash");
    }
  } else if (
    (e.ctrlKey || e.metaKey) &&
    !e.shiftKey &&
    e.key.toLowerCase() === "z"
  ) {
    e.preventDefault();
    e.stopImmediatePropagation();
    if (!e.repeat && props.canUndoRemoval && !props.removalBusy)
      emit("undoTrash");
  } else if (e.key === "Escape") {
    e.stopImmediatePropagation();
    e.preventDefault();
    if (moreOpen.value) moreOpen.value = false;
    else if (slideshow.value) slideshow.value = false;
    else if (info.value) closeInfo();
    else emit("close");
  } else if (e.key === "ArrowRight") {
    e.preventDefault();
    step(1);
  } else if (e.key === "ArrowLeft") {
    e.preventDefault();
    step(-1);
  } else if (e.key === "0") reset();
  else if (e.key === "1") actual();
  else if (e.key === "+" || e.key === "=") zoomTo(pixelScale.value * 1.2);
  else if (e.key === "-") zoomTo(pixelScale.value / 1.2);
  else if (e.key.toLowerCase() === "t") film.value = !film.value;
  else if (e.key.toLowerCase() === "i") info.value = !info.value;
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
  window.addEventListener("pointerdown", outside, true);
  if (stage.value) {
    resize = new ResizeObserver(([entry]) => {
      if (entry)
        viewport.value = {
          width: entry.contentRect.width,
          height: entry.contentRect.height,
        };
    });
    resize.observe(stage.value);
  }
});
onUnmounted(() => {
  serial++;
  clearInterval(timer);
  resize?.disconnect();
  window.removeEventListener("keydown", key, true);
  window.removeEventListener("pointerdown", outside, true);
  restoreFocus?.focus();
});
</script>
<template>
  <div
    ref="root"
    class="viewer lightbox-viewer"
    role="dialog"
    aria-modal="true"
    aria-label="大图查看器"
    tabindex="-1"
  >
    <header class="viewer-toolbar" @pointerdown="dragWindow">
      <button
        class="icon-button"
        @click="emit('close')"
        aria-label="关闭查看器"
      >
        <AppIcon name="left" /></button
      ><span class="truncate viewer-file-name" :title="image.fileName">{{
        image.fileName
      }}</span>
      <span class="viewer-position" aria-label="当前图片位置"
        >{{ Math.max(1, index + 1).toLocaleString() }} /
        {{ totalCount.toLocaleString() }}</span
      >
      <div class="viewer-actions">
        <button
          v-if="!readOnly"
          class="soft-button viewer-delete"
          aria-label="移入应用回收站"
          title="移入应用回收站（Delete）"
          :disabled="removalBusy"
          @click="
            slideshow = false;
            emit('trash');
          "
        >
          <AppIcon name="trash" :size="18" /><span>删除</span>
        </button>
        <button
          v-if="!readOnly"
          class="soft-button"
          @click="emit('favorite', image.id)"
          :aria-label="image.isFavorite ? '已收藏' : '收藏'"
        >
          <AppIcon name="heart" :filled="image.isFavorite" :size="18" /><span>{{
            image.isFavorite ? "已收藏" : "收藏"
          }}</span></button
        ><button
          ref="infoButton"
          class="soft-button"
          @click="info = !info"
          :aria-pressed="info"
          aria-label="信息"
        >
          <AppIcon name="info" :size="18" /><span>信息</span></button
        ><button
          v-if="!readOnly"
          class="soft-button"
          @click="slideshow = !slideshow"
          :aria-pressed="slideshow"
          :aria-label="slideshow ? '停止轮播' : '轮播'"
        >
          <AppIcon :name="slideshow ? 'pause' : 'play'" :size="18" /><span>{{
            slideshow ? "停止轮播" : "轮播"
          }}</span></button
        ><SoftSelect
          v-if="slideshow"
          :model-value="interval"
          @update:model-value="interval = Number($event)"
          :options="
            [3, 5, 10].map((value) => ({ value, label: value + ' 秒' }))
          "
          aria-label="轮播间隔"
        />
        <div class="viewer-more">
          <button
            class="icon-button"
            aria-label="更多图片操作"
            :aria-expanded="moreOpen"
            @click="moreOpen = !moreOpen"
          >
            <AppIcon name="more" :size="18" />
          </button>
          <div v-if="moreOpen" class="viewer-more-menu">
            <button class="text-button" @click="copy">
              <AppIcon name="copy" :size="16" />复制原图
            </button>
            <button
              class="text-button"
              @click="
                moreOpen = false;
                invoke('open_gallery_image_with_default_app_command', {
                  imageId: image.id,
                }).catch((e) => (error = errorMessage(e)));
              "
            >
              <AppIcon name="external" :size="16" />外部打开
            </button>
          </div>
        </div>
      </div>
    </header>
    <div
      v-if="canUndoRemoval || removalError"
      class="viewer-removal-status"
      role="status"
    >
      <span>{{ removalError || "已移入应用回收站" }}</span
      ><button
        v-if="canUndoRemoval"
        class="text-button"
        :disabled="removalBusy"
        @click="emit('undoTrash')"
      >
        撤销（Ctrl+Z）
      </button>
    </div>
    <div class="viewer-body" :class="{ 'with-info': info }">
      <div class="viewer-canvas">
        <div
          ref="stage"
          class="viewer-stage"
          @wheel.prevent="wheel"
          @pointerdown="startPan"
          @pointermove="move"
          @pointerup="pan = null"
          @pointercancel="pan = null"
          @dblclick="doubleClick"
          :aria-busy="!imageReady"
        >
          <img
            :src="imageUrl(image.path)"
            :alt="image.fileName"
            draggable="false"
            :style="imageStyle"
            @load="loaded"
            @error="
              imageReady = true;
              error = '无法读取原图，请检查文件或来源磁盘';
            "
          />
          <p v-if="!imageReady" class="viewer-image-loading" role="status">
            读取原图…
          </p>
          <p v-if="error" class="viewer-notice" role="alert">{{ error }}</p>
          <button
            class="icon-button viewer-arrow viewer-previous"
            @pointerdown.stop
            @click.stop="step(-1)"
            aria-label="上一张"
          >
            <AppIcon name="left" /></button
          ><button
            class="icon-button viewer-arrow viewer-next"
            @pointerdown.stop
            @click.stop="step(1)"
            aria-label="下一张"
          >
            <AppIcon name="right" />
          </button>
        </div>
        <div class="viewer-control-row">
          <div class="viewer-controls" role="toolbar" aria-label="看图工具">
            <button
              class="icon-button"
              aria-label="缩小"
              @click="zoomTo(pixelScale / 1.2)"
            >
              <AppIcon name="minimize" :size="17" />
            </button>
            <button
              class="text-button viewer-scale"
              aria-label="适应窗口"
              title="适应窗口 (0)"
              @click="reset"
            >
              {{ Math.round(pixelScale * 100) }}%<span v-if="mode === 'fit'">
                · 适应</span
              >
            </button>
            <button
              class="icon-button"
              aria-label="放大"
              @click="zoomTo(pixelScale * 1.2)"
            >
              <AppIcon name="plus" :size="17" />
            </button>
            <span class="viewer-control-divider" />
            <button
              class="text-button"
              aria-label="原始大小"
              :aria-pressed="mode === 'actual'"
              title="原始大小 (1)"
              @click="actual"
            >
              1:1
            </button>
            <button
              class="icon-button"
              aria-label="缩略图"
              :aria-pressed="film"
              title="缩略图 (T)"
              @click="film = !film"
            >
              <AppIcon name="images" :size="18" />
            </button>
          </div>
        </div>
        <nav
          v-if="film"
          ref="filmstrip"
          class="viewer-filmstrip"
          aria-label="附近图片"
        >
          <button
            v-for="item in filmImages"
            :key="item.id"
            class="viewer-thumb"
            :aria-label="`查看缩略图：${item.fileName}`"
            :aria-current="item.id === image.id ? 'true' : undefined"
            @click="
              advanceAfterLoad = false;
              emit('change', item);
            "
          >
            <img
              :src="imageUrl(item.thumbnailPath || item.path)"
              alt=""
              loading="lazy"
              decoding="async"
            />
          </button>
        </nav>
        <p v-if="loadingMore" class="viewer-next-loading" role="status">
          正在加载后续图片…
        </p>
        <p v-else-if="loadError" class="viewer-next-loading" role="alert">
          后续图片加载失败：{{ loadError }}
          <button class="text-button" @click="emit('loadMore')">重试</button>
        </p>
      </div>
      <aside v-if="info" class="viewer-info">
        <header class="viewer-info-heading">
          <h2>图片信息</h2>
          <button class="icon-button" aria-label="关闭信息" @click="closeInfo">
            <AppIcon name="close" :size="18" />
          </button>
        </header>
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
        <template v-if="!readOnly">
          <h3>虚拟分类</h3>
          <div class="chips">
            <span v-for="id in detail?.folderIds" :key="id" class="chip"
              >{{ folders.find((f) => f.id === id)?.name ?? id
              }}<button
                class="danger"
                aria-label="移除分类"
                @click="
                  requestRemoval(
                    'remove_image_from_user_folder_command',
                    {
                      imageId: image.id,
                      folderId: id,
                    },
                    '分类「' +
                      (folders.find((f) => f.id === id)?.name ?? id) +
                      '」',
                  )
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
                class="danger"
                aria-label="删除标签"
                @click="
                  requestRemoval(
                    'remove_image_user_custom_tag_command',
                    {
                      imageId: image.id,
                      tagText: t.tagText,
                    },
                    '标签「' + t.tagText + '」',
                  )
                "
              >
                <AppIcon name="close" :size="14" /></button></span
            ><span
              v-for="t in detail?.userTags.supplementTags"
              :key="t.tagEn"
              class="chip"
              >{{ t.tagZh || t.tagEn
              }}<button
                class="danger"
                aria-label="删除补充标签"
                @click="
                  requestRemoval(
                    'remove_image_user_supplement_tag_command',
                    {
                      imageId: image.id,
                      tagEn: t.tagEn,
                    },
                    '补充标签「' + (t.tagZh || t.tagEn) + '」',
                  )
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
        </template>
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
    <SoftDialog v-if="removal" title="确认移除" @close="removal = null"
      ><p>
        从当前图片移除{{ removal.label }}。该修改无法撤销，图片原文件保留。
      </p>
      <template #actions
        ><button class="text-button" @click="removal = null">取消</button
        ><button class="soft-button danger" @click="confirmRemoval">
          确认移除
        </button></template
      ></SoftDialog
    >
  </div>
</template>
