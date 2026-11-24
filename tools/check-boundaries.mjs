#!/usr/bin/env node
/**
 * 门禁 1/3 · 依赖方向与分层
 *
 * 对应宪法第 2 条。检查的是"谁能 import 谁"，这是最容易在 AI 生成时被破坏、
 * 又最难在 code review 里靠肉眼发现的一类问题。
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const ROOT = new URL("..", import.meta.url).pathname;
const errors = [];
const checked = [];

const rel = (p) => relative(ROOT, p);
const read = (p) => readFileSync(p, "utf8");

function walk(dir, out = []) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return out;
  }
  for (const name of entries) {
    if (name === "node_modules" || name === "target" || name === ".git" || name === "dist") continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

const allFiles = walk(ROOT);
const srcFiles = allFiles.filter((f) => f.startsWith(join(ROOT, "src")) && /\.(ts|vue)$/.test(f));
const rsFiles = allFiles.filter((f) => f.endsWith(".rs"));
const cargoFiles = allFiles.filter((f) => f.endsWith("Cargo.toml"));

// 规则 1：前端不得自己判断权限（唯一例外是 contracts.ts 的类型定义）
const PERM_TOKENS = ["filter_visible", "menu_codes", "action_codes"];
for (const file of srcFiles) {
  if (file.endsWith("src/contracts.ts")) continue;
  const text = read(file);
  for (const token of PERM_TOKENS) {
    if (text.includes(token)) {
      errors.push(
        `${rel(file)} 出现 \`${token}\`：前端不得推导权限。菜单可见性由 Rust 的 filter_visible 决定，前端只渲染结果。（宪法第 2 条）`,
      );
    }
  }
  checked.push(rel(file));
}

// 规则 2：核心 crate 不得依赖 tauri（否则无法脱离 GUI 测试）
const coreCargo = join(ROOT, "crates/tellerkit-core/Cargo.toml");
try {
  if (/tauri/.test(read(coreCargo))) {
    errors.push(`${rel(coreCargo)} 依赖了 tauri：核心逻辑必须能独立 cargo test。`);
  }
} catch {
  errors.push("找不到 crates/tellerkit-core/Cargo.toml");
}

// 规则 3：crates/ 下的代码不得 use tauri 或引用主壳
for (const file of rsFiles.filter((f) => f.includes("/crates/"))) {
  const text = read(file);
  if (/^\s*use\s+tauri(::|\s*;)/m.test(text) || /\btellerkit_lib\b/.test(text)) {
    errors.push(`${rel(file)} 引用了 tauri 或主壳：core 层不得依赖应用层。`);
  }
}

// 规则 4：crates/ 的 Cargo.toml 不得反向依赖 src-tauri
for (const file of cargoFiles.filter((f) => f.includes("/crates/"))) {
  const text = read(file);
  if (/src-tauri/.test(text)) {
    errors.push(`${rel(file)} 依赖了 src-tauri：依赖方向反了。`);
  }
}

// 规则 5：命令层不得 panic（会穿透 IPC 让客户端白屏）
const panicTargets = [join(ROOT, "src-tauri/src/commands.rs")];
for (const file of panicTargets) {
  let text;
  try {
    text = read(file);
  } catch {
    continue;
  }
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

// 规则 6：前端不得 import 核心库
for (const file of srcFiles) {
  const text = read(file);
  if (/from\s+["'][^"']*tellerkit-core/.test(text) || /from\s+["']\.\.\/crates/.test(text)) {
    errors.push(`${rel(file)} 直接 import 了 Rust 侧代码：跨语言的边界只能走 IPC。`);
  }
}

if (errors.length) {
  console.error("\n✗ check-boundaries 失败\n");
  for (const e of errors) console.error("  • " + e);
  console.error(`\n共 ${errors.length} 条越界。修复建议：把判断移回 Rust 侧，或改走 IPC。\n`);
  process.exit(1);
}

console.log(`✓ check-boundaries 通过（扫描 ${checked.length} 个前端文件、${rsFiles.length} 个 Rust 文件）`);
