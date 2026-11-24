<script setup lang="ts">
import { computed } from "vue";

/**
 * 动态水印。
 *
 * 目的不是好看，是**溯源**：屏幕被拍照后能定位到工号与网点。
 * 所以信息必须包含网点、柜员、时间，且铺满整个可视区——
 * 只在角落放一行，裁掉就没了。
 */
const props = defineProps<{ tellerNo: string; outlet: string }>();

const stamp = computed(() => {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${props.outlet} ${props.tellerNo} ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(
    d.getDate(),
  )} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
});
</script>

<template>
  <div class="watermark" aria-hidden="true">
    <div v-for="i in 40" :key="i" class="watermark-cell">{{ stamp }}</div>
  </div>
</template>
