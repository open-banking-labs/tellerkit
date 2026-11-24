# 离岗锁屏 · 行为规格

- 能力 ID：`032-session-lock`
- 状态：approved（P3.3）

## 背景

柜员离开座位、屏幕停着客户信息，是柜面最常见的信息泄露场景。
单一触发条件都不够：只看超时会漏掉"人走了但鼠标被碰到"，
只看失焦会漏掉"窗口没失焦但人已经走了"。

## 范围

**做**：多条件锁屏状态机（超时 / 失焦 / 外设断开 / 主动）、解锁后的状态恢复。

**不做**：身份验证方式（密码/指纹由上层）、水印（前端另一件事）、跨终端会话。

## 场景

### S-01 无操作超时锁定

```gherkin
GIVEN 策略为无操作 180 秒锁定
AND 最后一次操作在 t=0
WHEN 在 t=179 评估
THEN 未锁定
WHEN 在 t=180 评估
THEN 锁定，原因为 IdleTimeout
```

### S-02 评估只报告"新进入锁定"

```gherkin
GIVEN 会话已锁定
WHEN 再次评估
THEN 返回 None（不重复弹锁屏）
```

### S-03 锁定状态下操作不恢复会话

```gherkin
GIVEN 会话已锁定
WHEN 记录一次操作
THEN 仍处于锁定
AND 解锁后空闲计时从解锁时刻重新开始
```

### S-04 失焦按策略决定

```gherkin
GIVEN lock_on_blur = true
WHEN 窗口失焦
THEN 立即锁定

GIVEN lock_on_blur = false
WHEN 窗口失焦
THEN 不锁定
```

### S-05 外设断开不受策略影响

```gherkin
GIVEN lock_on_blur = false 且 idle_timeout_secs = 0（策略全关）
WHEN 外设断开
THEN 仍然锁定，原因为 DeviceDisconnected
```

### S-06 时钟倒退不 panic

```gherkin
GIVEN 最后一次操作在 t=1000
WHEN 在 t=900 计算空闲
THEN 返回 0，不 panic
```

## 验收标准

- `cargo test -p tellerkit-core session::` 全绿，覆盖 S-01～S-06
- 所有测试用显式时间戳，不读系统时钟

## 不变量

1. 锁屏判定是纯函数：时钟由调用方注入
2. 已锁定再评估不重复上报
3. 外设断开无条件锁定
