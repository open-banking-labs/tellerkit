# 业务包加载 · 行为规格

- 能力 ID：`020-bizpkg-loader`
- 状态：approved（P2 第一片）
- 关联契约：`contracts/menu-biz-schema.md`、`changes/004-bizpkg-container.md`

## 背景

机构新增一项业务，柜面就要多一组菜单和页面。若每次都要升级主壳，
产品就退化成"一家分行一个版本"。业务包要能独立演进，同时**不能**变成
"装个包就能改导航、自证来源、自带权限"的后门。

## 范围

**做**：`.bizpkg` 签名容器、清单与版本兼容、依赖拓扑、菜单合并与冲突策略、
包声明权限的服务端闭环验证。

**不做**（后续切片）：
- 不做前端模块加载（`import()` 与 `__shellAPI`）——P2.2
- 不做从网络下发——P2.3
- 不做热插拔（重启生效）

## 场景

### S-01 合法包加载并合并菜单

```gherkin
GIVEN 一个签名正确、版本兼容的包
WHEN 加载并合并
THEN 菜单节点进入全量树
AND 节点 source 被改写为 pkg:<包ID>
```

### S-02 篡改或换密钥的包被拒绝

```gherkin
GIVEN 包内任一字节被改动，或签名密钥不匹配
WHEN 加载
THEN 验签失败
AND 不产生任何菜单
```

### S-03 版本不兼容的包被跳过并说明原因

```gherkin
GIVEN 包声明 min_shell 高于当前主壳
WHEN 制定加载计划
THEN 该包不加载
AND 跳过原因写明"不兼容当前主壳 x.y.z"
```

### S-04 依赖按拓扑序加载，缺依赖或成环则整组跳过

```gherkin
GIVEN A 依赖 B，或 X 与 Y 互相依赖
WHEN 制定加载计划
THEN A 排在 B 之后加载
AND 缺依赖的包被跳过
AND 成环的包全部跳过（不允许加载一半）
```

### S-05 包不能劫持已有菜单，也不能越出自己的命名空间

```gherkin
GIVEN 包里出现与主壳同名的 code，或出现不属于 pkg ID 前缀的 code
WHEN 合并或加载
THEN 同名节点保留主壳版本，包节点被丢弃并记为冲突
AND 越界 code 在加载阶段就被拒绝
```

### S-06 包声明权限不等于获得权限

```gherkin
GIVEN 包声明了 permission 但服务端未授予该 code
WHEN 渲染菜单
THEN 该菜单不可见
AND 服务端授予后才出现
```

## 验收标准

- `cargo test -p tellerkit-core pkg::` 全绿，覆盖 S-01～S-06
- `mtree-tool pkg-build` + `pkg-verify` 能对一个真实目录完成打包与验签
- 冲突用例必须断言"最终树里是谁赢了"，不能只断言返回列表

## 不变量

1. 包不能覆盖主壳菜单（已存在的 code 优先）
2. 包不能自证来源（source 强制改写）
3. 包不能授予权限（授权只在服务端）
4. 加载失败不影响其他包与主壳功能
