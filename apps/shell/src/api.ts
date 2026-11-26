/**
 * 兼容层：组件继续 `import { api }`，实际拿到的是边界 ① 的 `ShellClient`。
 *
 * 新代码请直接 `import { client } from "./platform"`。
 * 本文件保留是为了让 20 多处调用点不必一次性改名；包模块信封的解码也放这里。
 */
import { client } from "./platform";

export { client as api, isTauri } from "./platform";
export type { ShellClient } from "./platform";

/** 宿主注入到包调用参数里的来源标记；包给的同名字段会被覆盖。 */
export const PKG_ORIGIN_KEY = "__pkg_origin";

export interface PackageCapabilities {
  /** `print` / `submit` / `log`，由 Rust 从签名清单推导。 */
  methods: string[];
  txCodes: string[];
  templates: string[];
}

export interface PackageModuleView {
  /** 包模块源码。 */
  source: string;
  /** `pkg:<id>`，审计 target 用；预览 mock 没有来源。 */
  origin: string;
  capabilities: PackageCapabilities;
}

const strings = (value: unknown): string[] =>
  Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];

/**
 * IPC 仍是 `string`：真包返回 JSON 信封；浏览器预览的 mock 直接返回模块源码。
 * 这里只做解码与形态归一化，不判定任何业务权限。
 */
export async function loadPackageModule(route: string): Promise<PackageModuleView> {
  const raw = await client.packageModule(route);
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed === "object" && parsed !== null) {
      const value = parsed as Record<string, unknown>;
      const caps = value.capabilities;
      if (typeof value.source === "string" && typeof caps === "object" && caps !== null) {
        const fields = caps as Record<string, unknown>;
        return {
          source: value.source,
          origin: typeof value.origin === "string" ? value.origin : "",
          capabilities: {
            methods: strings(fields.methods),
            txCodes: strings(fields.txCodes),
            templates: strings(fields.templates),
          },
        };
      }
    }
  } catch {
    // mock 返回的是模块源码，不是 JSON。
  }
  // 预览 mock 没有任何清单：只放开本地日志，资金与打印由 Rust 侧从源头拒绝。
  return {
    source: raw,
    origin: "",
    capabilities: { methods: ["log"], txCodes: [], templates: [] },
  };
}

export const ROLES = [
  { id: "teller", label: "柜员", teller: "T0231", hint: "账务处理 / 客户查询" },
  { id: "supervisor", label: "主管", teller: "T0101", hint: "授权中心 / 冲正 / 报表" },
  { id: "lobby", label: "大堂经理", teller: "T0450", hint: "客户画像 / 查询" },
] as const;
