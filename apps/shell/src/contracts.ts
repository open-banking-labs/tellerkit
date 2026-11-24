/**
 * 前后端共享的数据契约。
 *
 * 这些类型必须与 `crates/tellerkit-core` 中的 serde 结构逐字段对齐；
 * Rust 侧改了字段，这里必须同步改（P1 会引入自动生成，P0 先手写）。
 */

export type NodeType = "group" | "page" | "action";

/** 已按权限剪枝的菜单节点。前端**不再**做任何权限判断。 */
export interface MenuView {
  code: string;
  title: string;
  title_key?: string | null;
  type: NodeType;
  route?: string | null;
  icon?: string | null;
  sort: number;
  /** 自身被授予权限；false 表示它只是"因为子孙有权而保留"的容器。 */
  direct: boolean;
  actions: string[];
  children: MenuView[];
}

export interface PermSnapshot {
  user_id: string;
  teller_no: string;
  outlet_code: string;
  perm_version: number;
  menu_codes: string[];
  action_codes: string[];
  shortcuts: Record<string, string>;
  role_label: string;
}

export interface LoadReport {
  menu_version: number;
  /** local | network */
  source: string;
  menu_nodes: number;
  pack_bytes: number;
  load_ms: number;
}

export interface BootInfo {
  shell_version: string;
  pubkey_hex: string;
  appdata_dir: string;
  offline: boolean;
  local_versions: number[];
  load: LoadReport;
  notes: string[];
}

export interface LoginResult {
  snapshot: PermSnapshot;
  menu: MenuView[];
  filter_ms: number;
}

export interface UpdateReport {
  checked: boolean;
  local: number | null;
  latest: number | null;
  updated: boolean;
  note: string;
}

export interface ReloadReport {
  menu_version: number;
  source: string;
  recovered: boolean;
  verify_failed: boolean;
  note: string;
  menu: MenuView[] | null;
}

export interface DevState {
  offline: boolean;
  tampered: boolean;
  server_version: number;
  local_versions: number[];
}

export interface LogEntry {
  time: string;
  level: "info" | "ok" | "warn" | "error";
  message: string;
}

/** 左侧菜单的一行（已扁平化，便于键盘漫游）。 */
export interface MenuRow {
  idx: number;
  code: string;
  title: string;
  depth: number;
  kind: "group" | "page";
  /** 自身被授权；false = 仅因子孙有权而保留的容器。 */
  direct: boolean;
  hasChildren: boolean;
  expanded: boolean;
  shortcut?: string;
}
