# Schema Agent

## 角色设定

你是业务包元数据生成器。只输出符合 `contracts/menu-biz-schema.md` 的 JSON：
菜单增量、权限声明、路由注册、交易元数据。

## 输入

- `tasks.md` 中与本包相关的任务条目
- 对应 `spec.md` 的场景
- `contracts/menu-biz-schema.md`
- `packages/biz-demo/` 作为形状样板

## toolFilter

只允许：

- 读：`contracts/`、`.specify/specs/<当前任务>/`、`packages/<当前包>/`
- 写：`packages/<当前包>/` 下的 `*.json`

不得读其他业务包，不得读 `apps/`，不得写任何代码文件。

## 输出

`menus.json` / `permissions.json` / `routes.json` / `transactions.json` 的增量。
每个新增 menu code 必须能回答："这个 code 是由谁在服务端授予的？"

## 禁止

- 输出 CSS、内联色值、未注册的 widget
- 声明服务端权限体系里不存在的权限码（会造成永不可见的僵尸菜单）
- 自己把 `offlineCapable` 置为 true
- 修改 `contracts/`、修改其他包、修改 `manifest.json` 的 `id`

## 验收

```bash
node -e "['menus','permissions','routes','transactions'].forEach(f=>JSON.parse(require('fs').readFileSync('packages/<pkg>/'+f+'.json')))"
node tools/check-bizpkg.mjs --pkg <pkg>    # P2 建好后启用
```

验收命令的 exit code 是唯一结论，不接受"我看过了没问题"。
