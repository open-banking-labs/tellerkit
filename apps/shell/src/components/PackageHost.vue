<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { api } from "../api";
import type { MenuView } from "../generated";
import {
  mountSandboxed,
  parsePackageIdentity,
  PKG_IDENTITY_CODE_KEY,
  PKG_IDENTITY_SOURCE_KEY,
  type PackageIdentity,
  type PackageTxArgs,
  type PackageVoucherArgs,
  type SandboxHandle,
} from "../platform/sandbox";

const props = defineProps<{ node: MenuView }>();

const host = ref<HTMLElement | null>(null);
const error = ref<string | null>(null);
const loaded = ref(false);
let sandbox: SandboxHandle | null = null;

/**
 * 交给业务包的能力白名单。
 *
 * 沙箱（`platform/sandbox.ts`）负责隔离；包能调用哪些交易码/设备由 Rust 侧
 * 按清单裁决。这里仅把 Rust 注入的`包身份`转录到既有 IPC 参数 map，
 * 不自己决定能否提交。
 */
const identityFields = (
  fields: Record<string, string> | undefined,
  identity: PackageIdentity,
): Record<string, string> => ({
  ...(fields && typeof fields === "object" ? fields : {}),
  [PKG_IDENTITY_SOURCE_KEY]: identity.source,
  [PKG_IDENTITY_CODE_KEY]: identity.code,
});

const shellApi = {
  printVoucher: (args: PackageVoucherArgs | undefined, identity: PackageIdentity) =>
    api.printVoucher({
      template: args?.template ?? "",
      txCode: args?.txCode ?? "",
      reference: args?.reference ?? null,
      fields: identityFields(args?.fields, identity),
    }),
  submitTransaction: (args: PackageTxArgs | undefined, identity: PackageIdentity) =>
    api.submitTransaction({
      txCode: args?.txCode ?? "",
      idempotencyKey: args?.idempotencyKey ?? "",
      amountCents: args?.amountCents ?? 0,
      elements: identityFields(args?.elements, identity),
      offlineCapable: args?.offlineCapable ?? false,
    }),
  log: (message: string) => console.info("[pkg]", message),
};

onMounted(async () => {
  try {
    const route = props.node.route ?? "";
    const source = await api.packageModule(route);
    if (!host.value) return;
    // 真实包身份来自 Rust 写在模块首行的注释；预览 mock 没有注释时退回菜单节点。
    const identity: PackageIdentity = parsePackageIdentity(source) ?? {
      source: props.node.source,
      code: props.node.code,
    };
    sandbox = mountSandboxed(host.value, source, shellApi, identity);
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
