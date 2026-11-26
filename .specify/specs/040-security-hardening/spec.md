# 安全加固 · 行为规格

- 能力 ID：`040-security-hardening`
- 状态：draft（待人工评审；评审依据见 [docs/reviews/2026-11-25-security-review.md](../../docs/reviews/2026-11-25-security-review.md)）
- 关联契约：`contracts/ipc-commands.md`、`contracts/shell-api.md`、`contracts/lifecycle.md`

## 背景

2026-11-25 的安全评审发现：签名、TLS、配置验签、原子替换这些"点"都实现了，
但**在生产上没有连成链**——公钥是演示的、端点配置默认不要求签名且失败回退 mock、
锁屏不拦命令、业务包的能力声明没人执行、审计不能作为证据。
B1–B8 已覆盖联调、IAM、证书吊销、密钥保管，但没有覆盖上面五条；
其中任意一条被利用，其余控制都会一起失效。

做这件事的目的不是"再写一层校验"，而是让已有控制**在异常路径上也成立**：
配置坏了就拒绝启动，会话没了就拒绝执行，审计写不进去就拒绝成交。

**不做**（明确写出来，防止 AI 扩范围）：

- 不接真实 IAM、不做双因素、不实现服务端幂等与对账接口（B1/B2/B3）。
- 不做 mTLS/终端证书（需要先有证书签发体系，另立提案）。
- 不改 `contracts/**`、`docs/adr/**`、`docs/constitution.md`——契约变更走 `changes/` 提案。
- 不做安装包签名、SBOM、构建可复现（另立供应链提案）。
- 不实现更新执行器（B5）。

## 场景

### S-01 构建未注入公钥时拒绝启动

```gherkin
GIVEN 构建未提供 TK_PACK_PUBKEY
WHEN 主壳启动
THEN 启动失败并返回可读错误，不得使用内置演示公钥
AND 注入合法公钥后，菜单包与业务包都按该公钥验签
```

### S-02 端点配置无效时拒绝启动而不是回退 mock

```gherkin
GIVEN endpoints.json 存在但验签失败或解析失败，或生产构建根本没有配置
WHEN 主壳启动
THEN 启动失败并说明原因，不得回退内置 mock
AND 只有显式设置 TK_DEMO=1 才允许演示宿主，且启动信息标记为演示
```

### S-03 锁屏或未登录时资金类命令一律拒绝

```gherkin
GIVEN 会话已锁定（空闲、失焦、设备断开）或尚未登录
WHEN 调用 submit_transaction、print_voucher 或 request_reversal
THEN 返回可读错误且不触达主机，审计记录拒绝原因
AND 锁的判定来自 Rust 侧状态，不依赖前端是否调用 tick
```

### S-04 后台重放不越过锁屏

```gherkin
GIVEN 有挂起交易且会话已锁定或已登出
WHEN 后台重放线程到达 tick
THEN 不发起任何主机调用，并记录一次跳过审计
AND 解锁后按退避策略继续，不重置 attempts
```

### S-05 业务包只能提交自己声明的交易

```gherkin
GIVEN 业务包 P 的清单声明交易集合为 t1
WHEN P 通过沙箱调用 submitTransaction 提交 t2，或调用未携带包身份
THEN 拒绝并返回可读错误，审计记录 package_id 与拒绝原因
AND t1 的 offline_capable 取自包声明，而不是调用方传参
```

### S-06 冲正授权请求携带原交易要素

```gherkin
GIVEN 一笔被标记待冲正的交易，账本里有交易码、金额、账号掩码与时间
WHEN 发起冲正
THEN 授权请求包含上述要素，且要素取自账本而不是调用方传参
AND 授权结果与要素一起写入审计
```

### S-07 未登录不能发起冲正

```gherkin
GIVEN 当前没有有效会话
WHEN 调用 request_reversal
THEN 拒绝并说明需要有效会话
AND 不得因为发起人为 unknown 就绕过自批检查
```

### S-08 审计记录可检测篡改

```gherkin
GIVEN 审计表已有 N 条记录
WHEN 其中任意一条被直接 UPDATE 或 DELETE
THEN 校验能定位到第一条断链并返回可读错误
AND 正常追加的记录校验通过
```

### S-09 审计写失败时资金类操作不静默通过

```gherkin
GIVEN 审计库不可写
WHEN 提交交易或发起冲正
THEN 操作失败并返回可读错误
AND 不得只打印日志后继续成交
```

### S-10 待冲正清单重启后仍在

```gherkin
GIVEN 一笔交易被标记待冲正
WHEN 进程重启并重新登录
THEN tx_exceptions 仍返回该笔的完整幂等键与要素
AND 可用该键完成冲正
```

### S-11 对账与待冲正清单有可点击入口

```gherkin
GIVEN 主管已登录且当日有交易
WHEN 打开对账
THEN 可看到按状态聚合，以及带完整幂等键的待冲正清单
AND 清单项可直接发起冲正
```

### S-12 幂等与重放状态重启后不丢

```gherkin
GIVEN 幂等键 K 已用于金额 A 的交易
WHEN 进程重启后用同一 K 提交金额 B
THEN 拒绝
AND 已达到重放上限的交易重启后仍显示为已放弃，不重新开始计数
```

### S-13 传输层的 TLS 门与响应上限

```gherkin
GIVEN 端点配置为 tcp 且未启用 TLS 强制，或主机返回超大响应
WHEN 建立连接或读取响应
THEN 前者被拒绝并提示启用 TLS，后者在超限时中止并报错
```

### S-14 报文与凭证只含掩码

```gherkin
GIVEN 柜员输入完整卡号
WHEN 提交交易与打印凭证
THEN 发往主机的 elements 与凭证字段只含掩码
AND 账本里不出现完整卡号
```

### S-15 登录失败等安全事件进审计

```gherkin
GIVEN 登录失败
WHEN 连续多次失败
THEN 审计里能看到每次失败与累计次数
AND 配置变更、包加载、锁屏与解锁同样留痕
```

## 验收标准

- `npm run gate` 退出码为 0。
- `node tools/check-tasks.mjs` 通过，且每个场景至少被一个任务引用。
- 每个任务执行后 `run-task` 判 `PASS`；任何一条命令非零退出都算失败。
- S-03、S-04、S-07 必须带负向用例：锁屏/未登录时调用必须 `Err`，且主机调用次数为 0。
- S-01、S-02 必须带"坏配置"用例：缺失或非法输入时启动返回 `Err`，不得进入演示路径。

## 不变量

- 权限快照仍然不落盘；锁屏状态只存在于 Rust 侧。
- 前端仍然不判断权限，不新增 `menu_codes` 推导。
- 验签失败的单据绝不落盘、绝不执行。
- 任何新增的"降级"路径都必须显式、可审计、在界面上可见。
- 一次任务只改一个文件或一个 trait；单次新增超过 300 行退回重拆。
