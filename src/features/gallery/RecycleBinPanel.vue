<script setup lang="ts">
import SoftDialog from "../../shared/ui/SoftDialog.vue";
defineProps<{
  open: boolean;
  title: string;
  scope: string;
  busy: boolean;
  error: string;
  action: "restore" | "systemTrash";
}>();
const emit = defineEmits<{ close: []; confirm: [] }>();
</script>
<template>
  <SoftDialog v-if="open" :title="title" @close="emit('close')">
    <p>{{ scope }}</p>
    <p v-if="action === 'systemTrash'">
      原文件将移入 Windows 回收站，之后可在 Windows
      回收站中恢复；此操作不保证立即释放磁盘空间。
    </p>
    <p v-else>图片会回到正常图库，并保留原有收藏、分类和标签。</p>
    <p v-if="error" class="error-text" role="alert">{{ error }}</p>
    <template #actions>
      <button class="text-button" :disabled="busy" @click="emit('close')">
        取消
      </button>
      <button
        class="soft-button"
        :class="{
          danger: action === 'systemTrash',
          primary: action === 'restore',
        }"
        :disabled="busy"
        @click="emit('confirm')"
      >
        {{ action === "restore" ? "确认还原" : "确认移入 Windows 回收站" }}
      </button>
    </template>
  </SoftDialog>
</template>
