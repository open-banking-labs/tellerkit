/**
 * 只存在于界面层的类型，**不属于跨语言契约**。
 *
 * 跨语言契约（菜单、权限快照、IPC DTO）全部由 Rust 生成到 `./generated/`，
 * 这里只放纯展示用的中间结构。
 */

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

/** 运行日志条目（只在界面内使用）。 */
export interface LogEntry {
  time: string;
  level: "info" | "ok" | "warn" | "error";
  message: string;
}
