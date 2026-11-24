/**
 * 示例业务包前端入口。
 *
 * 约定：主壳把模块源码用 Blob URL 加载，然后调用 mount(container, shell)。
 * 这里**不能**有 import 语句——包在运行时是孤立模块，没有打包器解析依赖。
 * 需要能力时只能用主壳传进来的 shell，不能碰 window.__TAURI__。
 */
export function mount(container, shell) {
  container.innerHTML = "";

  const head = document.createElement("div");
  head.className = "pkg-head";
  head.textContent = "示例业务包 · 由 pkg:biz.demo 提供";
  container.appendChild(head);

  const note = document.createElement("p");
  note.className = "hint";
  note.textContent =
    "这个页面不是主壳渲染的，是业务包自己的模块。它只能通过 shell 暴露的接口干活。";
  container.appendChild(note);

  const row = document.createElement("div");
  row.className = "actions";

  const printBtn = document.createElement("button");
  printBtn.className = "btn";
  printBtn.textContent = "通过主壳打印一张示例凭证";
  printBtn.onclick = async () => {
    try {
      const receipt = await shell.printVoucher({
        template: "voucher-demo",
        txCode: "biz.demo.hello",
        reference: "DEMO-0001",
        fields: { amount: "100.00", note: "业务包调用" },
      });
      note.textContent = `主壳返回作业号 ${receipt.job_id}（${receipt.pages} 页）`;
    } catch (err) {
      note.textContent = `调用失败：${err}`;
    }
  };
  row.appendChild(printBtn);
  container.appendChild(row);
}
