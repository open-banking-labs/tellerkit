# 004 `.bizpkg` 容器格式

## 现状

`contracts/menu-biz-schema.md` 只描述了菜单与权限，没有定义业务包的
二进制容器、字段名与签名方式。`packages/biz-demo/` 里的 JSON 是"看起来对"的形状。

## 建议

在契约中登记 `.bizpkg`：

```
[0..8)    magic "MTKPKG01"
[8..10)   format_version u16 LE = 1
[10..14)  payload_len u32 LE
[14..22)  预留（u64）
[22..26)  预留（u16 ×2）
[26..)    payload: MessagePack(BizPackage)
[末 64B]  Ed25519 over sha256(header || payload)
```

字段名**一律 snake_case**（与 Rust serde 一致）：
`min_shell` / `max_shell` / `offline_capable`，不用 camelCase。

## 为什么不复用 `.mtree` 的容器

菜单包是"纯结构"，业务包还带路由、交易元数据与依赖声明，生命周期也独立
（菜单包随主壳发版，业务包按月甚至按周迭代）。共用容器会逼着两者同版本演进。

## 影响

- `contracts/menu-biz-schema.md` 新增 `.bizpkg` 一节
- `packages/biz-demo/*.json` 字段名改为 snake_case，`routes.json` 与
  `transactions.json` 顶层改为 map / 数组（原来多包了一层）
- 打包工具 `mtree-tool pkg-build`

## 风险

包 ID 只要求两段（`biz.demo`）。三段的约束更"规范"，但会催生
`biz.demo.core` 这类凑数命名，实际防不住冲突——真正的隔离靠命名空间检查，
不靠段数。
