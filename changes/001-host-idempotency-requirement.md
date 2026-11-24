# 001 契约需声明"主机按幂等键去重"

## 现状

`packages/core/src/tx.rs` 的 `TxHost` trait 只要求 `submit(&TxRequest) -> Result<String, HostError>`，
契约（`contracts/lifecycle.md`）没有写"服务端必须按幂等键去重"。

客户端的行为已经依赖这条前提：

- 瞬时失败（超时/不可达）后，柜员用**同一幂等键**重试；
- `replay_pending` 会把挂起请求原样重发，且**不查**客户端幂等索引。

两条路径都假定"服务端以幂等键去重"。如果服务端不去重，重放就会重复入账。

## 建议

在 `contracts/lifecycle.md` 增加一条不变量：

> 客户端重试与重放一律复用同一 `idempotencyKey`；服务端必须以该键做幂等，
> 同一键的多次提交只入账一次，并返回同一个业务流水号。

同时在 `contracts/ipc-commands.md` 或交易接口契约里注明：服务端未实现幂等时，
客户端不得启用离线挂起（`offline_capable` 必须保持 false）。

## 影响

- `contracts/lifecycle.md`：新增不变量
- P1 接真实主机时：接口文档需明确幂等语义
- 测试：主机 mock 需支持"同键重复提交返回同一流水号"（P1.4 落盘后补）

## 风险

这是**服务端责任**，客户端无法自证。若服务端不去重，客户端的所有重试策略都会变成
重复扣账的帮凶——所以这条必须在接主机之前写进契约，而不是事后补。
