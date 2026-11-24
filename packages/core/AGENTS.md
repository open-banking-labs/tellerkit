# packages/core/AGENTS.md

## 职责

菜单契约、权限剪枝、`.mtree` 编解码与验签、本地包仓库、启动与同步编排、mock 宿主。
**纯逻辑，无 GUI、无 Tauri 依赖**，必须能脱离客户端 `cargo test`。

## 允许依赖

`serde` · `rmp-serde` · `ed25519-dalek` · `sha2` · `hex` · `thiserror`

## 禁止

- 依赖 `tauri` 或 `tellerkit_lib`（会破坏可测试性，见宪法第 2 条）
- 读写应用数据目录以外的路径（目录由调用方传入）
- 在库内做权限判定之外的业务决策（交易成败、授权结论不在这里）
- 静默吞掉验签错误：失败必须返回 `PackError`，由上层决定回退

## 对外契约

`model.rs`（MenuNode/MenuTree）· `filter.rs`（MenuView）· `perm.rs`（PermSnapshot）
· `pack.rs`（PackError）· `store.rs`（StoreError）· `sync.rs`（BootError/UpdateOutcome）
以上公开类型与 `contracts/` 逐字段对齐，改动必须同步契约 + TS 类型 + 测试。

## 测试

```bash
cargo test -p tellerkit-core
```

新增公开函数必须有对应 `#[cfg(test)]` 用例；边界用例（父无权子有权、篡改、截断、回滚）不得删减。
