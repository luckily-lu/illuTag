<script setup lang="ts">
import SoftSelect from "./shared/ui/SoftSelect.vue";
import AppIcon from "./shared/ui/AppIcon.vue";
import { computed, onMounted, onUnmounted, ref } from "vue";
import { api, errorMessage, invoke } from "./shared/ipc";
import { useGallery } from "./features/gallery/useGallery";
import { useSelection } from "./features/gallery/useSelection";
import { provideWorkspace } from "./shared/workspace";
import { useFolders } from "./features/folders/useFolders";
import { useJobs } from "./features/jobs/useJobs";
import { usePreferences } from "./features/settings/usePreferences";
import FolderNav from "./features/folders/FolderNav.vue";
import GalleryToolbar from "./features/gallery/GalleryToolbar.vue";
import GalleryWall from "./features/gallery/GalleryWall.vue";
import SearchBar from "./features/search/SearchBar.vue";
import ImageViewer from "./features/viewer/ImageViewer.vue";
import TagManager from "./features/tags/TagManager.vue";
import TaskCenter from "./features/jobs/TaskCenter.vue";
import SettingsPage from "./features/settings/SettingsPage.vue";
import SoftDialog from "./shared/ui/SoftDialog.vue";
import type { GalleryImage } from "./shared/contracts/GalleryImage";
import type { LibraryFolder } from "./shared/contracts/LibraryFolder";
import type { ImageMutation } from "./shared/contracts/ImageMutation";
import type { UserFolderRuleCondition } from "./shared/contracts/UserFolderRuleCondition";
const gallery = useGallery(),
  selection = useSelection(),
  folders = useFolders(),
  preferences = usePreferences();
provideWorkspace({ gallery, selection });
const page = ref("gallery"),
  sources = ref<LibraryFolder[]>([]),
  drawer = ref(false),
  viewer = ref<GalleryImage | null>(null),
  error = ref(""),
  notice = ref(""),
  pinned = ref(false);
const jobs = useJobs(() => {
  void refresh();
  if (page.value === "gallery") void gallery.search();
});
const scopeName = computed(() =>
  gallery.query.value.unclassifiedOnlyParentFolderId !== null
    ? (folders.folders.value.find(
        (f) => f.id === gallery.query.value.unclassifiedOnlyParentFolderId,
      )?.name ?? "当前分类") + " · 仅本级待分类"
    : gallery.query.value.folderId !== null
      ? (folders.folders.value.find(
          (f) => f.id === gallery.query.value.folderId,
        )?.name ?? "文件夹")
      : ((
          {
            all: "全部图片",
            random: "随机浏览",
            favorites: "收藏",
            unclassified: "待分类",
            trash: "应用回收站",
          } as Record<string, string>
        )[gallery.query.value.scope] ?? "全部图片"),
);
const dialog = ref<null | {
    kind: string;
    title: string;
    id?: number;
    parentId?: number | null;
  }>(null),
  draft = ref(""),
  targetFolder = ref<number | null>(null),
  ruleConditions = ref<UserFolderRuleCondition[]>([]),
  busy = ref(false);
async function refresh() {
  try {
    const b = await api.navigation();
    folders.folders.value = b.userFolders;
    sources.value = b.folders;
    return b;
  } catch (e) {
    error.value = errorMessage(e);
  }
}
async function go(scope: string, folderId: number | null = null) {
  page.value = "gallery";
  drawer.value = false;
  gallery.query.value.scope = scope;
  gallery.query.value.folderId = folderId;
  gallery.query.value.unclassifiedOnlyParentFolderId = null;
  if (scope === "random") gallery.query.value.randomSeed = Date.now();
  selection.clear();
  await gallery.search();
}
async function mutate(
  action: string,
  ids = Array.from(selection.ids.value),
  extra: Partial<ImageMutation> = {},
) {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  notice.value = "";
  try {
    const result = await api.mutate({
      action,
      imageIds: ids,
      folderId: null,
      favorite: null,
      customTags: [],
      supplementTags: [],
      ...extra,
    });
    gallery.patch(result.images);
    if (viewer.value)
      viewer.value =
        result.images.find((i) => i.id === viewer.value?.id) ?? viewer.value;
    selection.done(result.successfulIds);
    if (result.failures.length)
      error.value = result.failures
        .map((f) => `${f.imageId.split(/[\\/]/).at(-1)}：${f.message}`)
        .join("\n");
    notice.value = `已完成 ${result.successfulIds.length} 项`;
    if (["trash", "restore", "systemTrash", "move", "remove"].includes(action))
      await gallery.search();
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}
function favorite(id: string) {
  const image = gallery.images.value.find((i) => i.id === id);
  if (image) void mutate("favorite", [id], { favorite: !image.isFavorite });
}
function openDialog(
  kind: string,
  title: string,
  id?: number,
  parentId?: number | null,
) {
  draft.value = "";
  targetFolder.value = null;
  dialog.value = { kind, title, id, parentId };
}
async function saveDialog() {
  const d = dialog.value;
  if (!d) return;
  try {
    if (d.kind === "create")
      await folders.create(draft.value.trim(), d.parentId ?? null);
    else if (d.kind === "rename" && d.id)
      await folders.rename(d.id, draft.value.trim());
    else if (d.kind === "delete" && d.id) {
      await folders.remove(d.id);
      if (gallery.query.value.folderId === d.id) await go("all");
    } else if (d.kind === "rule" && d.id)
      await invoke("save_user_folder_rule_command", {
        folderId: d.id,
        conditions: ruleConditions.value,
        applyNow: false,
      });
    else if (["assign", "move", "remove"].includes(d.kind)) {
      if (targetFolder.value === null) return;
      await mutate(d.kind, undefined, { folderId: targetFolder.value });
    } else if (d.kind === "tags")
      await mutate("addTags", undefined, {
        customTags: draft.value
          .split(/[,，\n]/)
          .map((t) => t.trim())
          .filter(Boolean),
      });
    else if (d.kind === "systemTrash") await mutate("systemTrash");
    dialog.value = null;
    await refresh();
  } catch (e) {
    error.value = errorMessage(e);
  }
}
async function rule(id: number) {
  try {
    const r = await invoke<{ conditions: UserFolderRuleCondition[] } | null>(
      "get_user_folder_rule_command",
      { folderId: id },
    );
    ruleConditions.value = r?.conditions ?? [];
    openDialog("rule", "自动分类规则", id);
  } catch (e) {
    error.value = errorMessage(e);
  }
}
async function start(kind: string, sourceFolderId: number | null = null) {
  page.value = "tasks";
  await jobs.start(kind, preferences.thumbnails.value, sourceFolderId);
}
function tagSearch(tag: string) {
  viewer.value = null;
  page.value = "gallery";
  if (!gallery.query.value.tags.includes(tag))
    gallery.query.value.tags.push(tag);
  void gallery.search();
}
async function unclassified(id: number) {
  page.value = "gallery";
  gallery.query.value.folderId = null;
  gallery.query.value.unclassifiedOnlyParentFolderId = id;
  await gallery.search();
}
const drag = ref<{
  id: string;
  startX: number;
  startY: number;
  x: number;
  y: number;
  active: boolean;
  target: number | null;
} | null>(null);
let dragTimer: ReturnType<typeof setTimeout> | undefined,
  suppressOpen = false;
function beginDrag(id: string, e: PointerEvent) {
  if (e.button !== 0 || selection.active.value) return;
  drag.value = {
    id,
    startX: e.clientX,
    startY: e.clientY,
    x: e.clientX,
    y: e.clientY,
    active: false,
    target: null,
  };
  dragTimer = setTimeout(() => {
    if (drag.value) drag.value.active = true;
  }, 160);
}
function dragMove(e: PointerEvent) {
  const d = drag.value;
  if (!d) return;
  d.x = e.clientX;
  d.y = e.clientY;
  if (!d.active && Math.hypot(e.clientX - d.startX, e.clientY - d.startY) > 8) {
    clearTimeout(dragTimer);
    drag.value = null;
    return;
  }
  if (d.active) {
    e.preventDefault();
    suppressOpen = true;
    if (e.clientX < 65) drawer.value = true;
    const target = document
      .elementFromPoint(e.clientX, e.clientY)
      ?.closest<HTMLElement>("[data-folder-id]");
    d.target = target ? Number(target.dataset.folderId) : null;
  }
}
function dragEnd() {
  clearTimeout(dragTimer);
  const d = drag.value;
  drag.value = null;
  if (d?.active && d.target !== null)
    void mutate("assign", [d.id], { folderId: d.target });
  if (d?.active) setTimeout(() => (suppressOpen = false), 0);
}
function openImage(image: GalleryImage) {
  if (!suppressOpen) viewer.value = image;
}
function key(e: KeyboardEvent) {
  if (e.key !== "Escape" || dialog.value || viewer.value) return;
  if (drawer.value) {
    drawer.value = false;
    e.preventDefault();
  } else if (selection.active.value) {
    selection.clear();
    e.preventDefault();
  } else if (preferences.immersive.value) preferences.immersive.value = false;
}
function windowAction(command: string) {
  void invoke(command).catch((e) => (error.value = errorMessage(e)));
}
onMounted(async () => {
  window.addEventListener("keydown", key);
  window.addEventListener("pointermove", dragMove);
  window.addEventListener("pointerup", dragEnd);
  window.addEventListener("pointercancel", dragEnd);
  try {
    const b = await api.bootstrap();
    folders.folders.value = b.userFolders;
    sources.value = b.folders;
    gallery.adopt(b.page);
  } catch (e) {
    error.value = errorMessage(e);
    await gallery.search();
  }
  console.info(
    `[startup-prof] first_screen_ms=${performance.now().toFixed(1)}`,
  );
});
onUnmounted(() => {
  clearTimeout(dragTimer);
  window.removeEventListener("keydown", key);
  window.removeEventListener("pointermove", dragMove);
  window.removeEventListener("pointerup", dragEnd);
  window.removeEventListener("pointercancel", dragEnd);
});
</script>
<template>
  <div
    class="app"
    :class="{
      'nav-collapsed': preferences.collapsed.value,
      immersive: preferences.immersive.value,
      'drawer-open': drawer,
      'auto-title': preferences.hideTitle.value,
    }"
  >
    <header class="titlebar">
      <div
        class="brand"
        @pointerdown.self="windowAction('window_start_dragging_command')"
      >
        illuTag <small>画廊工作台</small>
      </div>
      <span class="spacer" data-tauri-drag-region /><button
        class="window-control"
        :aria-pressed="pinned"
        aria-label="窗口置顶"
        @click="
          invoke<boolean>('window_toggle_always_on_top_command')
            .then((v) => (pinned = v))
            .catch((e) => (error = errorMessage(e)))
        "
      >
        <AppIcon name="pin" :size="16" /></button
      ><button
        class="window-control"
        aria-label="最小化"
        @click="windowAction('window_minimize_command')"
      >
        <AppIcon name="minimize" :size="16" /></button
      ><button
        class="window-control"
        aria-label="最大化或还原"
        @click="windowAction('window_toggle_maximize_command')"
      >
        <AppIcon name="square" :size="14" /></button
      ><button
        class="window-control close-control"
        aria-label="关闭窗口"
        @click="windowAction('window_close_command')"
      >
        <AppIcon name="close" :size="16" />
      </button>
    </header>
    <div class="workspace">
      <button
        v-if="drawer"
        class="drawer-scrim"
        aria-label="关闭导航"
        @click="drawer = false"
      /><FolderNav
        :page="page"
        :scope="gallery.query.value.scope"
        :folder-id="gallery.query.value.folderId"
        :rows="folders.rows.value"
        :expanded="folders.expanded.value"
        :drop-target="drag?.target ?? null"
        :running="jobs.rows.value.some((j) => j.state === 'running')"
        @go="go"
        @page="
          page = $event;
          drawer = false;
        "
        @collapse="
          preferences.collapsed.value = !preferences.collapsed.value;
          drawer = false;
        "
        @toggle="folders.toggle"
        @create="openDialog('create', '新建虚拟文件夹', undefined, $event)"
        @rename="
          openDialog('rename', '重命名文件夹', $event.id);
          draft = $event.name;
        "
        @remove="openDialog('delete', '删除虚拟文件夹', $event)"
        @rule="rule"
        @unclassified="unclassified"
        @reorder="
          (id, target) =>
            folders
              .reorder(id, target, 'before')
              .then(refresh)
              .catch((e) => (error = errorMessage(e)))
        "
      />
      <main class="main-workspace">
        <template v-if="page === 'gallery'"
          ><div class="search-area">
            <button
              class="soft-button nav-opener"
              aria-label="展开导航"
              @click="drawer = !drawer"
            >
              <AppIcon name="menu" /></button
            ><SearchBar :scope-label="scopeName" /><button
              v-if="preferences.immersive.value"
              class="soft-button"
              @click="preferences.immersive.value = false"
            >
              退出沉浸
            </button>
          </div>
          <GalleryToolbar
            :scope-name="scopeName"
            :total="gallery.total.value"
            :active="selection.active.value"
            :count="selection.count.value"
            :busy="busy"
            :scope="gallery.query.value.scope"
            @select-all="selection.all(gallery.images.value)"
            @select="selection.active.value = true"
            @done="selection.clear()"
            @favorite="mutate('favorite', undefined, { favorite: true })"
            @dialog="(kind, title) => openDialog(kind, title)"
            @trash="
              mutate(
                gallery.query.value.scope === 'trash' ? 'restore' : 'trash',
              )
            "
            @immersive="
              preferences.immersive.value = !preferences.immersive.value
            "
            @random="go('random')" />
          <p v-if="error" class="inline-error" role="alert">
            {{ error
            }}<button aria-label="关闭提示" @click="error = ''">
              <AppIcon name="close" :size="16" />
            </button>
          </p>
          <p v-else-if="notice" class="inline-notice" role="status">
            {{ notice
            }}<button aria-label="关闭提示" @click="notice = ''">
              <AppIcon name="close" :size="16" />
            </button>
          </p>
          <GalleryWall
            :thumbnails="preferences.thumbnails.value"
            @open="openImage"
            @favorite="favorite"
            @drag="beginDrag"
        /></template>
        <template v-else
          ><button
            class="soft-button page-nav-opener"
            aria-label="展开导航"
            @click="drawer = !drawer"
          >
            <AppIcon name="menu" /></button
          ><TaskCenter
            v-if="page === 'tasks'"
            :rows="jobs.rows.value"
            :error="jobs.error.value"
            :thumbnails="preferences.thumbnails.value"
            @start="start"
            @control="jobs.control" /><TagManager
            v-else-if="page === 'tags'"
            @search="tagSearch" /><SettingsPage
            v-else
            :sources="sources"
            v-model:thumbnails="preferences.thumbnails.value"
            v-model:hide-title="preferences.hideTitle.value"
            @refresh="refresh"
            @task="start"
        /></template>
      </main>
    </div>
    <ImageViewer
      v-if="viewer"
      :image="viewer"
      :images="gallery.images.value"
      :folders="folders.folders.value"
      @close="viewer = null"
      @change="viewer = $event"
      @favorite="favorite"
      @tag-search="tagSearch"
      @metadata-changed="gallery.search()"
    />
    <SoftDialog v-if="dialog" :title="dialog.title" @close="dialog = null"
      ><form id="workspace-dialog" @submit.prevent="saveDialog">
        <label v-if="['create', 'rename', 'tags'].includes(dialog.kind)"
          >{{ dialog.kind === "tags" ? "标签（逗号分隔）" : "名称"
          }}<input v-model="draft" required autofocus maxlength="200" /></label
        ><template
          v-else-if="['assign', 'move', 'remove'].includes(dialog.kind)"
          ><p>
            {{
              dialog.kind === "assign"
                ? "只增加虚拟分类归属，磁盘原图保持原位置。"
                : dialog.kind === "move"
                  ? "替换当前图片的分类归属，磁盘原图保持原位置。"
                  : "只移除指定虚拟分类归属。"
            }}
          </p>
          <SoftSelect
            :model-value="targetFolder"
            @update:model-value="
              targetFolder = typeof $event === 'number' ? $event : null
            "
            :options="
              folders.folders.value.map((f) => ({ value: f.id, label: f.name }))
            "
            placeholder="请选择文件夹"
            aria-label="目标虚拟文件夹" /></template
        ><template v-else-if="dialog.kind === 'rule'"
          ><p>规则自动作用于新增图片和标签变更。保存不会触发全库处理。</p>
          <div v-for="(c, i) in ruleConditions" :key="i" class="rule-row">
            <SoftSelect
              :model-value="c.logic"
              @update:model-value="
                c.logic = String($event) as UserFolderRuleCondition['logic']
              "
              :options="[
                { value: 'and', label: '并且' },
                { value: 'or', label: '或者' },
                { value: 'not', label: '排除' },
              ]"
              aria-label="条件逻辑"
            /><SoftSelect
              :model-value="c.source"
              @update:model-value="
                c.source = String($event) as UserFolderRuleCondition['source']
              "
              :options="[
                { value: 'danbooru', label: '模型标签' },
                { value: 'custom', label: '自定义标签' },
                { value: 'filename', label: '文件名' },
              ]"
              aria-label="条件来源"
            /><input
              v-model="c.keyword"
              aria-label="匹配内容"
              required
            /><button
              class="icon-button"
              type="button"
              aria-label="删除条件"
              @click="ruleConditions.splice(i, 1)"
            >
              <AppIcon name="close" :size="16" />
            </button>
          </div>
          <button
            class="soft-button"
            type="button"
            @click="
              ruleConditions.push({
                logic: 'and',
                source: 'danbooru',
                keyword: '',
              })
            "
          >
            添加条件
          </button></template
        >
        <p v-else-if="dialog.kind === 'delete'">
          删除分类及其子分类，磁盘原图保留。图片不会进入回收站。
        </p>
        <p v-else>
          所选原始文件将移入 Windows
          系统回收站，并删除图库索引。可通过系统回收站恢复原文件，之后重新扫描。
        </p>
      </form>
      <p v-if="error" class="error-text">{{ error }}</p>
      <template #actions
        ><button class="soft-button" @click="dialog = null">取消</button
        ><button
          class="soft-button accent"
          form="workspace-dialog"
          type="submit"
          :disabled="busy"
        >
          确认
        </button></template
      ></SoftDialog
    >
    <div
      v-if="drag?.active"
      class="drag-ghost"
      :style="{ left: drag.x + 14 + 'px', top: drag.y + 14 + 'px' }"
    >
      拖到虚拟文件夹分类
    </div>
  </div>
</template>
