<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import AppIcon from "../../shared/ui/AppIcon.vue";
import SoftSelect from "../../shared/ui/SoftSelect.vue";
import TagMaintenance from "./TagManager.vue";
import TagCoverEditor from "./TagCoverEditor.vue";
import { imageUrl } from "../../shared/ipc";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
import type { TagWorkbenchState } from "./useTagWorkbench";
import "./tag-workbench.css";
const props = defineProps<{ workspace: TagWorkbenchState }>();
const emit = defineEmits<{
  browse: [tags: string[]];
  open: [image: GalleryImage];
}>();
const w = props.workspace,
  t = w.catalog,
  editor = ref<{ tag: string; label: string } | null>(null),
  narrowPanel = ref<"tags" | "results">("tags"),
  previewColumnCount = ref(2),
  catalogScroll = ref<HTMLElement | null>(null),
  resultsScroll = ref<HTMLElement | null>(null),
  resultsEnd = ref<HTMLElement | null>(null);
let stopPreviewObserver = () => {};
let stopPreviewResize = () => {};
watch(
  resultsScroll,
  (root) => {
    stopPreviewResize();
    if (!root) return;
    const resize = () => {
      const width = root.clientWidth;
      if (width > 0)
        previewColumnCount.value = width >= 880 ? 4 : width >= 620 ? 3 : 2;
    };
    const observer = new ResizeObserver(resize);
    observer.observe(root);
    resize();
    stopPreviewResize = () => observer.disconnect();
  },
  { flush: "post" },
);
watch(
  [resultsScroll, resultsEnd],
  ([root, target]) => {
    stopPreviewObserver();
    if (!root || !target) return;
    let active = true,
      nearEnd = false,
      visibleEnd = false,
      consumed = false;
    function loadNextPage() {
      if (
        !active ||
        !nearEnd ||
        !visibleEnd ||
        consumed ||
        !w.previewOpen.value ||
        w.managing.value ||
        w.loading.value ||
        w.pageLoading.value ||
        w.previewError.value ||
        !w.hasMore.value
      )
        return;
      // One page per approach to the end; a short page must not drain the library.
      consumed = true;
      void w.preview(true);
    }
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!active || !entry) return;
        nearEnd = entry.isIntersecting;
        if (!nearEnd) consumed = false;
        loadNextPage();
      },
      { root, rootMargin: "0px 0px 240px 0px" },
    );
    // A hidden narrow-screen tab must not prefetch just because its own
    // scroll container is near the end.
    const visibilityObserver = new IntersectionObserver(
      ([entry]) => {
        if (!active || !entry) return;
        visibleEnd = entry.isIntersecting;
        if (!visibleEnd) consumed = false;
        loadNextPage();
      },
      { rootMargin: "0px 0px 240px 0px" },
    );
    observer.observe(target);
    visibilityObserver.observe(root);
    const scrollRoots = [
      root,
      root.closest<HTMLElement>(".tag-workbench"),
    ].filter((element): element is HTMLElement => element !== null);
    const scrollPositions = new Map(
      scrollRoots.map((element) => [element, element.scrollTop]),
    );
    function onScroll(event: Event) {
      const element = event.currentTarget as HTMLElement,
        previous = scrollPositions.get(element) ?? 0;
      scrollPositions.set(element, element.scrollTop);
      if (element.scrollTop > previous) {
        // A short page may leave the sentinel visible. Further scrolling can
        // request another page, but rendering alone cannot start a request loop.
        consumed = false;
        loadNextPage();
      }
    }
    scrollRoots.forEach((element) =>
      element.addEventListener("scroll", onScroll, { passive: true }),
    );
    stopPreviewObserver = () => {
      active = false;
      observer.disconnect();
      visibilityObserver.disconnect();
      scrollRoots.forEach((element) =>
        element.removeEventListener("scroll", onScroll),
      );
    };
  },
  { flush: "post" },
);
watch(
  w.selected,
  () => {
    if (resultsScroll.value) resultsScroll.value.scrollTop = 0;
  },
  { flush: "post" },
);
watch(
  [w.page, t.dictGroupId, t.dictQuery, t.dictOnlyUsed, t.dictSort],
  () => {
    const catalog = catalogScroll.value;
    if (!catalog) return;
    catalog.scrollTop = 0;
    if (getComputedStyle(catalog).overflowY === "visible") {
      catalog.scrollIntoView({ block: "start", inline: "nearest" });
    }
  },
  { flush: "post" },
);
const labels = computed(
  () =>
    new Map(
      t.filteredDictTags.value.map((tag) => [
        tag.tagEn,
        tag.tagZh || tag.tagEn,
      ]),
    ),
);
function label(tag: string) {
  return labels.value.get(tag) || w.labels.value.get(tag) || tag;
}
const src = (image: GalleryImage) =>
  imageUrl(image.thumbnailPath || image.path);
const imageColumns = computed(() => {
  const columns: GalleryImage[][] = Array.from(
    { length: previewColumnCount.value },
    () => [],
  );
  w.images.value.forEach((image, index) =>
    columns[index % columns.length].push(image),
  );
  return columns;
});
function maintainSearch(tag: string) {
  if (!w.selected.value.includes(tag)) w.toggle(tag);
  w.managing.value = false;
}
function coverStyle(tag: string) {
  const c = w.covers.value.get(tag);
  return {
    objectPosition: `${c?.x ?? 50}% ${c?.y ?? 50}%`,
    transform: `scale(${c?.zoom ?? 1})`,
    transformOrigin: `${c?.x ?? 50}% ${c?.y ?? 50}%`,
  };
}
onMounted(() => void w.init());
onUnmounted(() => {
  stopPreviewObserver();
  stopPreviewResize();
  w.suspend();
});
</script>
<template>
  <section v-if="w.managing.value" class="tag-maintenance-page">
    <button class="text-button" @click="w.managing.value = false">
      ← 返回标签工作台</button
    ><TagMaintenance @search="maintainSearch" />
  </section>
  <section
    v-else
    class="tag-workbench"
    :class="{ 'preview-closed': !w.previewOpen.value }"
    :data-narrow-panel="narrowPanel"
  >
    <header class="tag-workbench-heading">
      <h1>标签</h1>
      <div class="tag-find">
        <AppIcon name="search" :size="16" /><input
          v-model="t.dictQuery.value"
          aria-label="搜索标签"
          placeholder="搜索中文或英文标签"
        /><button
          v-if="t.dictQuery.value"
          class="icon-button"
          aria-label="清空标签搜索"
          @click="t.dictQuery.value = ''"
        >
          ×
        </button>
      </div>
      <button
        class="soft-button tag-my-tags"
        aria-label="我的标签"
        @click="w.managing.value = true"
      >
        <AppIcon name="tags" :size="16" /><span>我的标签</span>
      </button>
    </header>
    <nav class="tag-category-nav" aria-label="标签分组">
      <div class="tag-category-buttons">
        <button
          v-for="group in t.dictGroupItems.value.slice(0, 7)"
          :key="group.id"
          :aria-pressed="t.dictGroupId.value === group.id"
          @click="t.dictGroupId.value = group.id"
        >
          {{ group.zh }}</button
        ><SoftSelect
          v-if="t.dictGroupItems.value.length > 7"
          :model-value="
            t.dictGroupItems.value
              .slice(7)
              .some((g) => g.id === t.dictGroupId.value)
              ? t.dictGroupId.value
              : null
          "
          :options="
            t.dictGroupItems.value
              .slice(7)
              .map((g) => ({ value: g.id, label: g.zh }))
          "
          placeholder="更多分组"
          aria-label="更多标签分组"
          @update:model-value="t.dictGroupId.value = String($event)"
        />
      </div>
      <SoftSelect
        class="tag-sort"
        :model-value="t.dictSort.value"
        :options="[
          { value: 'count', label: '热门优先' },
          { value: 'zh', label: '中文名称' },
          { value: 'en', label: '英文名称' },
          { value: 'index', label: '字典顺序' },
        ]"
        aria-label="标签排序"
        @update:model-value="
          t.dictSort.value = String($event) as 'count' | 'zh' | 'en' | 'index'
        "
      />
    </nav>
    <div class="tag-condition-bar">
      <span class="tag-hint">{{
        w.selected.value.length ? "已选条件" : "组合条件"
      }}</span>
      <div class="tag-query-chips">
        <button
          v-for="tag in w.selected.value"
          :key="tag"
          class="tag-query-chip"
          :aria-label="`移除筛选：${label(tag)}`"
          @click="w.toggle(tag)"
        >
          {{ label(tag) }}<AppIcon name="close" :size="13" /></button
        ><span v-if="!w.selected.value.length" class="tag-hint"
          >点击下方标签加入，多个标签取交集</span
        >
      </div>
      <button
        v-if="w.selected.value.length"
        class="text-button"
        @click="w.selected.value = []"
      >
        清空</button
      ><button
        class="soft-button primary"
        :disabled="
          !w.selected.value.length || w.loading.value || !w.total.value
        "
        @click="emit('browse', [...w.selected.value])"
      >
        {{
          w.loading.value
            ? "正在查找…"
            : w.selected.value.length
              ? `浏览 ${w.total.value.toLocaleString()} 张图片`
              : "浏览组合结果"
        }}
      </button>
    </div>
    <p v-if="w.error.value" class="inline-error" role="alert">
      {{ w.error.value
      }}<button class="text-button" @click="w.error.value = ''">关闭</button>
    </p>
    <nav
      v-if="w.previewOpen.value"
      class="tag-panel-tabs"
      aria-label="标签与结果视图"
    >
      <button
        :aria-pressed="narrowPanel === 'tags'"
        @click="narrowPanel = 'tags'"
      >
        <AppIcon name="tags" :size="15" />标签条件
      </button>
      <button
        :aria-pressed="narrowPanel === 'results'"
        @click="narrowPanel = 'results'"
      >
        <AppIcon name="images" :size="15" />组合图片
        <small v-if="w.selected.value.length">{{
          w.total.value.toLocaleString()
        }}</small>
      </button>
    </nav>
    <div class="tag-workbench-body">
      <section class="tag-catalog">
        <div class="tag-catalog-tools">
          <h2>
            标签条件
            <small>{{
              t.filteredDictTags.value.length.toLocaleString()
            }}</small>
          </h2>
          <label
            ><input
              v-model="t.dictOnlyUsed.value"
              type="checkbox"
            />仅图库内</label
          ><button
            v-if="!w.previewOpen.value"
            class="text-button"
            @click="
              w.previewOpen.value = true;
              narrowPanel = 'results';
            "
          >
            展开图片预览
          </button>
        </div>
        <div ref="catalogScroll" class="tag-catalog-scroll">
          <div
            v-if="t.isTagManagerLoading.value && !w.visible.value.length"
            class="tag-workbench-empty"
          >
            正在读取标签…
          </div>
          <div v-else-if="!w.visible.value.length" class="tag-workbench-empty">
            没有匹配标签。试试其他名称或分组。
          </div>
          <div v-else class="tag-cover-grid">
            <article
              v-for="tag in w.visible.value"
              :key="tag.tagEn"
              class="tag-cover-card"
              :class="{ selected: w.selected.value.includes(tag.tagEn) }"
            >
              <button
                class="tag-card-main"
                :aria-pressed="w.selected.value.includes(tag.tagEn)"
                :aria-label="`${w.selected.value.includes(tag.tagEn) ? '移除' : '添加'}筛选：${tag.tagZh || tag.tagEn}`"
                :title="`${tag.tagZh || tag.tagEn}\n${tag.tagEn}`"
                @click="w.toggle(tag.tagEn)"
              >
                <div class="tag-card-image">
                  <img
                    v-if="w.covers.value.get(tag.tagEn)?.image"
                    :src="src(w.covers.value.get(tag.tagEn)!.image!)"
                    :style="coverStyle(tag.tagEn)"
                    alt=""
                    loading="lazy"
                    decoding="async"
                    @error="
                      ($event.target as HTMLImageElement).style.visibility =
                        'hidden'
                    "
                  /><span v-else class="tag-card-placeholder"
                    ><AppIcon name="images" :size="18" />{{
                      w.coverLoading.value ? "读取中" : "暂无封面"
                    }}</span
                  ><span
                    v-if="w.selected.value.includes(tag.tagEn)"
                    class="tag-selected-mark"
                    ><AppIcon name="check" :size="15"
                  /></span>
                </div>
                <div class="tag-card-caption">
                  <strong>{{ tag.tagZh || tag.tagEn }}</strong
                  ><span>{{ tag.tagEn }}</span>
                </div>
                <span class="tag-card-count"
                  >{{ tag.imageCount.toLocaleString() }}<small>张</small></span
                ></button
              ><button
                class="tag-favorite-toggle icon-button"
                :class="{ bookmarked: t.dictFavoriteTags.value.has(tag.tagEn) }"
                :aria-label="`${t.dictFavoriteTags.value.has(tag.tagEn) ? '取消收藏标签' : '收藏标签'}：${tag.tagZh || tag.tagEn}`"
                :aria-pressed="t.dictFavoriteTags.value.has(tag.tagEn)"
                :disabled="
                  w.favoriteLoading.value || w.favoriteBusy.value.has(tag.tagEn)
                "
                @click="w.favoriteTag(tag.tagEn)"
              >
                <AppIcon
                  name="heart"
                  :size="14"
                  :filled="t.dictFavoriteTags.value.has(tag.tagEn)"
                /></button
              ><button
                class="tag-cover-edit icon-button"
                :aria-label="`设置封面：${tag.tagZh || tag.tagEn}`"
                @click="
                  editor = { tag: tag.tagEn, label: tag.tagZh || tag.tagEn }
                "
              >
                <AppIcon name="more" :size="17" />
              </button>
            </article>
          </div>
        </div>
        <footer class="tag-catalog-pagination">
          <button
            class="text-button"
            :disabled="w.page.value === 0"
            @click="w.page.value--"
          >
            上一页</button
          ><span>{{ w.page.value + 1 }} / {{ w.pages.value }}</span
          ><button
            class="text-button"
            :disabled="w.page.value + 1 >= w.pages.value"
            @click="w.page.value++"
          >
            下一页
          </button>
        </footer>
      </section>
      <aside v-if="w.previewOpen.value" class="tag-results-panel">
        <header>
          <h2>
            组合图片
            <small v-if="w.selected.value.length"
              >{{ w.total.value.toLocaleString() }} 张</small
            >
          </h2>
          <button
            class="icon-button"
            aria-label="收起图片预览"
            @click="w.previewOpen.value = false"
          >
            <AppIcon name="collapse" :size="17" />
          </button>
        </header>
        <div ref="resultsScroll" class="tag-results-scroll">
          <p v-if="!w.selected.value.length" class="tag-workbench-empty">
            选择标签后，在这里查看关联图片。浏览分组不会改变已选条件。
          </p>
          <p
            v-else-if="w.loading.value"
            class="tag-workbench-empty"
            role="status"
          >
            正在查找组合结果…
          </p>
          <p
            v-else-if="w.previewError.value && !w.images.value.length"
            class="tag-workbench-empty"
            role="alert"
          >
            组合结果加载失败：{{ w.previewError.value }}
            <button class="text-button" @click="w.preview()">重试</button>
          </p>
          <p v-else-if="!w.images.value.length" class="tag-workbench-empty">
            暂无同时包含这些标签的图片。
          </p>
          <template v-else
            ><div
              class="tag-preview-masonry"
              :style="{
                gridTemplateColumns: `repeat(${previewColumnCount}, minmax(0, 1fr))`,
              }"
            >
              <div v-for="(column, index) in imageColumns" :key="index">
                <button
                  v-for="image in column"
                  :key="image.id"
                  :aria-label="`查看图片：${image.fileName}`"
                  @click="emit('open', image)"
                >
                  <img
                    :src="src(image)"
                    :alt="image.fileName"
                    :width="image.width"
                    :height="image.height"
                    loading="lazy"
                    decoding="async"
                  />
                </button>
              </div>
            </div>
            <div
              v-if="w.hasMore.value"
              ref="resultsEnd"
              class="tag-preview-more"
              :role="w.previewError.value ? 'alert' : 'status'"
              aria-live="polite"
            >
              <template v-if="w.previewError.value">
                加载失败：{{ w.previewError.value }}
                <button class="text-button" @click="w.preview(true)">
                  重试
                </button>
              </template>
              <span v-else>{{
                w.pageLoading.value
                  ? "正在加载更多图片…"
                  : "继续向下滚动，自动加载更多图片"
              }}</span>
            </div></template
          >
        </div>
      </aside>
    </div>
    <TagCoverEditor
      v-if="editor"
      :tag="editor.tag"
      :label="editor.label"
      :cover="w.covers.value.get(editor.tag)"
      @close="editor = null"
      @saved="w.setCover"
    />
  </section>
</template>
