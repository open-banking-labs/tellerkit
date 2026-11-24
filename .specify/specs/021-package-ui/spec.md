# 业务包页面加载 · 行为规格

- 能力 ID：`021-package-ui`
- 状态：approved（P2.2）
- 关联契约：`contracts/menu-biz-schema.md`、`changes/005-package-ui-module.md`

## 背景

P2.1 能装包、合并菜单，但装完看不见——主壳不扫描目录，前端也没法加载包模块。

## 范围

**做**：主壳启动扫描 `<appdata>/packages/*.bizpkg`、菜单合并、模块源码下发、
前端 Blob 加载并挂载、`shell` 接口白名单、CSP 收紧。

**不做**：真正的进程级沙箱（P3 评估 Worker/iframe）、包热插拔、网络下发。

## 场景

### S-01 装包后菜单出现

```gherkin
GIVEN appdata/packages 下有一个签名正确的包
WHEN 启动客户端
THEN 左侧菜单出现该包的节点
AND 启动提示里写明"已加载业务包 biz.demo v0.1.0"
```

### S-02 坏包不影响启动

```gherkin
GIVEN 目录里混有两个包，其中一个被篡改
WHEN 启动
THEN 好包正常加载
AND 坏包被跳过并给出原因
```

### S-03 同名菜单保留主壳版本

```gherkin
GIVEN 包里有与主壳相同的 code
WHEN 合并
THEN 菜单里仍是主壳的标题
AND 启动提示记录该冲突
```

### S-04 打开包页面

```gherkin
GIVEN 某个菜单节点的 source 形如 pkg:biz.demo
WHEN 点击它
THEN 主壳返回该路由对应的模块源码
AND 前端用 Blob URL 加载并调用 mount(container, shell)
```

### S-05 包只能走白名单接口

```gherkin
GIVEN 业务包模块运行在主壳的 WebView 中
WHEN 它需要打印或提交交易
THEN 只能调用 shell 暴露的方法
AND 拿不到 window.__TAURI__，也读不到任意本地文件
```

## 验收标准

- `cargo test -p tellerkit-core pkg::` 全绿（含目录扫描）
- `cargo build -p tellerkit` 通过，`npm run build` 通过
- 人工：把 `biz-demo.bizpkg` 放进 appdata/packages 后启动，菜单多出"示例业务"

## 不变量

1. 坏包不阻断启动
2. 包不能覆盖主壳菜单
3. 前端只拿到模块源码，不拿到文件路径
4. 包模块没有真正的沙箱——这条必须写进安全边界文档，不能含糊
