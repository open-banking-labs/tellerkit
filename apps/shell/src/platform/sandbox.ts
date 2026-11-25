/**
 * 业务包沙箱：把包模块放进 `sandbox="allow-scripts"` 的 iframe。
 *
 * ## 为什么必须沙箱
 *
 * 包是**签名但第三方**的代码。以前它和主壳跑在同一个 WebView 里，
 * 能直接读 `window`、`document`、`localStorage`——前面所有的权限剪枝、
 * 水印、剪贴板管控都能被一个恶意包绕过。
 *
 * ## 现在挡住了什么
 *
 * | 能力 | 结果 |
 * |---|---|
 * | 访问父页面 DOM / `window.parent` | ❌ 跨源，抛 SecurityError |
 * | `localStorage` / `cookie` | ❌ 不透明源，访问即抛错 |
 * | `fetch` / `XMLHttpRequest` | ❌ CSP `connect-src 'none'` |
 * | 调系统命令（`invoke`） | ❌ 拿不到 `__TAURI__`，只能走下面的白名单 |
 *
 * ## 仍然挡不住的
 *
 * 同进程内存、侧信道、以及**白名单里的能力本身被滥用**（比如疯狂打印）。
 * 真正的进程级隔离要 Tauri 多 webview 或独立进程，记在 security-boundary §4.5。
 */

/** 包可以调用的能力。新增一项都要问：这个能力被滥用会怎样？ */
export const PACKAGE_API_METHODS = ["printVoucher", "submitTransaction", "log"] as const;

export type PackageApiMethod = (typeof PACKAGE_API_METHODS)[number];

export type PackageApi = Record<PackageApiMethod, (...args: never[]) => unknown>;

interface CallMessage {
  __pkg: "call";
  id: number;
  method: string;
  args: unknown[];
}

interface InitMessage {
  __pkg: "init";
  source: string;
}

interface ReadyMessage {
  __pkg: "ready";
}

interface ErrorMessage {
  __pkg: "error";
  message: string;
}

type Inbound = CallMessage | ReadyMessage | ErrorMessage;

/**
 * 白名单分发（纯函数，便于单测与评审时逐条核对）。
 *
 * 未登记的方法一律拒绝并返回错误，**不静默忽略**——包作者需要知道调用被拒。
 */
export function dispatchCall(
  api: PackageApi,
  method: string,
  args: unknown[],
): Promise<unknown> {
  if (!(PACKAGE_API_METHODS as readonly string[]).includes(method)) {
    return Promise.reject(new Error(`业务包调用了未授权能力：${method}`));
  }
  const fn = api[method as PackageApiMethod] as (...a: unknown[]) => unknown;
  try {
    return Promise.resolve(fn(...args));
  } catch (err) {
    return Promise.reject(err);
  }
}

/** iframe 内的引导脚本。它只做三件事：收源码、加载、转发调用。 */
const BOOTSTRAP = `
<script>
(function () {
  var send = function (msg) { parent.postMessage(msg, "*"); };
  var pending = new Map();
  var nextId = 1;

  var shell = new Proxy({}, {
    get: function (_t, prop) {
      if (typeof prop !== "string") return undefined;
      return function () {
        var args = Array.prototype.slice.call(arguments);
        var id = nextId++;
        return new Promise(function (resolve, reject) {
          pending.set(id, { resolve: resolve, reject: reject });
          send({ __pkg: "call", id: id, method: prop, args: args });
        });
      };
    }
  });

  window.addEventListener("message", function (event) {
    var data = event.data || {};
    if (data.__pkg === "result") {
      var slot = pending.get(data.id);
      if (!slot) return;
      pending.delete(data.id);
      if (data.ok) slot.resolve(data.value);
      else slot.reject(new Error(data.error || "调用失败"));
      return;
    }
    if (data.__pkg === "init") {
      import(URL.createObjectURL(new Blob([data.source], { type: "text/javascript" })))
        .then(function (mod) {
          if (typeof mod.mount !== "function") {
            throw new Error("业务包模块必须导出 mount(container, shell)");
          }
          var root = document.createElement("div");
          root.className = "pkg-root";
          document.body.appendChild(root);
          mod.mount(root, shell);
          send({ __pkg: "ready" });
        })
        .catch(function (err) {
          send({ __pkg: "error", message: String(err && err.message ? err.message : err) });
        });
    }
  });
})();
<\/script>
`;

/**
 * 沙箱文档。CSP 是第二道锁：
 * 即使包代码拿到 DOM，也发不出网络请求。
 */
export function sandboxDoc(): string {
  return `<!doctype html>
<html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline' blob:; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; form-action 'none'; base-uri 'none'">
<style>html,body{margin:0;padding:0;font:inherit;color:inherit;background:transparent}</style>
</head><body>${BOOTSTRAP}</body></html>`;
}

export interface SandboxHandle {
  dispose: () => void;
  done: Promise<void>;
}

/**
 * 把包源码挂进 sandbox iframe。
 *
 * `done` 在包 mount 完成时 resolve；加载失败则 reject。
 */
export function mountSandboxed(
  container: HTMLElement,
  source: string,
  api: PackageApi,
): SandboxHandle {
  const iframe = document.createElement("iframe");
  iframe.setAttribute("sandbox", "allow-scripts");
  // 不透明源没有 referrer 可泄露
  iframe.setAttribute("referrerpolicy", "no-referrer");
  iframe.style.cssText = "width:100%;border:0;display:block;min-height:320px";
  iframe.srcdoc = sandboxDoc();

  let settle: { resolve: () => void; reject: (e: Error) => void } | null = null;
  const done = new Promise<void>((resolve, reject) => {
    settle = { resolve, reject };
  });

  const onMessage = (event: MessageEvent): void => {
    // 只认这个 iframe 发来的消息：其他窗口/扩展可以伪造 message
    if (event.source !== iframe.contentWindow) return;
    const data = event.data as Inbound | undefined;
    if (!data || typeof data !== "object") return;

    if (data.__pkg === "ready") {
      settle?.resolve();
      settle = null;
      return;
    }
    if (data.__pkg === "error") {
      settle?.reject(new Error(data.message));
      settle = null;
      return;
    }
    if (data.__pkg === "call") {
      dispatchCall(api, data.method, data.args)
        .then((value) => {
          iframe.contentWindow?.postMessage(
            { __pkg: "result", id: data.id, ok: true, value },
            "*",
          );
        })
        .catch((err: unknown) => {
          iframe.contentWindow?.postMessage(
            {
              __pkg: "result",
              id: data.id,
              ok: false,
              error: err instanceof Error ? err.message : String(err),
            },
            "*",
          );
        });
    }
  };

  window.addEventListener("message", onMessage);
  container.appendChild(iframe);

  const init: InitMessage = { __pkg: "init", source };
  iframe.addEventListener("load", () => {
    iframe.contentWindow?.postMessage(init, "*");
  });

  return {
    done,
    dispose: () => {
      window.removeEventListener("message", onMessage);
      iframe.remove();
    },
  };
}
