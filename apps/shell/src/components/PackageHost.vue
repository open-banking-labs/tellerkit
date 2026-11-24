<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { api } from "../api";
import type { MenuView } from "../generated";

const props = defineProps<{ node: MenuView }>();

const host = ref<HTMLElement | null>(null);
const error = ref<string | null>(null);
const loaded = ref(false);
let blobUrl: string | null = null;

/**
 * 交给业务包的接口白名单。
 *
 * 包模块跑在同一个 WebView 里，没有真正的沙箱——所以这里**只放行必要能力**，
 * 且全部经过 Rust 侧的命令。包拿不到 `window.__TAURI__`，
 * 也就无法绕过白名单直接调系统能力（这一点在 docs/security-boundary.md 有说明）。
 */
const shell = {
  printVoucher: (args: {
    template: string;
    txCode: string;
    reference: string | null;
    fields: Record<string, string>;
  }) => api.printVoucher(args),
  submitTransaction: (args: {
    txCode: string;
    idempotencyKey: string;
    amountCents: number;
    elements: Record<string, string>;
    offlineCapable: boolean;
  }) => api.submitTransaction(args),
  log: (message: string) => console.info("[pkg]", message),
};

onMounted(async () => {
  try {
    const route = props.node.route ?? "";
    const source = await api.packageModule(route);
    // Blob URL 加载：包是运行时外部文件，前端没法直接 import 本地路径
    blobUrl = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
    const module = (await import(/* @vite-ignore */ blobUrl)) as {
      mount?: (container: HTMLElement, shell: unknown) => void;
    };
    if (typeof module.mount !== "function") {
      throw new Error("业务包模块必须导出 mount(container, shell)");
    }
    if (host.value) {
      module.mount(host.value, Object.freeze(shell));
      loaded.value = true;
    }
  } catch (err) {
    error.value = String(err);
  }
});

onBeforeUnmount(() => {
  if (blobUrl) URL.revokeObjectURL(blobUrl);
});
</script>

<template>
  <div class="card">
    <h2>业务包页面</h2>
    <p class="tx-hint">
      由 <code>{{ node.source }}</code> 提供 · 路由 <code>{{ node.route }}</code>
    </p>
    <div ref="host" class="pkg-host"></div>
    <p v-if="error" class="tx-msg" style="color: var(--danger)">{{ error }}</p>
    <p v-else-if="!loaded" class="tx-hint">正在加载业务包模块…</p>
  </div>
</template>
