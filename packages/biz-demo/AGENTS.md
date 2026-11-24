# packages/biz-demo/AGENTS.md

## 职责

示例业务包骨架，用于固定 `.bizpkg` 的目录形状。**P2 之前不参与构建、不被加载。**

## 允许依赖

无。包与包之间不得互相 import；需要协作时通过主壳提供的 `__shellAPI`。

## 禁止

- 直接访问文件系统、网络、外设（只能经主壳暴露的接口）
- 声明服务端不认识的权限码（会造成永不可见的僵尸菜单）
- 自己把 `offlineCapable` 改成 true（资金类必须服务端配置覆盖）
- 在 `ui/` 里引用其他业务包的代码
- 修改 `manifests.json` 的 `id`（改了等于换一个包）

## 契约

字段定义见 `contracts/menu-biz-schema.md` 与 `contracts/ipc-commands.md`。
新增字段 → 先写 `changes/` 提案。

## 验收

P2 起：`node tools/check-bizpkg.mjs --pkg biz-demo`（尚未实现）。
现在只需要保持 JSON 合法、id 与目录名一致。
