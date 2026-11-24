# P2 收尾 · 行为规格

- 能力 ID：`022-p2-hardening`
- 状态：approved
- 关联契约：`contracts/ipc-commands.md`、`changes/006-audit-tail-command.md`

## 背景

P2 前两片解决了"包能装、能打开"。剩下的三件事都属于**合规底线**：
柜员做了什么要留痕、敏感字段不能随手复制、外设换了型号要拦住。
外加一件效率工具：新业务包不该靠复制 `biz-demo` 再改。

## 范围

**做**：审计日志（只增）、关键行为埋点、敏感字段剪贴板管控、外设型号白名单、
业务包脚手架、轨迹门禁。

**不做**：审计上报到服务端（P3）、日志加密（P3）、真实外设枚举（P3）。

## 场景

### S-01 关键行为必须留痕

```gherkin
GIVEN 柜员登录、打开菜单、提交交易、打印凭证、重放挂起
WHEN 每个动作完成
THEN 审计日志各追加一条
AND 每条包含 actor / action / target / result
```

### S-02 审计日志只增

```gherkin
GIVEN 已写入若干审计记录
WHEN 查看可用接口
THEN 不存在更新或删除方法
AND 重复写入只增加条数，不覆盖旧记录
```

### S-03 敏感字段禁止复制

```gherkin
GIVEN 输入框标记了 data-sensitive
WHEN 柜员按 Ctrl/Cmd+C 或剪切
THEN 复制被阻止
AND 界面给出提示（不允许静默失败）
```

### S-04 未登记型号的设备被拒绝

```gherkin
GIVEN 白名单只登记了 mock-printer
WHEN 检查四类设备
THEN 打印机通过
AND 其余三类返回"型号未授权"，错误里带型号名
```

### S-05 默认拒绝

```gherkin
GIVEN 策略为空（未配置）
WHEN 检查任意设备
THEN 一律拒绝
```

理由：白名单忘配置时必须**收紧**而不是放开——放开是事故，收紧只是不能用。

### S-06 一条命令生成业务包骨架

```gherkin
GIVEN 一个空目录
WHEN 运行 pkg-init
THEN 生成 manifest / menus / permissions / routes / transactions / ui/entry.js / README
AND 菜单 code 全部以包 ID 为前缀
```

### S-07 脚手架产物可直接打包并验签

```gherkin
GIVEN pkg-init 生成的目录
WHEN 用 pkg-build 打包、pkg-verify 验签
THEN 验签通过
AND 不需要手改任何文件
```

### S-08 轨迹门禁可复算

```gherkin
GIVEN 一条派发记录
WHEN 用当前任务清单重新编译提示词
THEN 输入指纹未变时，提示词 sha256 必须与记录一致（不一致 = 记录被改过）
AND 输入指纹变了（任务或预设被改）只降级为提示，不算篡改
AND 判定为 PASS 的记录不得有越界文件
AND 判定为 OUT_OF_SCOPE 的记录必须有越界文件（否则判定与证据矛盾）
```

## 验收标准

- `cargo test -p tellerkit-journal-sqlite`、`cargo test -p tellerkit-core device::` 全绿
- `pkg-init` + `pkg-build` + `pkg-verify` 三步在空目录上跑通
- `node tools/check-trajectory.mjs` 通过，且对篡改过的记录会红
- `npm run gate` 绿

## 不变量

1. 审计只增，不提供删除接口
2. 外设白名单默认拒绝
3. 剪贴板拦截必须有可见反馈
4. 轨迹门禁的结论必须来自复算，不是读记录里的自报字段
