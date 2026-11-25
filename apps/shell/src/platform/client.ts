/**
 * 边界 ①：桌面内部组件 ↔ 客户端门面。
 *
 * 组件只依赖这个接口，不知道 `invoke`、不知道命令名、不知道有没有 Tauri。
 * 换实现（Tauri / 浏览器预览 / 将来的 Web 版）不需要改任何组件。
 *
 * 契约来源：
 *   - 命令与 DTO 由 `contracts/ipc-commands.md` + ts-rs 生成（边界 ②）
 *   - 远端端点见 `contracts/remote-api.md`（边界 ③）
 */
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

export interface SubmitTransactionRequest {
  txCode: string;
  idempotencyKey: string;
  amountCents: number;
  elements: Record<string, string>;
  offlineCapable: boolean;
}

export interface PrintVoucherRequest {
  template: string;
  txCode: string;
  reference: string | null;
  fields: Record<string, string>;
}

/** 演示开关。只影响 mock 实现；真实实现里应恒为 no-op 或抛错。 */
export interface DevSwitches {
  setOffline(offline: boolean): Promise<DevState>;
  setTampered(tampered: boolean): Promise<DevState>;
  bumpServerVersion(): Promise<DevState>;
  corruptLocal(): Promise<string>;
  resetLocal(): Promise<DevState>;
}

export interface ShellClient {
  boot(): Promise<BootInfo>;
  login(roleId: string): Promise<LoginResult>;
  logout(): Promise<void>;
  currentMenu(): Promise<MenuView[]>;
  checkUpdate(): Promise<UpdateReport>;
  reloadFromDisk(): Promise<ReloadReport>;
  submitTransaction(req: SubmitTransactionRequest): Promise<TxResult>;
  printVoucher(req: PrintVoucherRequest): Promise<PrintReceipt>;
  txPendingCount(): Promise<number>;
  replayPending(): Promise<string>;
  auditTail(limit?: number): Promise<string[]>;
  sessionTouch(): Promise<SessionState>;
  sessionTick(): Promise<SessionState>;
  sessionBlur(): Promise<SessionState>;
  packageModule(route: string): Promise<string>;
  dev: DevSwitches;
}
