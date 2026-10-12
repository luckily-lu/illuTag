<script setup lang="ts">
import AppIcon from "../../shared/ui/AppIcon.vue";
import { ref, watch, onUnmounted } from "vue";
import { useWorkspace } from "../../shared/workspace";
import { invoke } from "../../shared/ipc";
import type { KnownAutoTagSuggestion } from "../../shared/contracts/KnownAutoTagSuggestion";
import { useTagLabels } from "../tags/useTagLabels";
const props = defineProps<{ scopeLabel: string }>();
const { gallery } = useWorkspace();
const tagLabels = useTagLabels();
watch(
  () => gallery.query.value.tags.slice(),
  (tags) => {
    void tagLabels.resolve(tags).catch(() => {});
  },
  { immediate: true },
);
const mode = ref<"tags" | "file" | "nl">("tags"),
  tagInput = ref(""),
  suggestions = ref<KnownAutoTagSuggestion[]>([]),
  composing = ref(false);
const fileInput = ref(gallery.query.value.fileName),
  nlInput = ref(gallery.query.value.naturalLanguage);
watch(
  () => gallery.query.value.fileName,
  (v) => (fileInput.value = v),
);
watch(
  () => gallery.query.value.naturalLanguage,
  (v) => (nlInput.value = v),
);
onUnmounted(() => {
  clearTimeout(timer);
  serial++;
});
let serial = 0,
  timer: ReturnType<typeof setTimeout> | undefined;
watch(tagInput, (value) => {
  clearTimeout(timer);
  const token = ++serial;
  suggestions.value = [];
  if (!value.trim()) return;
  timer = setTimeout(async () => {
    try {
      const result = await invoke<KnownAutoTagSuggestion[]>(
        "suggest_known_auto_tags_command",
        { query: value, limit: 30, includeUserCustom: true },
      );
      if (token === serial) {
        suggestions.value = result;
        tagLabels.remember(result);
      }
    } catch {}
  }, 180);
});
function add(tag: string) {
  if (!gallery.query.value.tags.includes(tag))
    gallery.query.value.tags.push(tag);
  tagInput.value = "";
  suggestions.value = [];
  void gallery.search();
}
function submit() {
  if (composing.value) return;
  if (mode.value === "file") gallery.query.value.fileName = fileInput.value;
  if (mode.value === "nl") gallery.query.value.naturalLanguage = nlInput.value;
  if (mode.value === "tags" && tagInput.value.trim()) {
    add(tagInput.value.trim());
    return;
  }
  void gallery.search();
}
</script>
<template>
  <div class="search-region">
    <div class="search-main">
      <div class="soft-segment" aria-label="搜索方式">
        <button
          :class="{ 'is-active': mode === 'tags' }"
          @click="mode = 'tags'"
        >
          标签</button
        ><button
          :class="{ 'is-active': mode === 'file' }"
          @click="mode = 'file'"
        >
          文件名</button
        ><button :class="{ 'is-active': mode === 'nl' }" @click="mode = 'nl'">
          自然语言
        </button>
      </div>
      <div class="search-input-wrap">
        <input
          v-if="mode === 'tags'"
          v-model="tagInput"
          class="soft-input"
          placeholder="搜索中文或英文标签"
          aria-label="标签搜索"
          @compositionstart="composing = true"
          @compositionend="composing = false"
          @keydown.enter.prevent="
            !$event.isComposing && $event.keyCode !== 229 && submit()
          "
        /><input
          v-else-if="mode === 'file'"
          v-model="fileInput"
          class="soft-input"
          placeholder="输入文件名关键词"
          aria-label="文件名搜索"
          @compositionstart="composing = true"
          @compositionend="composing = false"
          @keydown.enter.prevent="
            !$event.isComposing && $event.keyCode !== 229 && submit()
          "
        /><input
          v-else
          v-model="nlInput"
          class="soft-input"
          placeholder="描述你想找的画面…"
          aria-label="自然语言搜索"
          @compositionstart="composing = true"
          @compositionend="composing = false"
          @keydown.enter.prevent="
            !$event.isComposing && $event.keyCode !== 229 && submit()
          "
        />
        <div
          v-if="mode === 'tags' && suggestions.length"
          class="search-suggestions"
          role="listbox"
        >
          <button
            v-for="tag in suggestions"
            :key="tag.tagEn"
            @click="add(tag.tagEn)"
          >
            <span
              >{{ tag.tagZh || tag.tagEn }}<small>{{ tag.tagEn }}</small></span
            ><small>{{ tag.imageCount.toLocaleString() }}</small>
          </button>
        </div>
      </div>
      <button
        class="soft-button primary search-submit"
        :disabled="gallery.loading.value"
        @click="submit"
      >
        <AppIcon name="search" :size="18" />搜索
      </button>
    </div>
    <div
      v-if="
        gallery.query.value.tags.length ||
        gallery.query.value.fileName ||
        gallery.query.value.naturalLanguage ||
        gallery.query.value.scope !== 'all' ||
        gallery.query.value.folderId !== null ||
        gallery.query.value.unclassifiedOnlyParentFolderId !== null
      "
      class="active-filters"
      aria-label="当前搜索条件"
    >
      <button
        v-if="
          gallery.query.value.scope !== 'all' ||
          gallery.query.value.folderId !== null ||
          gallery.query.value.unclassifiedOnlyParentFolderId !== null
        "
        class="tag-chip"
        @click="
          gallery.query.value.scope = 'all';
          gallery.query.value.folderId = null;
          gallery.query.value.unclassifiedOnlyParentFolderId = null;
          gallery.search();
        "
      >
        范围：{{ props.scopeLabel }} <AppIcon name="close" :size="14" /></button
      ><button
        v-for="tag in gallery.query.value.tags"
        :key="tag"
        class="tag-chip"
        :title="tag"
        :aria-label="'移除标签 ' + tagLabels.label(tag)"
        @click="
          gallery.query.value.tags = gallery.query.value.tags.filter(
            (t) => t !== tag,
          );
          gallery.search();
        "
      >
        {{ tagLabels.label(tag) }} <AppIcon name="close" :size="14" /></button
      ><button
        v-if="gallery.query.value.fileName"
        class="tag-chip"
        @click="
          gallery.query.value.fileName = '';
          gallery.search();
        "
      >
        文件名：{{ gallery.query.value.fileName }}
        <AppIcon name="close" :size="14" /></button
      ><button
        v-if="gallery.query.value.naturalLanguage"
        class="tag-chip"
        @click="
          gallery.query.value.naturalLanguage = '';
          gallery.search();
        "
      >
        描述：{{ gallery.query.value.naturalLanguage }}
        <AppIcon name="close" :size="14" /></button
      ><button class="text-button" @click="gallery.resetFilters">
        清空条件
      </button>
    </div>
  </div>
</template>
