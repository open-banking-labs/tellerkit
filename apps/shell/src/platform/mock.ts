/**
 * 边界 ② 的浏览器预览实现：同样的 `ShellClient`，数据来自 Rust 导出的 fixture。
 *
 * 它**不做任何业务判断**（状态机、剪枝、验签都在 Rust 里）。唯一的目的：
 * 没有 Tauri 运行时也能迭代界面样式，且看到的是客户端会渲染的同一份结果。
 */
import bootFixture from "../fixtures/boot.json";
import sessionsFixture from "../fixtures/sessions.json";
import type {
  BootInfo,
  DevState,
  LoginResult,
  MenuView,
  PrintReceipt,
  ReloadReport,
  SessionState,
  TxResult,
  UpdateReport,
} from "../generated";
import type { DevSwitches, ShellClient } from "./client";

const boot = bootFixture as unknown as BootInfo;
const sessions = sessionsFixture as unknown as Record<string, LoginResult>;

const devState: DevState = {
  offline: false,
  tampered: false,
  server_version: 7,
  local_versions: [7],
};

let current: LoginResult | null = null;

const unlocked: SessionState = { locked: false, reason: null, idle_secs: 0 };

const dev: DevSwitches = {
  async setOffline(offline) {
    devState.offline = offline;
    return { ...devState };
  },
  async setTampered(tampered) {
    devState.tampered = tampered;
    return { ...devState };
  },
  async bumpServerVersion() {
    devState.server_version += 1;
    return { ...devState };
  },
  async corruptLocal() {
    return "浏览器预览没有真实文件可篡改，请用 `npm run tauri:dev` 验证";
  },
  async resetLocal() {
    return { ...devState, local_versions: [] };
  },
};

export const mockClient: ShellClient = {
  async boot(): Promise<BootInfo> {
    return boot;
  },

  async login(roleId: string): Promise<LoginResult> {
    const session = sessions[roleId];
    if (!session) throw new Error(`未知角色：${roleId}`);
    current = session;
    return session;
  },

  async logout(): Promise<void> {
    current = null;
  },

  async currentMenu(): Promise<MenuView[]> {
    return current?.menu ?? [];
  },

  async checkUpdate(): Promise<UpdateReport> {
    return {
      checked: true,
      local: 7,
      latest: 7,
      updated: false,
      note: "浏览器预览模式：版本探测是静态值，真实行为请在 Tauri 客户端验证",
    };
  },

  async reloadFromDisk(): Promise<ReloadReport> {
    return {
      menu_version: 7,
      source: "local",
      recovered: false,
      verify_failed: false,
      note: "浏览器预览模式：不做真实验签，请用 `npm run tauri:dev` 验证",
      menu: current?.menu ?? null,
    };
  },

  /**
   * 交易桩：固定返回成功，**不实现状态机**。
   * 前端重算状态机必然与 Rust 分叉，宁可显示"预览结果"。
   */
  async submitTransaction(req): Promise<TxResult> {
    return {
      status: "committed",
      reference: `PREVIEW-${req.txCode}-${req.amountCents}`,
      message: "浏览器预览：这是模拟结果，真实状态机请在 Tauri 客户端验证",
      replayable: false,
    };
  },

  async printVoucher(): Promise<PrintReceipt> {
    return { job_id: "PREVIEW-PRINT-1", pages: 1 };
  },

  async txPendingCount(): Promise<number> {
    return 0;
  },

  async replayPending(): Promise<string> {
    return "浏览器预览：没有真实账本，重放需在 Tauri 客户端验证";
  },

  async auditTail(): Promise<string[]> {
    return ["浏览器预览：审计日志在 Tauri 客户端内查看"];
  },

  async sessionTouch(): Promise<SessionState> {
    return unlocked;
  },

  async sessionTick(): Promise<SessionState> {
    return unlocked;
  },

  async sessionBlur(): Promise<SessionState> {
    return unlocked;
  },

  async packageModule(): Promise<string> {
    throw new Error("浏览器预览不加载业务包模块：请在 Tauri 客户端验证");
  },

  dev,
};
