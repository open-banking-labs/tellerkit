# 更新策略 · 行为规格

- 能力 ID：`031-update-policy`
- 状态：approved（P3.2）
- 关联：`docs/PLAN.md` P3 更新与运维

## 背景

柜面更新错一次就是全天停业。两条硬约束：**不在交易中要求重启**、
**灰度按网点而不是按终端随机**（否则同一网点两台机器版本不同，排障时说不清）。

## 范围

**做**：版本/通道/灰度判定、营业高峰处理、稳定散列。

**不做**：下载、签名校验、替换与回滚（由 tauri updater 负责）、增量补丁。

## 场景

### S-01 新版本且命中灰度

```gherkin
GIVEN 候选版本高于当前版本、通道一致、网点命中灰度
WHEN 决定更新
THEN 返回 DownloadAndNotify
```

### S-02 版本不更新

```gherkin
GIVEN 候选版本等于或低于当前版本
WHEN 决定更新
THEN 返回 Skip，理由说明"已是最新"
```

### S-03 通道不匹配

```gherkin
GIVEN 当前终端在 Stable 通道，候选只在 Canary
WHEN 决定更新
THEN 返回 Skip
```

### S-04 未命中灰度

```gherkin
GIVEN 灰度比例为 10，网点散列落在 10 以外
WHEN 决定更新
THEN 返回 Skip，理由说明"未命中灰度"
```

### S-05 营业高峰只下载不提示

```gherkin
GIVEN 本机时间处于营业高峰（9:00-17:00）
WHEN 命中更新
THEN 返回 DownloadOnly
AND 理由说明高峰不打扰
```

### S-06 散列稳定

```gherkin
GIVEN 同一网点与同一版本
WHEN 连续调用散列
THEN 结果相同
AND 不同网点尽量分散（不要求均匀，要求确定）
```

## 验收标准

- `cargo test -p tellerkit-core update::` 全绿，覆盖 S-01～S-06
- 散列稳定性用例至少覆盖 20 个网点，且结果分布不是常量

## 不变量

1. 纯函数：同输入同输出，不读系统时间
2. 高峰时段绝不提示重启用
3. 灰度按网点散列，不按终端随机
