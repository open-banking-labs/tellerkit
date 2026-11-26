#!/usr/bin/env node
/**
 * 门禁 1/3 · 依赖方向与分层
 *
 * 对应宪法第 2 条。检查"谁能 import 谁"——这是 AI 生成时最容易破坏、
 * 又最难靠肉眼 code review 发现的一类问题。
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import {
  ROOT,
  rel,
  posix,
  apps,
  packages,
  frontendFiles,
  rustFiles,
  commandFiles,
  packageSrcDirs,
} from "./paths.mjs";

const errors = [];
const read = (p) => readFileSync(p, "utf8");

// 规则 1：前端不得自己判断权限（唯一例外是各 app 的 contracts.ts 类型定义）
const PERM_TOKENS = ["filter_visible", "menu_codes", "action_codes"];
for (const file of frontendFiles) {
  // generated/ 是生成物，不是手写逻辑：它的内容由 check-contracts 的 hash 校验兜底
  if (posix(file).includes("/generated/")) continue;
  const text = read(file);
  for (const token of PERM_TOKENS) {
    if (text.includes(token)) {
      errors.push(
        `${rel(file)} 出现 \`${token}\`：前端不得推导权限。可见性由 Rust 侧剪枝决定，前端只渲染结果。（宪法第 2 条）`,
      );
    }
  }
}

// 规则 1.5：边界 ① ——只有 platform/tauri.ts 可以接触 Tauri API
const TAURI_ALLOWED = ["/platform/tauri.ts"];
for (const file of frontendFiles) {
  const relPath = posix(file);
  if (TAURI_ALLOWED.some((suffix) => relPath.endsWith(suffix))) continue;
  const text = read(file);
  if (text.includes("@tauri-apps/api")) {
    errors.push(
      `${rel(file)} 直接 import 了 @tauri-apps/api：组件只能依赖 platform/client.ts 的 ShellClient 接口。（docs/architecture.md §2）`,
    );
  }
  if (/\binvoke\s*[<(]/.test(text)) {
    errors.push(
      `${rel(file)} 直接调用 invoke：IPC 调用只能出现在 platform/tauri.ts 里。`,
    );
  }
}

// 规则 1.8：边界 ② —— 命令层只做映射，不得直接调领域编排（docs/architecture.md §2）
const COMMAND_FILE = "apps/shell/src-tauri/src/commands.rs";
// 匹配调用而不是 import：类型导入是允许的（MenuView 本身就是 DTO 字段）。
// 用正则匹配任意模块路径形式，避免 `use ... as s; s::boot_load(` 绕过纯字符串匹配（D-02）。
const FORBIDDEN_IN_COMMANDS = [
  ["boot_load", "用例编排应放在 app.rs"],
  ["check_and_update", "用例编排应放在 app.rs"],
  ["reload", "用例编排应放在 app.rs"],
  ["filter_visible", "菜单剪枝应放在 app.rs"],
  ["scan_dir", "业务包加载应放在 app.rs"],
  ["merge_packages", "业务包合并应放在 app.rs"],
  ["run_once", "重放编排应放在 app.rs"],
  ["ensure_allowed", "设备白名单校验应放在 app.rs"],
].map(([name, why]) => [
  name === "ensure_allowed"
    ? new RegExp(`\\.${name}\\s*\\(`)
    : new RegExp(`(?:^|[^A-Za-z0-9_])(?:[A-Za-z_][A-Za-z0-9_]*::)+${name}\\s*\\(`),
  name,
  why,
]);
{
  const target = join(ROOT, COMMAND_FILE);
  if (existsSync(target)) {
    const text = read(target);
    for (const [re, name, why] of FORBIDDEN_IN_COMMANDS) {
      if (re.test(text)) {
        errors.push(`${COMMAND_FILE} 出现了 ${name}() 调用：${why}（命令层只做加锁 → 用例 → DTO 映射）`);
      }
    }
  }
}

// 规则 1.9：桌面端边界 —— core 里不得出现核心系统的业务词汇（docs/desktop-scope.md §7）
const CORE_DIR = "packages/core/src";
const CORE_BUSINESS_TERMS = ["balance", "interest", "accrual", "fee"];
for (const file of rustFiles.filter((f) => f.includes(CORE_DIR))) {
  const text = read(file);
  for (const term of CORE_BUSINESS_TERMS) {
    const re = new RegExp(`\\b${term}\\b`, "i");
    if (re.test(text)) {
      errors.push(
        `${rel(file)} 出现核心系统词汇 \`${term}\`：账务规则属于远端核心系统，桌面端不得内置（docs/desktop-scope.md）`,
      );
    }
  }
}

// 规则 1.95：业务包模块只能经沙箱加载（docs/security-boundary.md §4.5）
for (const file of frontendFiles) {
  const relPath = posix(file);
  if (relPath.endsWith("/platform/sandbox.ts")) continue;
  const text = read(file);
  if (/URL\.createObjectURL\s*\(/.test(text) && /import\s*\(/.test(text)) {
    errors.push(
      `${rel(file)} 在主窗口里用 Blob URL 动态 import：业务包必须经 platform/sandbox.ts 加载，否则包能访问 window/DOM`,
    );
  }
}

// 规则 2：核心包不得依赖 tauri（否则无法脱离 GUI 测试）
for (const dir of packageSrcDirs) {
  const pkgRoot = dir.replace(/\/src$/, "");
  const cargo = join(pkgRoot, "Cargo.toml");
  try {
    if (/\btauri\b/.test(read(cargo))) {
      errors.push(`${rel(cargo)} 依赖了 tauri：核心逻辑必须能独立 cargo test。`);
    }
  } catch {
    /* 不是 Rust 包，跳过 */
  }
}

// 规则 3：packages/ 下的代码不得 use tauri 或引用主壳
for (const file of rustFiles.filter((f) => packages.some((p) => f.startsWith(p)))) {
  const text = read(file);
  if (/^\s*use\s+tauri(::|\s*;)/m.test(text) || /\btellerkit_lib\b/.test(text)) {
    errors.push(`${rel(file)} 引用了 tauri 或主壳：packages 层不得依赖 apps 层。`);
  }
}

// 规则 4：packages/ 的 Cargo.toml 不得反向依赖 apps/
for (const pkg of packages) {
  const cargo = join(pkg, "Cargo.toml");
  try {
    if (/apps\//.test(read(cargo))) {
      errors.push(`${rel(cargo)} 依赖了 apps/：依赖方向反了。`);
    }
  } catch {
    /* 非 Rust 包 */
  }
}

// 规则 5：命令层不得 panic（会穿透 IPC 让客户端白屏）
for (const file of commandFiles) {
  const text = read(file);
  const idx = text.indexOf("#[cfg(test)]");
  const body = idx >= 0 ? text.slice(0, idx) : text;
  body.split("\n").forEach((line, i) => {
    if (/\.unwrap\(\)|\.expect\(/.test(line)) {
      errors.push(
        `${rel(file)}:${i + 1} 使用了 unwrap/expect：命令层 panic 会直接打到前端。（src-tauri/AGENTS.md）`,
      );
    }
  });
}

// 规则 6：前端不得 import Rust 侧代码（跨语言只能走 IPC）
for (const file of frontendFiles) {
  const text = read(file);
  if (/from\s+["'][^"']*tellerkit-core/.test(text) || /from\s+["'][^"']*\/packages\//.test(text)) {
    errors.push(`${rel(file)} 直接 import 了 Rust 侧代码：跨语言的边界只能走 IPC。`);
  }
}

// 规则 7：apps/ 下只允许一个前端入口（防止复制粘贴出第二个 UI）
if (apps.length !== 1) {
  errors.push(`apps/ 下有 ${apps.length} 个应用：当前架构只允许一个主壳，新增前先写 ADR。`);
}

if (errors.length) {
  console.error("\n✗ check-boundaries 失败\n");
  for (const e of errors) console.error("  • " + e);
  console.error(`\n共 ${errors.length} 条越界。修复建议：把判断移回 Rust 侧，或改走 IPC。\n`);
  process.exit(1);
}

console.log(
  `✓ check-boundaries 通过（${apps.length} 个 app、${packages.length} 个 package、${frontendFiles.length} 个前端文件、${rustFiles.length} 个 Rust 文件）`,
);
