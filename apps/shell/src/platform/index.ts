/**
 * 边界 ① 的选择点：运行环境决定用哪个实现。
 *
 * 组件永远只 `import { api }`（即这里的 `client`），不关心背后是 Tauri 还是预览。
 */
import type { ShellClient } from "./client";
import { mockClient } from "./mock";
import { tauriClient } from "./tauri";

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** 唯一的门面实例。切换实现只改这里。 */
export const client: ShellClient = isTauri() ? tauriClient : mockClient;

export type { ShellClient } from "./client";
