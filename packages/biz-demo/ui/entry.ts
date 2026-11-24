/**
 * 示例业务包的 UI 入口（骨架，P2 才被主壳加载）。
 *
 * 约定的加载方式：主壳按 routes.json 动态 import() 本模块，
 * 调用 mount(container, shell) 挂载。当前抛错是故意的——
 * 让"以为自己接好了"的人立刻看到未实现，而不是看到一片空白。
 */
export function mount(): never {
  throw new Error("业务包加载在 P2 实现：当前只是目录骨架，不参与构建");
}
