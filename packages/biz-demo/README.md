# packages/biz-demo · 示例业务包（骨架）

**状态：skeleton。P2 才会被主壳加载。** 现在存在的意义是固定 `.bizpkg` 的目录形状，
让 Schema Agent / 生成器 / 门禁提前有东西可对齐，而不是等 P2 再发明格式。

## 目录

```
manifest.json       包 ID、版本、兼容范围、依赖、权限声明
menus.json          菜单增量（挂接到全量树的哪个节点）
permissions.json    本包声明的权限码（必须与服务端权限体系对齐）
routes.json         路由 → 前端模块
transactions.json   交易元数据：交易码、幂等策略、offline_capable、所需设备
ui/entry.ts         前端入口（ES module）
templates/          套打模板
```

## 已经确定的三条约束

1. **业务包不授予权限，只声明能力。** 服务端不下发对应权限码，这里的菜单自然被剪掉。
2. **菜单父子关系不是代码依赖。** 包之间不得互相 import，协作走主壳服务。
3. **资金类交易 `offlineCapable` 默认 false**，包不能自己改成 true。

## 待定（P2 决定，勿提前实现）

- 签名如何附加（`signature` 字段现在为 null）
- 包格式是 zip 还是 tar
- 加载器是否做模块级沙箱（当前决定：ES module + CSP + `__shellAPI` 白名单）
