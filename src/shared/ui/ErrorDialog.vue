<script setup lang="ts">
import { ref, useId, watch } from "vue";
import AppIcon from "./AppIcon.vue";
import SoftDialog from "./SoftDialog.vue";
import { useErrorDialog } from "./useErrorDialog";

const props = withDefaults(
  defineProps<{
    errors: readonly string[];
    title?: string;
    description?: string;
    retryLabel?: string;
    retrying?: boolean;
    retryDisabled?: boolean;
  }>(),
  {
    title: "操作未完成",
    description: "操作遇到问题，请查看详情后再继续。",
    retrying: false,
    retryDisabled: false,
  },
);
const emit = defineEmits<{ close: []; retry: [] }>();
const dialog = useErrorDialog(
  () => props.errors,
  () => props.retrying,
);
const expanded = ref(false);
const detailsId = useId();
watch(dialog.open, () => (expanded.value = false));
function close() {
  dialog.close();
  emit("close");
}
</script>

<template>
  <Teleport to="body">
    <SoftDialog
      v-if="dialog.open.value"
      class="error-dialog"
      :title="title"
      @close="close"
    >
      <p class="error-dialog-description">{{ description }}</p>
      <button
        class="text-button error-dialog-details-toggle"
        :aria-expanded="expanded"
        :aria-controls="detailsId"
        @click="expanded = !expanded"
      >
        技术详情<AppIcon name="down" :size="15" :class="{ expanded }" />
      </button>
      <div v-if="expanded" :id="detailsId" class="error-dialog-details">
        <pre v-for="message in dialog.messages.value" :key="message">{{
          message
        }}</pre>
      </div>
      <template #actions>
        <button
          v-if="retryLabel"
          class="text-button"
          :disabled="retrying || retryDisabled"
          @click="emit('retry')"
        >
          {{ retrying ? "刷新中…" : retryLabel }}
        </button>
        <button class="soft-button primary" @click="close">知道了</button>
      </template>
    </SoftDialog>
  </Teleport>
</template>

<style scoped>
.error-dialog {
  z-index: 300;
}
.error-dialog :deep(.soft-dialog) {
  max-width: 520px;
}
.error-dialog :deep(h2) {
  font-size: 18px;
  font-weight: 600;
}
.error-dialog-description {
  margin: 0 0 14px;
  color: var(--muted);
  line-height: 1.75;
}
.error-dialog-details-toggle {
  padding: 0;
  color: var(--muted);
  font-size: 12px;
}
.error-dialog-details-toggle .expanded {
  transform: rotate(180deg);
}
.error-dialog-details {
  margin-top: 12px;
  padding: 14px 16px;
  border-radius: 5px;
  background: var(--hover);
}
.error-dialog-details pre {
  margin: 0;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  font:
    12px/1.75 ui-monospace,
    Consolas,
    monospace;
  user-select: text;
}
.error-dialog-details pre + pre {
  margin-top: 12px;
}
</style>
