#!/usr/bin/env node
/**
 * 门禁 2/3 · 契约一致性
 *
 * 对应宪法第 1 条。拿 contracts/fields.json 和真实代码双向比对：
 * 契约有、代码没有 → 实现没跟上；代码有、契约没有 → 有人绕过契约加字段。
 * 两种都必须红。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT, rel, rustFiles, contractsTsFiles, handlerFiles } from "./paths.mjs";

const errors = [];

const contract = JSON.parse(readFileSync(join(ROOT, "contracts/fields.json"), "utf8"));

// —— 解析 Rust：结构体字段（尊重 #[serde(rename)] 与 #[serde(skip)]）——
const rustStructs = new Map();
for (const file of rustFiles) {
  const text = readFileSync(file, "utf8");
  const re = /pub struct (\w+) \{([\s\S]*?)^\}/gm;
  let m;
  while ((m = re.exec(text)) !== null) {
    const [, name, body] = m;
    const fields = [];
    let rename = null;
    let skip = false;
    for (const raw of body.split("\n")) {
      const line = raw.trim();
      if (!line) continue;
      if (line.startsWith("#[serde(")) {
        const r = /rename\s*=\s*"([^"]+)"/.exec(line);
        if (r) rename = r[1];
        if (/\bskip\b/.test(line) && !/skip_serializing_if/.test(line)) skip = true;
        continue;
      }
      if (line.startsWith("#[") || line.startsWith("//")) continue;
      const f = /^pub\s+(\w+)\s*:/.exec(line);
      if (f) {
        if (!skip) fields.push(rename ?? f[1]);
        rename = null;
        skip = false;
      }
    }
    rustStructs.set(name, { file: rel(file), fields });
  }
}

// —— 解析 TS：interface 字段 ——
const tsStructs = new Map();
for (const file of contractsTsFiles) {
  const text = readFileSync(file, "utf8");
  const re = /export interface (\w+) \{([\s\S]*?)^\}/gm;
  let m;
  while ((m = re.exec(text)) !== null) {
    const [, name, body] = m;
    const fields = [];
    for (const raw of body.split("\n")) {
      const line = raw.trim();
      if (!line || line.startsWith("//") || line.startsWith("*") || line.startsWith("/*")) continue;
      const f = /^(\w+)\??\s*:/.exec(line);
      if (f) fields.push(f[1]);
    }
    tsStructs.set(name, { file: rel(file), fields });
  }
}

const diff = (label, expected, actual, where) => {
  const e = new Set(expected);
  const a = new Set(actual);
  const missing = [...e].filter((x) => !a.has(x));
  const extra = [...a].filter((x) => !e.has(x));
  if (missing.length) {
    errors.push(`${label}（${where}）缺少字段：${missing.join(", ")} —— 契约有、实现没跟上`);
  }
  if (extra.length) {
    errors.push(`${label}（${where}）多出字段：${extra.join(", ")} —— 绕过契约加字段，需补 changes/ 提案`);
  }
};

for (const [name, expected] of Object.entries(contract.rust_structs)) {
  const found = rustStructs.get(name);
  if (!found) {
    errors.push(`契约登记了 Rust 结构体 ${name}，但代码里找不到`);
    continue;
  }
  diff(`Rust ${name}`, expected, found.fields, found.file);
}

for (const [name, expected] of Object.entries(contract.ts_interfaces)) {
  const found = tsStructs.get(name);
  if (!found) {
    errors.push(`契约登记了 TS interface ${name}，但前端 contracts.ts 里找不到`);
    continue;
  }
  diff(`TS ${name}`, expected, found.fields, found.file);
}

// —— 比对命令清单 ——
const declared = [];
for (const file of rustFiles) {
  const text = readFileSync(file, "utf8");
  const re = /#\[tauri::command\][\s\S]{0,120}?pub fn (\w+)/g;
  let m;
  while ((m = re.exec(text)) !== null) declared.push(m[1]);
}

let registered = [];
for (const file of handlerFiles) {
  const text = readFileSync(file, "utf8");
  const block = /generate_handler!\[([\s\S]*?)\]/.exec(text);
  if (block) {
    registered = registered.concat(
      block[1]
        .split(",")
        .map((s) => s.trim().replace(/^commands::/, ""))
        .filter(Boolean),
    );
  }
}

const expectedCmds = new Set(contract.commands);
for (const cmd of expectedCmds) {
  if (!declared.includes(cmd)) errors.push(`契约列了命令 ${cmd}，但没有 #[tauri::command] 实现`);
  if (!registered.includes(cmd)) errors.push(`命令 ${cmd} 没有在 generate_handler! 注册，前端会报 command not found`);
}
for (const cmd of declared) {
  if (!expectedCmds.has(cmd)) errors.push(`实现了未登记的命令 ${cmd}：新增命令必须先改 contracts/ipc-commands.md`);
}

if (errors.length) {
  console.error("\n✗ check-contracts 失败\n");
  for (const e of errors) console.error("  • " + e);
  console.error(
    `\n共 ${errors.length} 条不一致。修复建议：契约对→改实现；契约错→先写 changes/ 提案，别直接改 fields.json。\n`,
  );
  process.exit(1);
}

console.log(
  `✓ check-contracts 通过（${Object.keys(contract.rust_structs).length} 个 Rust 结构体、${Object.keys(contract.ts_interfaces).length} 个 TS interface、${expectedCmds.size} 条命令）`,
);
