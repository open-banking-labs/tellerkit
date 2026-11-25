<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { api, isTauri } from "./api";
import { installClipboardGuard, onClipboardBlocked } from "./clipboard";
import { actionLabel, menuNote } from "./labels";
import type { BootInfo, DevState, LoginResult, MenuView, UpdateReport } from "./generated";
import type { LogEntry, MenuRow } from "./ui-types";
import DevPanel from "./components/DevPanel.vue";
import LockScreen from "./components/LockScreen.vue";
import ReconcilePanel from "./components/ReconcilePanel.vue";
import LoginView from "./components/LoginView.vue";
import MenuList from "./components/MenuList.vue";
import PackageHost from "./components/PackageHost.vue";
import TxPanel from "./components/TxPanel.vue";
import Watermark from "./components/Watermark.vue";

type Phase = "booting" | "login" | "ready" | "error";

const phase = ref<Phase>("booting");
const bootInfo = ref<BootInfo | null>(null);
const session = ref<LoginResult | null>(null);
const selected = ref<MenuView | null>(null);
const focusedIndex = ref(0);
const query = ref("");
const logs = ref<LogEntry[]>([]);
const busy = ref(false);
const fatal = ref("");
const dev = ref<DevState | null>(null);
const update = ref<UpdateReport | null>(null);
/** 开发构建才显示演示开关；后端 `dev_only()` 也会拒绝，两层防护。 */
const showDevTools = !import.meta.env.PROD;
const showReconcile = ref(false);
const locked = ref(false);
const lockReason = ref<string | null>(null);
let sessionTimer: number | undefined;
let lastTouch = 0;
const expanded = ref<Set<string>>(new Set());

/** 浏览器预览：没有 Tauri 运行时，数据来自 Rust 导出的 fixture。 */
const preview = !isTauri();

let probeTimer: number | undefined;

// ---------- 工具 ----------

function now(): string {
  return new Date().toLocaleTimeString("zh-CN", { hour12: false });
}

function log(level: LogEntry["level"], message: string): void {
  logs.value.unshift({ time: now(), level, message });
  if (logs.value.length > 300) logs.value.length = 300;
}

function flattenAll(nodes: MenuView[], out: MenuView[] = []): MenuView[] {
  for (const n of nodes) {
    out.push(n);
    flattenAll(n.children, out);
  }
  return out;
}

function countMenu(nodes: MenuView[]): number {
  return flattenAll(nodes, []).length;
}

function firstNavigable(nodes: MenuView[]): MenuView | null {
  for (const n of nodes) {
    if (n.type === "page" && n.direct) return n;
    const child = firstNavigable(n.children);
    if (child) return child;
  }
  return null;
}

function expandAll(nodes: MenuView[]): void {
  const next = new Set<string>();
  for (const n of flattenAll(nodes, [])) {
    if (n.children.length) next.add(n.code);
  }
  expanded.value = next;
}

function pathOf(nodes: MenuView[], code: string, trail: MenuView[] = []): MenuView[] {
  for (const n of nodes) {
    const here = [...trail, n];
    if (n.code === code) return here;
    const deeper = pathOf(n.children, code, here);
    if (deeper.length) return deeper;
  }
  return [];
}

const nodeIndex = computed(() => {
  const map = new Map<string, MenuView>();
  for (const n of flattenAll(session.value?.menu ?? [], [])) map.set(n.code, n);
  return map;
});

const rows = computed<MenuRow[]>(() => {
  const nodes = session.value?.menu ?? [];
  const q = query.value.trim().toLowerCase();
  const out: MenuRow[] = [];
  const push = (n: MenuView, depth: number) => {
    out.push({
      idx: out.length,
      code: n.code,
      title: n.title,
      depth,
      kind: n.type === "group" ? "group" : "page",
      direct: n.direct,
      hasChildren: n.children.length > 0,
      expanded: expanded.value.has(n.code),
      shortcut: session.value?.snapshot.shortcuts[n.code],
    });
  };

  if (q) {
    for (const n of flattenAll(nodes, [])) {
      if (n.title.toLowerCase().includes(q) || n.code.toLowerCase().includes(q)) push(n, 0);
    }
    return out;
  }

  const walk = (list: MenuView[], depth: number) => {
    for (const n of list) {
      push(n, depth);
      if (n.children.length && expanded.value.has(n.code)) walk(n.children, depth + 1);
    }
  };
  walk(nodes, 0);
  return out;
});

const crumbs = computed(() => (selected.value ? pathOf(session.value?.menu ?? [], selected.value.code) : []));

// ---------- 启动与登录 ----------

async function init(): Promise<void> {
  try {
    const info = await api.boot();
    bootInfo.value = info;
    dev.value = {
      offline: info.offline,
      tampered: false,
      server_version: info.load.menu_version,
      local_versions: info.local_versions,
    };
    for (const note of info.notes) log("info", note);
    log(
      "ok",
      `菜单包加载完成 v${info.load.menu_version}（${info.load.source === "local" ? "本地" : "服务端"}，${info.load.menu_nodes} 节点，${info.load.load_ms.toFixed(1)}ms）`,
    );
    phase.value = "login";
  } catch (err) {
    fatal.value = String(err);
    phase.value = "error";
    log("error", String(err));
  }
}

async function login(roleId: string): Promise<void> {
  if (busy.value) return;
  busy.value = true;
  try {
    const res = await api.login(roleId);
    session.value = res;
    expandAll(res.menu);
    selected.value = firstNavigable(res.menu);
    focusedIndex.value = 0;
    phase.value = "ready";
    log(
      "ok",
      `登录 ${res.snapshot.role_label}（${res.snapshot.teller_no}）：可见菜单 ${countMenu(res.menu)} 项，剪枝耗时 ${res.filter_ms.toFixed(3)}ms`,
    );
    scheduleProbe();
    startSessionWatch();
    touchSession();
  } catch (err) {
    log("error", `登录失败：${err}`);
  } finally {
    busy.value = false;
  }
}

async function logout(): Promise<void> {
  window.clearInterval(sessionTimer);
  locked.value = false;
  lockReason.value = null;
  await api.logout();
  session.value = null;
  selected.value = null;
  expanded.value = new Set();
  query.value = "";
  update.value = null;
  phase.value = "login";
  log("info", "已登出：内存中的权限快照已清空");
}

/** 渲染之后才发起的后台版本探测，绝不阻塞首屏。 */
function scheduleProbe(): void {
  window.clearTimeout(probeTimer);
  probeTimer = window.setTimeout(async () => {
    try {
      const r = await api.checkUpdate();
      update.value = r;
      log(r.updated ? "ok" : "info", `[后台] ${r.note}`);
      if (r.updated && session.value) {
        session.value.menu = await api.currentMenu();
        expandAll(session.value.menu);
      }
    } catch (err) {
      log("warn", `[后台] 版本探测失败：${err}`);
    }
  }, 600);
}

// ---------- 交互 ----------

function toggle(code: string): void {
  const next = new Set(expanded.value);
  if (next.has(code)) next.delete(code);
  else next.add(code);
  expanded.value = next;
}

function activate(idx: number): void {
  const row = rows.value[idx];
  if (!row) return;
  focusedIndex.value = idx;
  const node = nodeIndex.value.get(row.code);
  if (!node) return;

  if (row.hasChildren) toggle(row.code);
  if (node.type === "group") return;

  if (node.direct) {
    selected.value = node;
  } else {
    log("warn", `「${node.title}」是容器节点：你无权直接进入，但下面有你可访问的菜单`);
  }
}

function moveFocus(delta: number): void {
  const n = rows.value.length;
  if (!n) return;
  focusedIndex.value = Math.min(n - 1, Math.max(0, focusedIndex.value + delta));
  document.querySelector(`[data-row="${focusedIndex.value}"]`)?.scrollIntoView({ block: "nearest" });
}

function handleShortcut(e: KeyboardEvent): void {
  const parts: string[] = [];
  if (e.altKey) parts.push("Alt");
  if (e.metaKey) parts.push("Meta");
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.shiftKey) parts.push("Shift");
  let key = e.key;
  if (e.code?.startsWith("Digit")) key = e.code.slice(5);
  else if (e.code?.startsWith("Key")) key = e.code.slice(3);
  parts.push(key.toUpperCase());
  const combo = parts.join("+");

  const hit = Object.entries(session.value?.snapshot.shortcuts ?? {}).find(([, v]) => v === combo);
  if (!hit) return;
  e.preventDefault();
  const idx = rows.value.findIndex((r) => r.code === hit[0]);
  if (idx >= 0) {
    focusedIndex.value = idx;
    activate(idx);
  }
}

function onKeydown(e: KeyboardEvent): void {
  if (phase.value !== "ready") return;
  const t = e.target as HTMLElement | null;
  if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) {
    if (e.key === "Escape") query.value = "";
    return;
  }
  if (!rows.value.length) return;

  switch (e.key) {
    case "ArrowDown":
      e.preventDefault();
      moveFocus(1);
      break;
    case "ArrowUp":
      e.preventDefault();
      moveFocus(-1);
      break;
    case "ArrowRight": {
      const row = rows.value[focusedIndex.value];
      if (row?.hasChildren && !expanded.value.has(row.code)) toggle(row.code);
      break;
    }
    case "ArrowLeft": {
      const row = rows.value[focusedIndex.value];
      if (row?.hasChildren && expanded.value.has(row.code)) toggle(row.code);
      break;
    }
    case "Enter":
      e.preventDefault();
      activate(focusedIndex.value);
      break;
    case "Escape":
      query.value = "";
      break;
    default:
      handleShortcut(e);
  }
}

// ---------- 验收动作 ----------

async function run(action: () => Promise<void>): Promise<void> {
  if (busy.value) return;
  busy.value = true;
  try {
    await action();
  } catch (err) {
    log("error", String(err));
  } finally {
    busy.value = false;
  }
}

function showAudit(): Promise<void> {
  return run(async () => {
    const lines = await api.auditTail(20);
    for (const line of lines.slice(0, 10)) log("info", `[审计] ${line}`);
    if (lines.length > 10) log("info", `[审计] 还有 ${lines.length - 10} 条，见账本`);
  });
}

function checkUpdate(): Promise<void> {
  return run(async () => {
    const r = await api.checkUpdate();
    update.value = r;
    log(r.updated ? "ok" : r.checked ? "info" : "warn", r.note);
    if (r.updated && session.value) {
      session.value.menu = await api.currentMenu();
      expandAll(session.value.menu);
    }
  });
}

function toggleOffline(): Promise<void> {
  return run(async () => {
    dev.value = await api.dev.setOffline(!dev.value?.offline);
    log("warn", dev.value.offline ? "已模拟断网：版本探测将失败，但本地菜单仍可用" : "已恢复网络");
  });
}

function toggleTampered(): Promise<void> {
  return run(async () => {
    dev.value = await api.dev.setTampered(!dev.value?.tampered);
    log("warn", dev.value.tampered ? "服务端将下发签名错误的菜单包（用于验证下载侧验签）" : "服务端已恢复下发正确包");
  });
}

function bumpVersion(): Promise<void> {
  return run(async () => {
    dev.value = await api.dev.bumpServerVersion();
    log("info", `服务端菜单版本提升到 v${dev.value.server_version}`);
  });
}

function corruptLocal(): Promise<void> {
  return run(async () => {
    const msg = await api.dev.corruptLocal();
    log("warn", msg);
  });
}

function reload(): Promise<void> {
  return run(async () => {
    const r = await api.reloadFromDisk();
    if (r.menu) {
      session.value && (session.value.menu = r.menu);
      expandAll(r.menu);
    }
    log(r.verify_failed ? "warn" : "ok", r.note);
  });
}

function resetLocal(): Promise<void> {
  return run(async () => {
    dev.value = await api.dev.resetLocal();
    log("warn", "已删除本地菜单包，重新执行启动流程（相当于首次启动）");
    const info = await api.boot();
    bootInfo.value = info;
    for (const note of info.notes) log("info", note);
    log("ok", `重新加载完成：${info.load.source === "network" ? "服务端拉取" : "本地命中"} v${info.load.menu_version}，${info.load.load_ms.toFixed(1)}ms`);
    if (session.value) {
      session.value.menu = await api.currentMenu();
      expandAll(session.value.menu);
    }
  });
}

let uninstallClipboard: (() => void) | null = null;
let unsubscribeClipboard: (() => void) | null = null;

/** 节流：鼠标移动每秒最多一次 IPC，否则事件比锁屏判定还密。 */
function touchSession(): void {
  const now = Date.now();
  if (now - lastTouch < 1000) return;
  lastTouch = now;
  void api.sessionTouch();
}

/** 每秒评估一次会话状态。锁屏状态由 Rust 持有，前端只负责显示。 */
function startSessionWatch(): void {
  window.clearInterval(sessionTimer);
  sessionTimer = window.setInterval(async () => {
    try {
      const state = await api.sessionTick();
      applySession(state.locked, state.reason);
    } catch {
      /* 账本/IPC 抖动不该让界面炸掉，下一拍再试 */
    }
  }, 1000);
}

function applySession(isLocked: boolean, reason: string | null): void {
  if (isLocked && !locked.value) {
    log("warn", `会话已锁定：${reason ?? "需要重新验证"}`);
  }
  locked.value = isLocked;
  lockReason.value = reason;
}

async function onWindowBlur(): Promise<void> {
  if (!session.value) return;
  try {
    const state = await api.sessionBlur();
    applySession(state.locked, state.reason);
  } catch {
    /* 同上 */
  }
}

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  window.addEventListener("mousemove", touchSession);
  window.addEventListener("mousedown", touchSession);
  window.addEventListener("blur", onWindowBlur);
  uninstallClipboard = installClipboardGuard();
  unsubscribeClipboard = onClipboardBlocked((field) => {
    log("warn", `已阻止复制敏感字段：${field}`);
  });
  void init();
});

onUnmounted(() => {
  window.clearTimeout(probeTimer);
  window.clearInterval(sessionTimer);
  window.removeEventListener("keydown", onKeydown);
  window.removeEventListener("mousemove", touchSession);
  window.removeEventListener("mousedown", touchSession);
  window.removeEventListener("blur", onWindowBlur);
  uninstallClipboard?.();
  unsubscribeClipboard?.();
});
</script>

<template>
  <div v-if="phase === 'booting'" class="center-state">
    <span class="spinner"></span> 正在加载本地菜单包…
  </div>

  <div v-else-if="phase === 'login' || phase === 'error'" class="login-wrap">
    <LoginView :boot="bootInfo" :busy="busy" :error="fatal" @select="login" />
  </div>

  <div v-else class="shell">
    <header class="topbar">
      <div class="brand">TellerKit <small>柜面终端 · P0</small></div>
      <span class="chip" :class="bootInfo?.load.source === 'local' ? 'ok' : 'warn'">
        <i class="dot"></i>
        {{ bootInfo?.load.source === "local" ? "本地菜单包" : "服务端回退" }}
      </span>
      <span class="chip">菜单 v{{ bootInfo?.load.menu_version }}</span>
      <span class="chip">加载 {{ bootInfo?.load.load_ms.toFixed(1) }}ms</span>
      <span v-if="dev?.offline" class="chip offline"><i class="dot"></i> 已断网</span>
      <span v-if="preview" class="chip warn">浏览器预览 · 静态数据</span>
      <div class="spacer"></div>
      <div class="teller" v-if="session">
        <span>网点 <b>{{ session.snapshot.outlet_code }}</b></span>
        <span>柜员 <b>{{ session.snapshot.teller_no }}</b></span>
        <span class="chip">{{ session.snapshot.role_label }}</span>
      </div>
    </header>

    <div class="body">
      <aside class="sidebar">
        <div class="sidebar-head">
          <div class="sidebar-title">
            <span>功能菜单</span>
            <span class="mono">{{ rows.length }} 项</span>
          </div>
          <input v-model="query" class="search" placeholder="搜索菜单（Esc 清空）" spellcheck="false" />
        </div>
        <MenuList
          :rows="rows"
          :selected-code="selected?.code ?? null"
          :focused-index="focusedIndex"
          empty-text="没有可见菜单：当前角色没有可访问的功能"
          @activate="activate"
          @hover="focusedIndex = $event"
        />
      </aside>

      <main class="main">
        <template v-if="selected">
          <div class="crumbs">
            {{ crumbs.map((c) => c.title).join(" / ") }}
          </div>
          <h1>{{ selected.title }}</h1>
          <div class="route">
            {{ selected.route ?? "（无路由）" }}
            <template v-if="menuNote(selected.code)"> · {{ menuNote(selected.code) }}</template>
          </div>

          <div class="card">
            <h2>可用操作（按 actionCodes 过滤）</h2>
            <div v-if="selected.actions.length" class="actions">
              <button v-for="a in selected.actions" :key="a" class="btn primary" :title="a">
                {{ actionLabel(a) }}
              </button>
            </div>
            <p v-else class="hint" style="margin: 0">
              该页面没有授予任何按钮级权限，操作按钮全部隐藏。
            </p>
          </div>

          <div class="card">
            <h2>节点元数据</h2>
            <div class="meta-grid">
              <div>
                <div class="k">菜单 code</div>
                <div class="v">{{ selected.code }}</div>
              </div>
              <div>
                <div class="k">直接权限</div>
                <div class="v">
                  <span class="tag" :class="selected.direct ? 'ok' : ''">
                    {{ selected.direct ? "已授予" : "仅容器" }}
                  </span>
                </div>
              </div>
              <div>
                <div class="k">子节点</div>
                <div class="v">{{ selected.children.length }}</div>
              </div>
              <div>
                <div class="k">快捷键</div>
                <div class="v">{{ session?.snapshot.shortcuts[selected.code] ?? "—" }}</div>
              </div>
            </div>
          </div>

          <TxPanel v-if="selected.source === 'shell'" :key="selected.code" :node="selected" />
          <PackageHost v-else :key="selected.code" :node="selected" />
        </template>
        <div v-else class="placeholder">选择左侧菜单开始。</div>
      </main>

      <!-- 演示开关只在开发构建显示：正式包里后端也会拒绝这些命令（双保险） -->
      <DevPanel
        v-if="showDevTools"
        :boot="bootInfo"
        :session="session"
        :dev="dev"
        :update="update"
        :busy="busy"
        @check-update="checkUpdate"
        @toggle-offline="toggleOffline"
        @toggle-tampered="toggleTampered"
        @bump-version="bumpVersion"
        @corrupt="corruptLocal"
        @reload="reload"
        @reset="resetLocal"
        @logout="logout"
        @audit="showAudit"
      />
    </div>

    <Watermark
      v-if="session"
      :teller-no="session.snapshot.teller_no"
      :outlet="session.snapshot.outlet_code"
    />
    <ReconcilePanel v-if="showReconcile" @close="showReconcile = false" />
    <LockScreen
      v-if="locked"
      :reason="lockReason"
      @reauth="logout"
    />

    <footer class="console">
      <div class="console-head">
        <span>运行日志</span>
        <div class="spacer"></div>
        <button class="btn ghost" style="color: #6f8aa6" @click="logs = []">清空</button>
      </div>
      <div class="console-body">
        <div v-for="(entry, i) in logs" :key="i" class="logline" :class="entry.level">
          <span class="t">{{ entry.time }}</span>
          <span class="lv">{{ entry.level.toUpperCase() }}</span>
          <span>{{ entry.message }}</span>
        </div>
      </div>
    </footer>
  </div>
</template>
