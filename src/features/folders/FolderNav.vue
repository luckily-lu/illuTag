<script setup lang="ts">
import AppIcon from "../../shared/ui/AppIcon.vue";
import type { IconName } from "../../shared/ui/AppIcon.vue";
import type { UserFolder } from "../../shared/contracts/UserFolder";
const navigation: Array<[string, string, IconName]> = [
  ["all", "全部", "images"],
  ["random", "随机", "shuffle"],
  ["favorites", "收藏", "heart"],
  ["unclassified", "待分类", "inbox"],
  ["trash", "回收站", "trash"],
];
const utilities: Array<[string, string, IconName]> = [
  ["tags", "标签", "tags"],
  ["tasks", "任务中心", "tasks"],
  ["settings", "设置", "settings"],
];
defineProps<{
  page: string;
  scope: string;
  folderId: number | null;
  rows: Array<UserFolder & { depth: number; hasChildren: boolean }>;
  expanded: Set<number>;
  dropTarget: number | null;
  running: boolean;
}>();
const emit = defineEmits<{
  go: [scope: string, id?: number];
  page: [page: string];
  collapse: [];
  toggle: [id: number];
  create: [parentId?: number];
  rename: [folder: UserFolder];
  remove: [id: number];
  rule: [id: number];
  unclassified: [id: number];
  reorder: [id: number, targetId: number];
}>();
</script>
<template>
  <nav class="sidebar" aria-label="图库导航">
    <div class="sidebar-heading">
      <strong>图库</strong
      ><button
        class="icon-button"
        aria-label="收起导航"
        @click="emit('collapse')"
      >
        <AppIcon name="collapse" :size="18" />
      </button>
    </div>
    <button
      v-for="[key, label, icon] in navigation"
      :key="key"
      class="nav-row"
      :class="{
        active: page === 'gallery' && scope === key && folderId === null,
      }"
      :title="label"
      @click="emit('go', key)"
    >
      <span class="nav-icon"><AppIcon :name="icon" /></span
      ><span class="nav-label">{{ label }}</span>
    </button>
    <div class="folder-heading">
      <span class="nav-label">虚拟文件夹</span
      ><button
        class="icon-button"
        aria-label="新建虚拟文件夹"
        @click="emit('create')"
      >
        <AppIcon name="plus" :size="18" />
      </button>
    </div>
    <div class="folder-tree">
      <div
        v-for="f in rows"
        :key="f.id"
        class="folder-row"
        :class="{
          active: page === 'gallery' && folderId === f.id,
          'drag-over': dropTarget === f.id,
        }"
        :style="{ paddingLeft: Math.min(f.depth, 8) * 14 + 'px' }"
        :data-folder-id="f.id"
        @dragover.prevent
        @drop.prevent="
          const id = $event.dataTransfer?.getData('illutag/folder');
          if (id && Number(id) !== f.id) emit('reorder', Number(id), f.id);
        "
      >
        <button
          class="folder-toggle"
          :aria-label="
            f.hasChildren ? '展开或收起 ' + f.name : '文件夹 ' + f.name
          "
          @click="
            f.hasChildren ? emit('toggle', f.id) : emit('go', 'all', f.id)
          "
        >
          <AppIcon
            :name="
              f.hasChildren ? (expanded.has(f.id) ? 'down' : 'right') : 'folder'
            "
            :size="18"
          /></button
        ><button
          class="folder-name"
          draggable="true"
          @dragstart="
            $event.dataTransfer?.setData('illutag/folder', String(f.id))
          "
          @click="emit('go', 'all', f.id)"
        >
          {{ f.name }}
        </button>
        <details class="folder-menu">
          <summary aria-label="文件夹操作">
            <AppIcon name="more" :size="18" />
          </summary>
          <div class="menu-popover">
            <button @click="emit('create', f.id)">新建子文件夹</button
            ><button @click="emit('rename', f)">重命名</button
            ><button @click="emit('rule', f.id)">自动分类规则</button
            ><button @click="emit('unclassified', f.id)">仅本级待分类</button
            ><button @click="emit('remove', f.id)">删除文件夹</button>
          </div>
        </details>
      </div>
    </div>
    <div class="sidebar-bottom">
      <button
        v-for="[id, label, icon] in utilities"
        :key="id"
        class="nav-row"
        :class="{ active: page === id }"
        :title="label"
        @click="emit('page', id)"
      >
        <span class="nav-icon"><AppIcon :name="icon" /></span
        ><span class="nav-label">{{ label }}</span
        ><small v-if="id === 'tasks' && running" class="task-dot">●</small>
      </button>
    </div>
  </nav>
</template>
