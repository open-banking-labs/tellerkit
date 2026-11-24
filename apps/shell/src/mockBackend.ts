/**
 * 浏览器预览后端的适配层（仅在没有 Tauri 运行时启用）。
 *
 * 数据来自 `src/fixtures/*.json`，由 `mtree-tool fixtures` 用**真实核心逻辑**
 * 生成（同一份 demo 树、同一个剪枝函数），所以这里只是搬运，不重算权限。
 * 目的是让前端可以脱离 Tauri 迭代样式，同时保证看到的就是客户端会渲染的结果。
 */
import bootFixture from "./fixtures/boot.json";
import sessionsFixture from "./fixtures/sessions.json";
import type {
  BootInfo,
  DevState,
  LoginResult,
  MenuView,
  ReloadReport,
  UpdateReport,
} from "./generated";

const boot = bootFixture as unknown as BootInfo;
const sessions = sessionsFixture as unknown as Record<string, LoginResult>;

const dev: DevState = {
  offline: false,
  tampered: false,
  server_version: 7,
  local_versions: [7],
};

let current: LoginResult | null = null;

export const mockApi = {
  async boot(): Promise<BootInfo> {
    return boot;
  },

  async login(roleId: string): Promise<LoginResult> {
    const session = sessions[roleId];
    if (!session) throw new Error(`未知角色：${roleId}`);
    current = session;
    return session;
  },

  async logout(): Promise<void> {
    current = null;
  },

  async currentMenu(): Promise<MenuView[]> {
    return current?.menu ?? [];
  },

  async checkUpdate(): Promise<UpdateReport> {
    return {
      checked: true,
      local: 7,
      latest: 7,
      updated: false,
      note: "浏览器预览模式：版本探测是静态值，真实行为请在 Tauri 客户端验证",
    };
  },

  async reloadFromDisk(): Promise<ReloadReport> {
    return {
      menu_version: 7,
      source: "local",
      recovered: false,
      verify_failed: false,
      note: "浏览器预览模式：不做真实验签，请用 `npm run tauri:dev` 验证",
      menu: current?.menu ?? null,
    };
  },

  async devSetOffline(offline: boolean): Promise<DevState> {
    dev.offline = offline;
    return { ...dev };
  },

  async devSetTampered(tampered: boolean): Promise<DevState> {
    dev.tampered = tampered;
    return { ...dev };
  },

  async devBumpServerVersion(): Promise<DevState> {
    dev.server_version += 1;
    return { ...dev };
  },

  async devCorruptLocal(): Promise<string> {
    return "浏览器预览模式：没有真实文件可篡改，请用 `npm run tauri:dev` 验证";
  },

  async devResetLocal(): Promise<DevState> {
    return { ...dev, local_versions: [] };
  },
};
