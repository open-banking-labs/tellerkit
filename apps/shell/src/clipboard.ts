/**
 * 剪贴板管控。
 *
 * 柜面场景里，账号、身份证号、金额被复制到记事本再发出去，
 * 是最常见也最难追的数据外泄路径。这里做的是**体验层阻断**：
 * 它挡不住截图、手抄、改代码，但能挡住"顺手复制"。
 *
 * 标记约定：任何需要管控的输入框加 `data-sensitive`；
 * 拦截时必须给出可见反馈——静默失败会让柜员反复尝试，反而更容易绕过。
 */

const BLOCK_EVENT = "shell:clipboard-blocked";

let installed = false;

function notify(field: string): void {
  window.dispatchEvent(
    new CustomEvent(BLOCK_EVENT, { detail: { field } }),
  );
}

function handle(event: ClipboardEvent): void {
  const target = event.target as HTMLElement | null;
  if (!target) return;
  const sensitive = target.closest("[data-sensitive]");
  if (!sensitive) return;

  event.preventDefault();
  const label = sensitive.getAttribute("data-sensitive-label") ?? "该字段";
  notify(label);
}

/**
 * 安装全局拦截。返回卸载函数，便于组件卸载时清理。
 *
 * 用捕获阶段：业务包或组件里的 `stopPropagation` 不该决定这件事。
 */
export function installClipboardGuard(): () => void {
  if (installed) return () => undefined;
  installed = true;

  document.addEventListener("copy", handle, true);
  document.addEventListener("cut", handle, true);

  return () => {
    document.removeEventListener("copy", handle, true);
    document.removeEventListener("cut", handle, true);
    installed = false;
  };
}

/** 供界面层订阅拦截事件，返回取消订阅函数。 */
export function onClipboardBlocked(handler: (field: string) => void): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<{ field: string }>).detail;
    handler(detail?.field ?? "敏感字段");
  };
  window.addEventListener(BLOCK_EVENT, listener);
  return () => window.removeEventListener(BLOCK_EVENT, listener);
}
