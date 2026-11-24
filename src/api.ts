/**
 * Rust 命令的薄封装。
 *
 * 前端**不做**任何业务计算：菜单过滤、验签、版本比对全部在 Rust 侧完成。
 * 这样只有一份真相，也避免"前端能改权限"的错觉。
 *
 * 不在 Tauri 运行时（例如直接用浏览器打开 Vite）会退到 `mockBackend`，
 * 使用的是同一套 Rust 逻辑导出的 fixture，方便前端单独迭代样式。
 */
import { invoke } from "@tauri-apps/api/core";
import type {
  BootInfo,
  DevState,
  LoginResult,
  MenuView,
  ReloadReport,
  UpdateReport,
} from "./contracts";

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

type MockModule = typeof import("./mockBackend");
let mockPromise: Promise<MockModule> | null = null;

/** 非 Tauri 环境才加载 mock（动态 import，生产包里不会被执行）。 */
async function mock(): Promise<MockModule | null> {
  if (isTauri()) return null;
  mockPromise ??= import("./mockBackend");
  return mockPromise;
}

export const api = {
  async boot(): Promise<BootInfo> {
    const m = await mock();
    return m ? m.mockApi.boot() : invoke<BootInfo>("boot");
  },

  async login(roleId: string): Promise<LoginResult> {
    const m = await mock();
    return m ? m.mockApi.login(roleId) : invoke<LoginResult>("login", { roleId });
  },

  async logout(): Promise<void> {
    const m = await mock();
    if (m) return m.mockApi.logout();
    return invoke<void>("logout");
  },

  async currentMenu(): Promise<MenuView[]> {
    const m = await mock();
    return m ? m.mockApi.currentMenu() : invoke<MenuView[]>("current_menu");
  },

  async checkUpdate(): Promise<UpdateReport> {
    const m = await mock();
    return m ? m.mockApi.checkUpdate() : invoke<UpdateReport>("check_update");
  },

  async reloadFromDisk(): Promise<ReloadReport> {
    const m = await mock();
    return m ? m.mockApi.reloadFromDisk() : invoke<ReloadReport>("reload_from_disk");
  },

  async devSetOffline(offline: boolean): Promise<DevState> {
    const m = await mock();
    return m ? m.mockApi.devSetOffline(offline) : invoke<DevState>("dev_set_offline", { offline });
  },

  async devSetTampered(tampered: boolean): Promise<DevState> {
    const m = await mock();
    return m
      ? m.mockApi.devSetTampered(tampered)
      : invoke<DevState>("dev_set_tampered", { tampered });
  },

  async devBumpServerVersion(): Promise<DevState> {
    const m = await mock();
    return m ? m.mockApi.devBumpServerVersion() : invoke<DevState>("dev_bump_server_version");
  },

  async devCorruptLocal(): Promise<string> {
    const m = await mock();
    return m ? m.mockApi.devCorruptLocal() : invoke<string>("dev_corrupt_local");
  },

  async devResetLocal(): Promise<DevState> {
    const m = await mock();
    return m ? m.mockApi.devResetLocal() : invoke<DevState>("dev_reset_local");
  },
};

export const ROLES = [
  { id: "teller", label: "柜员", teller: "T0231", hint: "账务处理 / 客户查询" },
  { id: "supervisor", label: "主管", teller: "T0101", hint: "授权中心 / 冲正 / 报表" },
  { id: "lobby", label: "大堂经理", teller: "T0450", hint: "客户画像 / 查询" },
] as const;
