# Shell API · 数据契约

所有 DTO 的字段名与 Rust serde、`src/contracts.ts` 三方一致。
字段集合的机器可读版本见 `fields.json`，门禁 `check-contracts` 会双向比对。

## BootInfo

启动结果。前端用它在登录页展示"启动体检"。

| 字段 | 类型 | 说明 |
|---|---|---|
| `shell_version` | string | 主壳版本 |
| `pubkey_hex` | string | 内置公钥（演示用公开种子） |
| `appdata_dir` | string | 菜单包目录（排障用） |
| `offline` | boolean | 当前是否处于模拟断网 |
| `local_versions` | number[] | 本地已有的包版本，倒序 |
| `load` | LoadReport | 本次加载情况 |
| `notes` | string[] | 首次联网、验签失败回退等提示 |

## LoadReport

| 字段 | 类型 | 说明 |
|---|---|---|
| `menu_version` | number | 生效的菜单版本 |
| `source` | `"local"` \| `"network"` | 数据来源 |
| `menu_nodes` | number | 节点数 |
| `pack_bytes` | number | 包体积 |
| `load_ms` | number | 读文件 + 验签 + 反序列化耗时 |

## LoginResult

| 字段 | 类型 | 说明 |
|---|---|---|
| `snapshot` | PermSnapshot | 权限快照（不落盘） |
| `menu` | MenuView[] | **已剪枝**的可见菜单 |
| `filter_ms` | number | 递归剪枝耗时 |

## UpdateReport

| 字段 | 类型 | 说明 |
|---|---|---|
| `checked` | boolean | 是否真的探测到了服务端 |
| `local` | number \| null | 本地版本 |
| `latest` | number \| null | 服务端版本 |
| `updated` | boolean | 是否已静默安装新包 |
| `note` | string | 人能看懂的结果说明 |

## ReloadReport

| 字段 | 类型 | 说明 |
|---|---|---|
| `menu_version` | number | 重新加载后的版本 |
| `source` | `"local"` \| `"network"` | 本次来源 |
| `recovered` | boolean | 是否靠服务端救回 |
| `verify_failed` | boolean | 本地包是否验签失败 |
| `note` | string | 说明（含失败原因） |
| `menu` | MenuView[] \| null | 已登录时返回重新剪枝的结果 |

## DevState

| 字段 | 类型 | 说明 |
|---|---|---|
| `offline` | boolean | 模拟断网 |
| `tampered` | boolean | 服务端是否下发坏包 |
| `server_version` | number | mock 服务端版本 |
| `local_versions` | number[] | 本地版本列表 |
