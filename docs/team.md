# 团队

AI 主力，人保留四个决策点。这不是"AI 帮忙写代码"，而是按团队分工组织产出。

## 角色

| 角色 | 谁 | 负责 | 不能做 |
|---|---|---|---|
| 项目负责人 | 人 | 写规格、批准契约变更、合并、发布决策 | 不写实现 |
| Lead Agent | AI | 读规格与契约、拆任务、派发、汇总、写 `changes/` 提案 | 不改契约、不合并 |
| Schema Agent | AI | 生成 `PageSchema` / 业务包元数据增量 | 不写 CSS、不硬编码色值、不用未注册组件 |
| Engine Agent | AI | 生成 Rust/TS 实现与单元测试 | 不跨层 import、不改契约、不越出包的目录 |
| Test Agent | AI | 生成 GIVEN/WHEN/THEN 用例，引用 spec 场景 ID | 不生成"只验证实现细节"的测试 |
| Review Agent | AI | 输出「可合并 / 需人工 / 拒绝」+ 理由 | 不做最终批准、不代替人评审 |

## 合并流程（main 受保护）

```
分支 → 提交 → 推送 → 开 PR → gate 绿 → squash merge → 删除分支
```

- `main` 禁止直推、禁止 force push（规则集 `main protection`）
- 必须过 `gate` 检查，且分支要 up-to-date
- 不需要人工 approve（单人项目自审会死锁），但 **PR 正文必须自检**
- AI 可以开 PR、可以读检查结果，**不能**点合并

## 人保留的四个决策点

1. **规格定稿**：`spec.md` 的场景与验收标准只能人改。
2. **契约变更**：`contracts/**` 与 `fields.json` 只能人改（AI 只能写 `changes/` 提案）。
3. **合并**：`npm run gate` 绿 ≠ 可合并，人工评审最后一道。
4. **发布与灰度**：哪个网点、哪个批次、什么时候生效。

## 每个角色的门禁

| 角色 | 产出 | 由谁裁决 |
|---|---|---|
| Schema Agent | 业务包 JSON | `check-ui-schema`（P2 建） |
| Engine Agent | `.rs` / `.ts` | `check-boundaries` + `cargo test` |
| Test Agent | 测试文件 | 覆盖率 + 人工抽查用例是否引用场景 |
| Review Agent | 评审摘要 | 人 |
| 全部 | — | `npm run gate`（唯一裁决入口） |

## AI 主力意味着什么

- 产出速度不再是瓶颈，**规格质量与门禁严格度**成为瓶颈。
- 所以规则必须写成可执行的门禁，而不是写在文档里等人自觉。
- 所以 `AGENTS.md` 要短——长了会被截断，AI 实际按被截断的规则行事。

## 协作节奏

- 一次一个任务，任务来自 `tasks.md`，每项带 `layer` 与 `acceptance_cmd`。
- 门禁失败**不自动重试**：一次失败 → 一次分析 → 一次人工决策。
- 新需求写进 TODO，不当场做。
