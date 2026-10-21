<script setup lang="ts">
import { ref, watch } from "vue";
import { imageUrl } from "../../shared/ipc";
import AppIcon from "../../shared/ui/AppIcon.vue";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
import type { ImageDetail } from "../../shared/contracts/ImageDetail";
import { bytes } from "./format";
const props = defineProps<{
  member: DuplicateMember;
  reference?: boolean;
  detail?: ImageDetail;
  detailLoading?: boolean;
  detailError?: string;
  folderLabels?: string[];
}>();
const emit = defineEmits<{ compare: []; details: [] }>();
const failed = ref(false);
const detailsOpen = ref(false);
function details() {
  detailsOpen.value = !detailsOpen.value;
  if (detailsOpen.value) emit("details");
}
watch(
  () => props.member.image.path,
  () => (failed.value = false),
);
</script>
<template>
  <article
    class="duplicate-thumb"
    :class="{
      'duplicate-reference': reference,
    }"
    :data-image-id="member.image.id"
  >
    <button
      class="duplicate-open"
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
    </button>
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
    <button
      class="text-button compact duplicate-details-toggle"
      :aria-label="`查看标签与分类：${member.image.fileName}`"
      :aria-expanded="detailsOpen"
      @click="details"
    >
      <AppIcon
        v-if="member.image.isFavorite"
        name="heart"
        :filled="true"
        :size="13"
      />{{ detailsOpen ? "收起信息" : "标签与分类" }}
    </button>
    <div v-if="detailsOpen" class="duplicate-detail-summary">
      <p v-if="detailLoading">读取中…</p>
      <p v-else-if="detailError" class="error-text">{{ detailError }}</p>
      <template v-else-if="detail">
        <p>收藏：{{ detail.image.isFavorite ? "已收藏" : "未收藏" }}</p>
        <p>分类：{{ folderLabels?.join("、") || "未分类" }}</p>
        <p>
          手动标签：{{
            [
              ...detail.userTags.customTags.map((tag) => tag.tagText),
              ...detail.userTags.supplementTags.map(
                (tag) => tag.tagZh || tag.tagEn,
              ),
            ].join("、") || "无"
          }}
        </p>
      </template>
    </div>
  </article>
</template>
