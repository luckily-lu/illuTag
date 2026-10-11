<script setup lang="ts">
defineProps<{
  tags: { tagEn: string; tagZh?: string | null; confidence: number }[];
  label: string;
}>();
const emit = defineEmits<{ search: [tag: string] }>();
</script>
<template>
  <table class="tag-table" :aria-label="label">
    <colgroup>
      <col class="tag-zh-column" />
      <col class="tag-en-column" />
      <col class="tag-score-column" />
    </colgroup>
    <thead>
      <tr>
        <th scope="col">中文标签</th>
        <th scope="col">英文原名</th>
        <th scope="col" class="tag-score">置信度</th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="tag in tags" :key="tag.tagEn">
        <td>
          <button
            class="tag-search-label"
            :title="'搜索 ' + tag.tagEn"
            @click="emit('search', tag.tagEn)"
          >
            {{ tag.tagZh || tag.tagEn.replaceAll("_", " ") }}
          </button>
        </td>
        <td class="tag-english">{{ tag.tagEn }}</td>
        <td class="tag-score">{{ Math.round(tag.confidence * 100) }}%</td>
      </tr>
    </tbody>
  </table>
</template>
