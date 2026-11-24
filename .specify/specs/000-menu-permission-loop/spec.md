# 菜单权限闭环 · 行为规格

- 能力 ID：`000-menu-permission-loop`
- 状态：implemented（P0，补记式规格：先实现后补，用于给后续能力立格式）
- 关联契约：`contracts/menu-biz-schema.md`、`contracts/lifecycle.md`

## 背景

柜面客户端每次启动都拉菜单、解析、再按权限过滤，首屏被网络往返拖住。
目标是把菜单树放到本地签名包，权限差异用一份小快照在内存里剪枝。

不做会怎样：首屏延迟不可控，断网不可用，菜单包无法校验。

## 范围

**做**：本地 `.mtree` 包 + Ed25519 验签 + 全量树 + 权限快照剪枝 + 后台版本探测 + 切号清快照。

**不做**（明确边界，防止扩范围）：
- 不做按角色分包（坚持全量树 + 快照）
- 不做真实后端与真实登录
- 不做业务包、外设、离线队列、更新器
- 不做端侧 AI（只预留契约位置）

## 场景

### S-01 断网冷启动

```gherkin
GIVEN 本地已有一份合法 .mtree
AND 网络不可达
WHEN 启动客户端
THEN 菜单在 3 秒内可点
AND 数据来源显示"本地菜单包"
AND 启动过程中不发生任何菜单网络请求
```

### S-02 父节点无权、子节点有权

```gherkin
GIVEN 全量树中 account 是分组，account.withdraw 是其子节点
AND 权限快照只授予 account.withdraw
WHEN 渲染菜单
THEN account 作为容器出现（不可点击，标记 ◇）
AND account.withdraw 可点击
AND 同级的 account.deposit 不出现
```

### S-03 三角色三套菜单

```gherkin
GIVEN 三个角色分别是柜员、主管、大堂经理
WHEN 分别登录
THEN 三份可见菜单互不相同
AND 柜员看不到"授权中心"整棵子树
```

### S-04 篡改即失效并回退

```gherkin
GIVEN 本地 .mtree 被改动 1 个字节
WHEN 重新加载
THEN 验签失败
AND 自动从服务端拉取并覆盖
AND 日志出现"验签失败…已回退服务端"
```

### S-05 换号不残留

```gherkin
GIVEN 以柜员登录并看到 9 项菜单
WHEN 登出并以主管登录
THEN 页面不出现任何柜员专属菜单
AND 内存中的权限快照已被替换（不是合并）
```

### S-06 后台版本探测

```gherkin
GIVEN 服务端菜单版本高于本地
WHEN 首屏渲染完成 600ms 后
THEN 探测到版本差异
AND 静默下载并验签安装
AND 当前会话的权限快照不受影响
```

## 验收标准

- `cargo test -p tellerkit-core` 全绿，且包含 S-01/S-02/S-04/S-06 的对应用例
- `bash scripts/verify-p0.sh` 全绿（造包→验签→改字节→改版本号）
- 人工：按 `README.md` 的 P0 验收表逐条走，六条全过

## 不变量

1. 启动路径无菜单网络等待
2. 验签失败不落盘、不覆盖好包
3. 权限快照不落盘
4. 登出立刻清快照
5. 版本探测不阻塞首屏
