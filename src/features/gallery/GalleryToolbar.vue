<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
defineProps<{
  scopeName: string;
  total: number;
  active: boolean;
  count: number;
  busy: boolean;
  scope: string;
}>();
const emit = defineEmits<{
  selectAll: [];
  select: [];
  done: [];
  favorite: [];
  dialog: [kind: string, title: string];
  trash: [];
  immersive: [];
  random: [];
}>();
const compact = ref(innerWidth < 720),
  more = ref(false);
const secondary = [
  { kind: "move", label: "移动", title: "移动分类归属" },
  { kind: "remove", label: "移除分类", title: "移除分类归属" },
  { kind: "tags", label: "标签", title: "批量添加标签" },
];
function resize() {
  compact.value = innerWidth < 720;
  if (!compact.value) more.value = false;
}
function dialog(kind: string, title: string) {
  more.value = false;
  emit("dialog", kind, title);
}
onMounted(() => window.addEventListener("resize", resize));
onUnmounted(() => window.removeEventListener("resize", resize));
</script>
<template>
  <div class="gallery-toolbar">
    <strong class="scope-title">{{ scopeName }}</strong
    ><span class="result-count">{{ total.toLocaleString() }} 张</span
    ><span class="spacer" />
    <template v-if="active">
      <span>已选 {{ count }}</span
      ><button class="text-button" @click="emit('selectAll')">
        选择当前已加载结果
      </button>
      <div class="batch-actions">
        <button
          class="soft-button compact"
          :disabled="busy"
          @click="emit('favorite')"
        >
          收藏
        </button>
        <button
          class="soft-button compact"
          :disabled="busy"
          @click="dialog('assign', '复制到虚拟文件夹')"
        >
          分类
        </button>
        <template v-if="!compact"
          ><button
            v-for="a in secondary"
            :key="a.kind"
            class="soft-button compact"
            :disabled="busy"
            @click="dialog(a.kind, a.title)"
          >
            {{ a.label }}
          </button></template
        >
        <button
          class="soft-button compact"
          :disabled="busy"
          @click="emit('trash')"
        >
          {{ scope === "trash" ? "还原" : "移入应用回收站" }}
        </button>
        <button
          v-if="scope === 'trash' && !compact"
          class="soft-button compact danger"
          :disabled="busy"
          @click="dialog('systemTrash', '移入系统回收站')"
        >
          系统回收站
        </button>
        <button
          v-if="compact"
          class="soft-button compact"
          :disabled="busy"
          @click="more = true"
        >
          更多操作
        </button>
      </div>
      <button class="text-button" @click="emit('done')">完成</button>
    </template>
    <template v-else
      ><button class="text-button" @click="emit('select')">选择</button
      ><button class="text-button" @click="emit('immersive')">
        沉浸浏览</button
      ><button
        v-if="scope === 'random'"
        class="text-button"
        @click="emit('random')"
      >
        换一组
      </button></template
    >
  </div>
  <SoftDialog v-if="more && active" title="更多批量操作" @close="more = false">
    <div class="button-row">
      <button
        v-for="a in secondary"
        :key="a.kind"
        class="soft-button"
        @click="dialog(a.kind, a.title)"
      >
        {{ a.label }}</button
      ><button
        v-if="scope === 'trash'"
        class="soft-button danger"
        @click="dialog('systemTrash', '移入系统回收站')"
      >
        移入系统回收站
      </button>
    </div>
  </SoftDialog>
</template>
