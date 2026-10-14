<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
import type { DuplicateActionPreview } from "../../shared/contracts/DuplicateActionPreview";
import { bytes } from "./format";
const props = defineProps<{
  preview: DuplicateActionPreview;
  mode: string;
  advance: boolean;
  busy: boolean;
  error: string;
}>();
const emit = defineEmits<{
  close: [];
  confirm: [advance: boolean];
  retry: [];
}>();
const next = ref(props.advance),
  systemConfirmed = ref(false);
function key(event: KeyboardEvent) {
  if (
    event.isComposing ||
    event.key !== "Enter" ||
    event.target instanceof HTMLTextAreaElement ||
    document.querySelector(".select-popover")
  )
    return;
  event.preventDefault();
  event.stopImmediatePropagation();
  if (
    !props.busy &&
    props.preview.targetCount > 0 &&
    (!props.preview.requiresSystemConfirmation || systemConfirmed.value)
  )
    emit("confirm", next.value);
}
onMounted(() => window.addEventListener("keydown", key, true));
onUnmounted(() => window.removeEventListener("keydown", key, true));
</script>
<template>
  <SoftDialog
    :title="
      preview.action === 'systemTrash'
        ? '将原文件移入系统回收站'
        : '移入应用回收站'
    "
    @close="!busy && emit('close')"
  >
    <p class="duplicate-action-total">
      <strong>{{ preview.targetCount.toLocaleString() }} 张图片</strong> ·
      文件总大小 {{ bytes(preview.totalBytes) }}
    </p>
    <p v-if="mode === 'exactOthers'">
      本次处理此组除保留项以外的全部图片，包含尚未展开的成员。保留标记在所有分组中保护对应图片。
    </p>
    <p v-else-if="mode === 'autoExact'">
      仅整理字节完全相同的分组：每组按像素面积选一张保留，分辨率相同时按收藏、已有整理信息及固定顺序选择。近似候选不参与自动处理；你明确标记的保留项仍受保护。
    </p>
    <p v-else-if="preview.action !== 'systemTrash'">
      仅处理你明确选中的候选。未选中的图片不会受到影响。
    </p>
    <p>
      本次保护 {{ preview.protectedCount.toLocaleString() }} 项，跳过
      {{ preview.skippedCount.toLocaleString() }} 项。
    </p>
    <p>
      待处理项中：{{ preview.metadata.favoriteCount }} 张已收藏，{{
        preview.metadata.classifiedCount
      }}
      张有分类，{{ preview.metadata.customTaggedCount }} 张有自定义标签，{{
        preview.metadata.supplementTaggedCount
      }}
      张有补充标签。
    </p>
    <template v-if="preview.action === 'systemTrash'">
      <p class="duplicate-warning">
        原始文件将移入 Windows
        系统回收站，图库中的原记录及标签、分类等关联将移除。通过系统回收站恢复文件不会恢复这些图库关联；清空系统回收站后才释放磁盘空间。
      </p>
      <label class="duplicate-confirm-check"
        ><input
          v-model="systemConfirmed"
          type="checkbox"
        />我已了解原文件和图库记录的处理方式</label
      >
    </template>
    <p v-else>
      应用回收站只改变图库状态，原文件仍保留在磁盘上，不释放磁盘空间。完成后可以撤销本次软回收。
    </p>
    <details v-if="preview.sample.length">
      <summary>查看部分待处理文件（{{ preview.sample.length }} 项）</summary>
      <ul class="duplicate-preview-files">
        <li v-for="member in preview.sample" :key="member.image.id">
          <strong>{{ member.image.fileName }}</strong
          ><span>{{ member.image.id }}</span>
        </li>
      </ul>
    </details>
    <p
      v-for="(warning, index) in preview.warnings"
      :key="index"
      class="duplicate-warning"
    >
      {{ warning }}
    </p>
    <label
      v-if="preview.action !== 'systemTrash'"
      class="duplicate-confirm-check"
      ><input v-model="next" type="checkbox" />全部成功后进入下一组</label
    >
    <p v-if="error" class="error-message" role="alert">
      {{ error
      }}<button class="text-button" :disabled="busy" @click="emit('retry')">
        重新准备确认
      </button>
    </p>
    <template #actions
      ><button class="text-button" :disabled="busy" @click="emit('close')">
        取消</button
      ><button
        class="soft-button"
        :class="preview.action === 'systemTrash' ? 'danger' : 'primary'"
        :disabled="
          busy ||
          preview.targetCount === 0 ||
          (preview.requiresSystemConfirmation && !systemConfirmed)
        "
        @click="emit('confirm', next)"
      >
        {{
          busy
            ? "正在提交…"
            : preview.action === "systemTrash"
              ? "确认移入系统回收站"
              : next
                ? "确认处理并下一组"
                : "确认软回收"
        }}
      </button></template
    >
  </SoftDialog>
</template>
