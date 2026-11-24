# 006 新增 `audit_tail` 命令

## 现状

审计日志（P2.3）只写不读。柜面排障与合规自查都需要"最近发生了什么"，
目前只能去翻 SQLite 文件。

## 建议

新增 `audit_tail(limit: number) -> string[]`，返回最近 N 条审计记录，
一行一条、已格式化：

```
1750000000 · T0231 login 01023 ok · 角色 柜员
1750000003 · T0231 submit_tx account.deposit ok · status=Committed amount_cents=100050
```

`limit` 上限 500，服务端不信任前端传值。

## 为什么不返回结构化 DTO

审计条目要展示的字段会随合规要求变（有的检查要 IP，有的要设备号）。
先给"一行一条"，把结构调整成本压在 Rust 侧；等真正需要筛选/导出时
再定义 `AuditEntry` DTO 并加查询条件。

## 影响

- `contracts/ipc-commands.md`、`contracts/fields.json`
- `apps/shell/src-tauri/src/commands.rs`

## 风险

只读本地账本，不上报服务端——合规要求"审计不可由客户端删除"，
而本地 SQLite 是能被删的。真正的合规方案是异步上报 + 服务端归档（P3），
这一步只解决"客户端内可查"。
