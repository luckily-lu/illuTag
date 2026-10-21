<script setup lang="ts">
import SoftSelect from "../../shared/ui/SoftSelect.vue";
import AppIcon from "../../shared/ui/AppIcon.vue";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
import { onMounted, ref } from "vue";
import { useTagManagement } from "../../composables/useTagManagement";
import { errorMessage, invoke } from "../../shared/ipc";
const error = ref(""),
  editName = ref(""),
  editing = ref<number | null>(null);
const pendingDelete = ref<{
  title: string;
  text: string;
  command: string;
  args: Record<string, unknown>;
} | null>(null);
async function confirmDelete() {
  const pending = pendingDelete.value;
  if (!pending) return;
  pendingDelete.value = null;
  await call(pending.command, pending.args);
}
const t = useTagManagement({
  formatError: errorMessage,
  setErrorText: (v) => (error.value = v),
});
const emit = defineEmits<{ search: [tag: string] }>();
onMounted(() => {
  t.tagManagerOpen.value = true;
  void t.reloadTagManagementState();
});
async function call(command: string, args: Record<string, unknown>) {
  try {
    await invoke(command, args);
    await t.reloadTagManagementState();
  } catch (e) {
    error.value = errorMessage(e);
  }
}
function drop(e: DragEvent, id: number | null) {
  const tag = e.dataTransfer?.getData("illutag/tag");
  if (tag)
    void call(
      id === null
        ? "unassign_user_tag_from_folder_command"
        : "assign_user_tag_to_folder_command",
      id === null ? { tagText: tag } : { tagText: tag, folderId: id },
    );
}
</script>
<template>
  <section class="page-content">
    <h1>我的标签</h1>
    <p>整理自定义标签，或从标签字典查找图片。</p>
    <div class="segmented">
      <button
        :class="{ active: t.tagManagerTab.value === 'custom' }"
        @click="t.tagManagerTab.value = 'custom'"
      >
        自定义标签</button
      ><button
        :class="{ active: t.tagManagerTab.value === 'dict' }"
        @click="
          t.tagManagerTab.value = 'dict';
          t.loadDictionaryBrowser();
        "
      >
        标签字典
      </button>
    </div>
    <p v-if="error" class="error-text">{{ error }}</p>
    <template v-if="t.tagManagerTab.value === 'custom'"
      ><div class="tag-manager">
        <aside class="panel">
          <button
            class="nav-row"
            @click="t.activeTagManagerFolderId.value = null"
            @dragover.prevent
            @drop="drop($event, null)"
          >
            未分类标签</button
          ><button
            v-for="f in t.tagManagerFolders.value"
            :key="f.id"
            class="nav-row"
            :class="{ active: t.activeTagManagerFolderId.value === f.id }"
            @click="t.activeTagManagerFolderId.value = f.id"
            @dragover.prevent
            @drop="drop($event, f.id)"
          >
            {{ f.name }} <small>{{ f.tags.length }}</small>
          </button>
          <form
            @submit.prevent="
              call('create_user_tag_folder_command', {
                name: t.newTagManagerFolderName.value,
              });
              t.newTagManagerFolderName.value = '';
            "
          >
            <input
              v-model="t.newTagManagerFolderName.value"
              aria-label="标签文件夹名"
              placeholder="新标签文件夹"
              required
            /><button class="soft-button">创建</button>
          </form>
        </aside>
        <div class="panel">
          <div
            v-if="t.activeTagManagerFolderId.value !== null"
            class="button-row"
          >
            <button
              class="soft-button"
              @click="
                editing = t.activeTagManagerFolderId.value;
                editName =
                  t.tagManagerFolders.value.find((f) => f.id === editing)
                    ?.name ?? '';
              "
            >
              重命名</button
            ><button
              class="soft-button danger"
              @click="
                pendingDelete = {
                  title: '删除标签文件夹',
                  text: '删除此标签分组，组内标签保留并回到未分类；分组结构无法撤销。',
                  command: 'delete_user_tag_folder_command',
                  args: { folderId: t.activeTagManagerFolderId.value },
                }
              "
            >
              删除标签文件夹
            </button>
          </div>
          <form
            v-if="editing !== null"
            @submit.prevent="
              call('rename_user_tag_folder_command', {
                folderId: editing,
                name: editName,
              });
              editing = null;
            "
          >
            <input v-model="editName" aria-label="新名称" /><button
              class="soft-button"
            >
              保存
            </button>
          </form>
          <div class="chips">
            <span
              v-for="tag in t.activeTagManagerFolderId.value === null
                ? t.tagManagerUnclassifiedTags.value
                : (t.tagManagerFolders.value.find(
                    (f) => f.id === t.activeTagManagerFolderId.value,
                  )?.tags ?? [])"
              :key="tag"
              class="chip"
              draggable="true"
              @dragstart="$event.dataTransfer?.setData('illutag/tag', tag)"
              ><button @click="emit('search', tag)">{{ tag }}</button
              ><button
                class="danger"
                aria-label="删除标签"
                @click="
                  pendingDelete = {
                    title: '删除自定义标签',
                    text: `删除「${tag}」以及它与图片的关联；此操作无法撤销，图片原文件保留。`,
                    command: 'delete_user_custom_tag_command',
                    args: { tagText: tag },
                  }
                "
              >
                <AppIcon name="close" :size="14" /></button
            ></span>
          </div>
          <form
            class="inline-form"
            @submit.prevent="
              call('create_user_custom_tag_command', {
                tagText: t.newTagManagerTagText.value,
              });
              t.newTagManagerTagText.value = '';
            "
          >
            <input
              v-model="t.newTagManagerTagText.value"
              aria-label="新标签"
              placeholder="新标签"
              required
            /><button class="soft-button">添加</button>
          </form>
        </div>
      </div></template
    >
    <template v-else
      ><div class="button-row">
        <input
          v-model="t.dictQuery.value"
          aria-label="搜索标签字典"
          placeholder="中文或英文标签"
        /><SoftSelect
          :model-value="t.dictGroupId.value"
          @update:model-value="t.dictGroupId.value = String($event)"
          :options="[
            { value: 'all', label: '全部分组' },
            ...t.dictGroups.value.map((g) => ({ value: g.id, label: g.zh })),
          ]"
          aria-label="字典分组"
        /><SoftSelect
          :model-value="t.dictSort.value"
          @update:model-value="
            t.dictSort.value = String($event) as 'count' | 'zh' | 'en' | 'index'
          "
          :options="[
            { value: 'count', label: '图片数量' },
            { value: 'zh', label: '中文' },
            { value: 'en', label: '英文' },
            { value: 'index', label: '原顺序' },
          ]"
          aria-label="字典排序"
        /><label
          ><input
            v-model="t.dictOnlyUsed.value"
            type="checkbox"
          />仅已使用</label
        >
      </div>
      <div class="dictionary-grid">
        <button
          v-for="tag in t.visibleDictTags.value"
          :key="tag.tagEn"
          class="detail-tag"
          @click="emit('search', tag.tagEn)"
        >
          <span>{{ tag.tagZh || tag.tagEn }}</span
          ><small>{{ tag.tagEn }} · {{ tag.imageCount }} 张</small>
        </button>
      </div>
      <button
        v-if="t.dictHasMore.value"
        class="soft-button"
        @click="t.loadMoreDictTags"
      >
        加载更多标签
      </button></template
    >
    <SoftDialog
      v-if="pendingDelete"
      :title="pendingDelete.title"
      @close="pendingDelete = null"
      ><p>{{ pendingDelete.text }}</p>
      <template #actions
        ><button class="text-button" @click="pendingDelete = null">取消</button
        ><button class="soft-button danger" @click="confirmDelete">
          确认删除
        </button></template
      ></SoftDialog
    >
  </section>
</template>
