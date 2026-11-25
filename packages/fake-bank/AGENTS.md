# fake-bank/AGENTS.md

## 职责

测试替身：一个真实 TCP 服务端，让端到端用例覆盖分帧、连接与响应解析。

## 禁止

- 被产品代码依赖（只允许 `dev-dependencies` 与测试）
- 实现真实鉴权/持久化/验签
- 在网络协议之外添加业务规则

## 测试

```bash
cargo test -p tellerkit-fake-bank
```
