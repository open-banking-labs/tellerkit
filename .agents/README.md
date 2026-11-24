# .agents/ · Agent 定义

这里放**角色定义**，不是运行时配置。

- 现在：人工派发任务时，直接把对应预设作为系统提示词贴给 Agent。
- W4：接 DeepSeek Harness 的 subagent preset，把 `toolFilter` 映射到实际工具白名单。

## 为什么先有定义再接运行时

如果先接运行时，提示词会散落在会话里、无法评审、无法版本化。
先有文件，才能像契约一样评审和追溯——这也符合宪法第 8 条（决策留痕）。

## 预设

| 文件 | 角色 | 主战场 |
|---|---|---|
| `presets/schema-agent.md` | PageSchema / 业务包元数据生成 | 1.5 万交易入口的边际成本在这里 |
| `presets/engine-agent.md` | Rust/TS 实现与单测 | `packages/*`、`apps/*` |
| `presets/test-agent.md` | 契约测试生成 | 状态机、菜单合并、权限剪枝、授权评估 |
| `presets/review-agent.md` | 结构化评审摘要 | 不做最终批准 |

## 共同禁令

所有预设都继承根 `AGENTS.md` 的 8 条禁令。预设只能**加严**，不能放宽。
