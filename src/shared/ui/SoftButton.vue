<script setup lang="ts">
withDefaults(
  defineProps<{ active?: boolean; tone?: "normal" | "accent" | "danger" }>(),
  { tone: "normal" },
);
</script>
<template>
  <button
    type="button"
    class="soft-button"
    :class="[tone, { 'is-active': active }]"
  >
    <slot />
  </button>
</template>
