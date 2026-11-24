# contracts/ · 唯一事实源

这里定义客户端与服务端、前端与 Rust 之间的全部接口、字段与错误码。

## 规则

1. **只读。** AI 不得为实现改契约（宪法第 1 条）。
2. 实现与契约不一致 → **改实现**，不改契约。
3. 契约确实缺失或矛盾 → 在 `changes/` 写提案，人工批准后由负责人改。
4. `fields.json` 是机器可读摘要，`tools/check-contracts.mjs` 拿它和代码双向比对。

## 文件

| 文件 | 约束什么 |
|---|---|
| `ipc-commands.md` | 前端可调用的命令清单与参数 |
| `shell-api.md` | 命令的入参/返回 DTO |
| `menu-biz-schema.md` | 菜单节点、可见菜单、权限快照、`.mtree` 头部 |
| `error-codes.md` | 错误分类与对外文案 |
| `lifecycle.md` | 启动 / 登录 / 登出 / 更新 / 回退的时序约束 |
| `fields.json` | 上述结构体的字段集合（门禁数据源） |

## 契约即代码（W2）

前端类型不再手写。Rust 结构体上的 `#[ts(export)]` 会在 `cargo test` 时导出到
`apps/shell/src/generated/`，命令名由 `generate_handler!` 生成 `CommandName`。

```bash
npm run bindings    # 重新生成 + 写 manifest.json
npm run gate        # 校验 fields.json ↔ Rust ↔ generated 三方一致
```

**禁止**新建 `apps/shell/src/contracts.ts`——手写契约文件会被门禁拦下（ADR-0007）。

## 怎么改

```bash
# 1. 写提案
$EDITOR changes/001-add-transaction-dto.md

# 2. 人工评审通过后，由负责人修改 contracts/ 与 fields.json

# 3. 跑门禁，此时实现还没跟上，应当红
npm run gate
```
