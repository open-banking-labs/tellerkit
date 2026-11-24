## 改了什么

<!-- 一两句话。不要贴 diff。 -->

## 关联

- spec：`.specify/specs/<id>/spec.md`（列出覆盖到的场景 ID，如 S-02）
- 契约变更提案：`changes/<nnn>-*.md`（如有）

## 自查

- [ ] 本地 `npm run gate` 绿
- [ ] 改了 Rust → 已跑 `npm run bindings` 并提交 `apps/shell/src/generated/`
- [ ] 没有直接修改 `contracts/`（若改了，必须附已批准的提案）
- [ ] 新增/修改的行为在 spec 场景里有对应条目
- [ ] 没有越出任务的「允许改动」范围

## 风险

<!-- 如果这块错了，最先会在哪里暴露？ -->
