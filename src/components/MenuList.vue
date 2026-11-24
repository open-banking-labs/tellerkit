<script setup lang="ts">
import type { MenuRow } from "../contracts";

defineProps<{
  rows: MenuRow[];
  selectedCode: string | null;
  focusedIndex: number;
  emptyText: string;
}>();

const emit = defineEmits<{
  (e: "activate", idx: number): void;
  (e: "hover", idx: number): void;
}>();
</script>

<template>
  <div class="menu-list">
    <div v-if="rows.length === 0" class="menu-empty">{{ emptyText }}</div>
    <button
      v-for="row in rows"
      :key="row.code"
      :data-row="row.idx"
      class="menu-row"
      :class="{
        'is-group': row.kind === 'group',
        'container-only': !row.direct && row.hasChildren,
        selected: row.code === selectedCode,
        focused: row.idx === focusedIndex,
      }"
      :style="{ paddingLeft: 8 + row.depth * 14 + 'px' }"
      :title="row.direct ? row.code : row.code + '（仅容器，无直接访问权限）'"
      @click="emit('activate', row.idx)"
      @mouseenter="emit('hover', row.idx)"
    >
      <span class="caret">
        <template v-if="row.hasChildren">{{ row.expanded ? "▾" : "▸" }}</template>
        <template v-else>·</template>
      </span>
      <span class="label">{{ row.title }}</span>
      <span v-if="!row.direct" class="lock" title="无直接权限，仅因下有可见菜单而保留">◇</span>
      <span v-if="row.shortcut" class="key">{{ row.shortcut }}</span>
    </button>
  </div>
</template>
