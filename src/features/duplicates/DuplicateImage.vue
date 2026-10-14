<script setup lang="ts">
import { ref, watch } from "vue";
import { imageUrl } from "../../shared/ipc";
import AppIcon from "../../shared/ui/AppIcon.vue";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
import { bytes } from "./format";
const props = defineProps<{ member: DuplicateMember; reference?: boolean }>();
const emit = defineEmits<{ compare: [] }>();
const failed = ref(false);
watch(
  () => props.member.image.path,
  () => (failed.value = false),
);
</script>
<template>
  <button
    class="duplicate-thumb"
    :class="{ 'duplicate-reference': reference }"
    @click="emit('compare')"
    :aria-label="`${reference ? '查看参考图' : '与参考图比对'}：${member.image.fileName}`"
  >
    <span class="duplicate-thumb-stage">
      <img
        v-if="!failed"
        :src="imageUrl(member.image.thumbnailPath || member.image.path)"
        alt=""
        loading="lazy"
        @error="failed = true"
      />
      <span v-else class="duplicate-image-error"
        ><AppIcon name="images" /><span>图片不可用</span></span
      >
      <span v-if="reference" class="duplicate-badge">参考图</span>
    </span>
    <strong class="truncate" :title="member.image.fileName">{{
      member.image.fileName
    }}</strong>
    <span class="duplicate-thumb-meta"
      >{{ member.image.width }} × {{ member.image.height }} ·
      {{ bytes(member.image.fileSize) }}</span
    >
    <span class="duplicate-thumb-path" :title="member.image.id">{{
      member.image.id
    }}</span>
    <span v-if="member.stale" class="duplicate-warning"
      >文件已变化，请更新结果</span
    >
  </button>
</template>
