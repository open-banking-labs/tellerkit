# IPC 命令契约

前端只能通过下列命令访问系统能力。**新增命令 = 改契约**，必须走 `changes/` 提案。

## 命令清单

| 命令 | 参数 | 返回 | 是否触网 |
|---|---|---|---|
| `boot` | — | `BootInfo` | 否（本地无包时才回退拉取） |
| `login` | `roleId: string` | `LoginResult` | 否（P0 为 mock） |
| `logout` | — | `void` | 否 |
| `current_menu` | — | `MenuView[]` | 否 |
| `check_update` | — | `UpdateReport` | 是（版本探测 + 按需下载） |
| `reload_from_disk` | — | `ReloadReport` | 验签失败时是 |
| `submit_transaction` | `txCode, idempotencyKey, amountCents, elements, offlineCapable` | `TxResult` | 是（走主机） |
| `print_voucher` | `template, txCode, reference?, fields` | `PrintReceipt` | 否（走设备） |
| `tx_pending_count` | — | `number` | 否 |
| `dev_set_offline` | `offline: boolean` | `DevState` | 否 |
| `dev_set_tampered` | `tampered: boolean` | `DevState` | 否 |
| `dev_bump_server_version` | — | `DevState` | 否 |
| `dev_corrupt_local` | — | `string` | 否 |
| `dev_reset_local` | — | `DevState` | 否 |

## 约束

1. **`boot` 在首屏路径上不得有网络等待**（本地有包的前提下）。违反 → 拒绝合并。
2. `login` 之前必须已完成 `boot`；未登录时 `current_menu` 返回错误而不是空数组。
3. `dev_*` 只影响 mock 宿主，不得改变真实业务逻辑；P1 接真后端时整组删除。
4. 所有命令失败时返回**中文可读**字符串，不返回 `Debug` 输出或堆栈。
5. 命令层不得 `unwrap()`/`expect()`（宪法附则，`check-ai-rules` 检查）。

## 参数命名

Tauri 会把 Rust 的 `snake_case` 参数名转成前端调用的 `camelCase`：
`role_id` → 前端传 `{ roleId }`。新增参数必须同步 `src/api.ts`。
