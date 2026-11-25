/**
 * 兼容层：组件继续 `import { api }`，实际拿到的是边界 ① 的 `ShellClient`。
 *
 * 新代码请直接 `import { client } from "./platform"`。
 * 本文件保留是为了让 20 多处调用点不必一次性改名——**没有逻辑**。
 */
export { client as api, isTauri } from "./platform";
export type { ShellClient } from "./platform";

export const ROLES = [
  { id: "teller", label: "柜员", teller: "T0231", hint: "账务处理 / 客户查询" },
  { id: "supervisor", label: "主管", teller: "T0101", hint: "授权中心 / 冲正 / 报表" },
  { id: "lobby", label: "大堂经理", teller: "T0450", hint: "客户画像 / 查询" },
] as const;
