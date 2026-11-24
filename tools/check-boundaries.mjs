#!/usr/bin/env node
/**
 * 门禁 1/3 · 依赖方向与分层
 *
 * 对应宪法第 2 条。检查"谁能 import 谁"——这是 AI 生成时最容易破坏、
 * 又最难靠肉眼 code review 发现的一类问题。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  ROOT,
  rel,
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
  if (file.includes("/generated/")) continue;
  const text = read(file);
  for (const token of PERM_TOKENS) {
    if (text.includes(token)) {
      errors.push(
        `${rel(file)} 出现 \`${token}\`：前端不得推导权限。可见性由 Rust 侧剪枝决定，前端只渲染结果。（宪法第 2 条）`,
      );
    }
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
