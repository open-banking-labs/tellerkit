<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { api } from "../api";
import type { MenuView } from "../generated";
import { mountSandboxed, type SandboxHandle } from "../platform/sandbox";

const props = defineProps<{ node: MenuView }>();

const host = ref<HTMLElement | null>(null);
const error = ref<string | null>(null);
const loaded = ref(false);
let sandbox: SandboxHandle | null = null;

/**
 * 交给业务包的能力白名单。
 *
 * 沙箱（`platform/sandbox.ts`）负责隔离：包拿不到父页面 DOM、拿不到
 * `localStorage`、发不出网络请求。这里只决定"允许它做什么"。
 */
const shellApi = {
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
    if (!host.value) return;
    sandbox = mountSandboxed(host.value, source, shellApi);
    await sandbox.done;
    loaded.value = true;
  } catch (err) {
    error.value = String(err);
  }
});

onBeforeUnmount(() => {
  sandbox?.dispose();
});
</script>

<template>
  <div class="card">
    <h2>业务包页面</h2>
    <p class="tx-hint">
      由 <code>{{ node.source }}</code> 提供 · 路由 <code>{{ node.route }}</code>
      · 运行在沙箱 iframe 中
    </p>
    <div ref="host" class="pkg-host"></div>
    <p v-if="error" class="tx-msg" style="color: var(--danger)">{{ error }}</p>
    <p v-else-if="!loaded" class="tx-hint">正在加载业务包模块…</p>
  </div>
</template>
