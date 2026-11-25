/**
 * 边界 ②：Tauri 实现（唯一允许接触 `invoke` 的文件）。
 *
 * 命令名来自 `generate_handler!` 生成的 `CommandName`，所以拼错是**类型错误**，
 * 不是运行时的 "command not found"。
 *
 * 这里不做任何业务判断：错误原样上抛，由组件决定怎么展示。
 */
import { invoke } from "@tauri-apps/api/core";
import type { CommandName } from "../generated/commands";
import type {
  BootInfo,
  DevState,
  LoginResult,
  MenuView,
  PrintReceipt,
  ReloadReport,
  SessionState,
  TxException,
  TxResult,
  UpdateReport,
} from "../generated";
import type {
  DevSwitches,
  PrintVoucherRequest,
  ShellClient,
  SubmitTransactionRequest,
} from "./client";

function call<T>(cmd: CommandName, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(cmd, args);
}

const dev: DevSwitches = {
  setOffline: (offline) => call<DevState>("dev_set_offline", { offline }),
  setTampered: (tampered) => call<DevState>("dev_set_tampered", { tampered }),
  bumpServerVersion: () => call<DevState>("dev_bump_server_version"),
  corruptLocal: () => call<string>("dev_corrupt_local"),
  resetLocal: () => call<DevState>("dev_reset_local"),
};

export const tauriClient: ShellClient = {
  boot: () => call<BootInfo>("boot"),
  login: (roleId) => call<LoginResult>("login", { roleId }),
  logout: () => call<void>("logout"),
  currentMenu: () => call<MenuView[]>("current_menu"),
  checkUpdate: () => call<UpdateReport>("check_update"),
  reloadFromDisk: () => call<ReloadReport>("reload_from_disk"),

  submitTransaction: (req: SubmitTransactionRequest) =>
    call<TxResult>("submit_transaction", { ...req }),
  printVoucher: (req: PrintVoucherRequest) => call<PrintReceipt>("print_voucher", { ...req }),
  txPendingCount: () => call<number>("tx_pending_count"),
  txExceptions: () => call<TxException[]>("tx_exceptions"),
  requestReversal: (key, reason) => call<string>("request_reversal", { key, reason }),
  replayPending: () => call<string>("replay_pending"),

  auditTail: (limit = 20) => call<string[]>("audit_tail", { limit }),
  sessionTouch: () => call<SessionState>("session_touch"),
  sessionTick: () => call<SessionState>("session_tick"),
  sessionBlur: () => call<SessionState>("session_blur"),
  packageModule: (route) => call<string>("package_module", { route }),

  dev,
};
