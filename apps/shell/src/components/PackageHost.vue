<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { api, loadPackageModule, PKG_ORIGIN_KEY, type PackageModuleView } from "../api";
import type { MenuView } from "../generated";
import { mountSandboxed, type PackageApi, type SandboxHandle } from "../platform/sandbox";

const props = defineProps<{ node: MenuView }>();

const host = ref<HTMLElement | null>(null);
const error = ref<string | null>(null);
const loaded = ref(false);
let sandbox: SandboxHandle | null = null;

/**
 * 交给业务包的代理：只暴露 Rust 按清单下发的方法。
 *
 * 沙箱（`platform/sandbox.ts`）负责隔离；这里不判断业务权限，只转录能力列表
 * 和 Rust 给出的来源。交易码/模板是否放行由 Rust 侧再校验一次。
 */
const SHELL_METHODS: Record<string, string> = {
  print: "printVoucher",
  submit: "submitTransaction",
  log: "log",
};

const asText = (value: unknown): string => (typeof value === "string" ? value : "");
const asFields = (value: unknown): Record<string, string> =>
  value && typeof value === "object" && !Array.isArray(value)
    ? Object.fromEntries(
        Object.entries(value as Record<string, unknown>).map(([key, item]) => [key, asText(item)]),
      )
    : {};

function shellProxy(view: PackageModuleView): PackageApi {
  const allowed = new Set(
    view.capabilities.methods
      .map((method) => SHELL_METHODS[method])
      .filter((method): method is string => Boolean(method) && (method === "log" || !!view.origin)),
  );
  const handlers: Record<string, (...args: never[]) => unknown> = {
    printVoucher: (...args) => {
      const input = (args[0] ?? {}) as Record<string, unknown>;
      return api.printVoucher({
        template: asText(input.template),
        txCode: asText(input.txCode),
        reference: asText(input.reference) || null,
        // 宿主覆盖来源：包自己传的 __pkg_origin 不生效。
        fields: { ...asFields(input.fields), [PKG_ORIGIN_KEY]: view.origin },
      });
    },
    submitTransaction: (...args) => {
      const input = (args[0] ?? {}) as Record<string, unknown>;
      return api.submitTransaction({
        txCode: asText(input.txCode),
        idempotencyKey: asText(input.idempotencyKey),
        amountCents: typeof input.amountCents === "number" ? input.amountCents : 0,
        offlineCapable: input.offlineCapable === true,
        elements: { ...asFields(input.elements), [PKG_ORIGIN_KEY]: view.origin },
      });
    },
    log: (...args) => console.info("[pkg]", args[0]),
  };
  const reject = (method: string) => () =>
    Promise.reject(new Error(`业务包调用了未声明能力：${method}`));
  return new Proxy(
    {},
    {
      get(_target, prop) {
        const method = String(prop);
        return allowed.has(method) ? handlers[method] ?? reject(method) : reject(method);
      },
    },
  ) as PackageApi;
}

onMounted(async () => {
  try {
    const route = props.node.route ?? "";
    const view = await loadPackageModule(route);
    if (!host.value) return;
    sandbox = mountSandboxed(host.value, view.source, shellProxy(view));
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
