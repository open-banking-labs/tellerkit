#!/usr/bin/env node
/**
 * 门禁 2/3 · 契约一致性（三方）
 *
 * 对应宪法第 1 条。三份东西必须完全对齐：
 *   1. contracts/fields.json   人写的契约
 *   2. Rust 结构体             实现
 *   3. apps/shell/src/generated 由 ts-rs 从 Rust 生成的前端类型
 *
 * 另外校验生成的 manifest：手改生成物、忘了重新生成，都会红。
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { ROOT, rel, rustFiles, generatedDirs, handlerFiles } from "./paths.mjs";

const errors = [];
const read = (p) => readFileSync(p, "utf8");

const contract = JSON.parse(read(join(ROOT, "contracts/fields.json")));
const genDir = generatedDirs[0];

if (!genDir) {
  console.error("✗ 找不到 apps/*/src/generated：先跑 `npm run bindings`");
  process.exit(1);
}

// —— 0. 生成物必须是生成的，不是手写的 ——
for (const app of ["apps/shell"]) {
  const stray = join(ROOT, app, "src/contracts.ts");
  try {
    read(stray);
    errors.push(
      `${app}/src/contracts.ts 出现了：跨语言类型只能由 Rust 生成，手写契约文件禁止回流（ADR-0007）`,
    );
  } catch {
    /* 正常：文件不存在 */
  }
}

// —— 1. manifest 必须与磁盘一致（抓手改与漏更新） ——
let manifest;
try {
  manifest = JSON.parse(read(join(genDir, "manifest.json")));
} catch {
  console.error("✗ 缺少 apps/shell/src/generated/manifest.json：先跑 `npm run bindings`");
  process.exit(1);
}

const onDisk = readdirSync(genDir).filter((f) => f.endsWith(".ts") || f === "manifest.json").sort();
const inManifest = Object.keys(manifest.files).sort();
const missing = inManifest.filter((f) => !onDisk.includes(f));
const extra = onDisk.filter((f) => f !== "manifest.json" && !inManifest.includes(f));

if (missing.length) {
  errors.push(`生成物缺失：${missing.join(", ")} —— 有人删了文件却没重新生成`);
}
if (extra.length) {
  errors.push(
    `生成物多出：${extra.join(", ")} —— 手写文件混进 generated/，或新增了 #[ts(export)] 却没登记契约`,
  );
}

for (const [file, want] of Object.entries(manifest.files)) {
  if (missing.includes(file)) continue;
  const got = createHash("sha256").update(readFileSync(join(genDir, file))).digest("hex");
  if (got !== want) {
    errors.push(
      `apps/shell/src/generated/${file} 被改动过（hash 不符）：生成物只能由 \`npm run bindings\` 产出`,
    );
  }
}

// —— 2. 结构体字段：Rust 实现 ——
const rustStructs = new Map();
for (const file of rustFiles) {
  const text = read(file);
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

// —— 3. 结构体字段：生成的 TS ——
const ttStructs = new Map();
for (const file of readdirSync(genDir).filter((f) => f.endsWith(".ts") && f !== "index.ts")) {
  const text = read(join(genDir, file))
    .replace(/\/\*\*[\s\S]*?\*\//g, "") // 去掉文档注释，避免中文里的冒号干扰
    .replace(/\/\/[^\n]*/g, "");
  const m = /export type (\w+) = \{([\s\S]*)\};/.exec(text);
  if (!m) continue;
  const fields = [...m[2].matchAll(/(\w+)\s*:/g)].map((x) => x[1]);
  ttStructs.set(m[1], { file: `generated/${file}`, fields });
}

const diff = (label, expected, actual, where) => {
  const e = new Set(expected);
  const a = new Set(actual);
  const miss = [...e].filter((x) => !a.has(x));
  const over = [...a].filter((x) => !e.has(x));
  if (miss.length) errors.push(`${label}（${where}）缺少字段：${miss.join(", ")} —— 契约有、实现没跟上`);
  if (over.length)
    errors.push(`${label}（${where}）多出字段：${over.join(", ")} —— 绕过契约加字段，需补 changes/ 提案`);
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
  const found = ttStructs.get(name);
  if (!found) {
    errors.push(`契约登记了 TS 类型 ${name}，但 generated/ 里没有 ${name}.ts（跑 npm run bindings）`);
    continue;
  }
  diff(`TS ${name}`, expected, found.fields, found.file);
}

// —— 4. 生成物清单必须与契约登记一致 ——
const expectedTypes = new Set(contract.generated_types);
const actualTypes = new Set(
  readdirSync(genDir)
    .filter((f) => f.endsWith(".ts") && !["index.ts", "commands.ts"].includes(f))
    .map((f) => f.replace(/\.ts$/, "")),
);
for (const t of expectedTypes) {
  if (!actualTypes.has(t)) errors.push(`契约登记的生成类型 ${t} 没有出现在 generated/`);
}
for (const t of actualTypes) {
  if (!expectedTypes.has(t))
    errors.push(`generated/${t}.ts 未在 contracts/fields.json 的 generated_types 登记：新增类型必须先改契约`);
}

// —— 5. 命令清单 ——
const declared = [];
for (const file of rustFiles) {
  const text = read(file);
  const re = /#\[tauri::command\][\s\S]{0,120}?pub fn (\w+)/g;
  let m;
  while ((m = re.exec(text)) !== null) declared.push(m[1]);
}

let registered = [];
for (const file of handlerFiles) {
  const block = /generate_handler!\[([\s\S]*?)\]/.exec(read(file));
  if (block) {
    registered = registered.concat(
      block[1]
        .split(",")
        .map((s) => s.trim().replace(/^commands::/, ""))
        .filter(Boolean),
    );
  }
}

// 命令名也要生成到前端：由 generate_handler! 派生，避免手写拼错
const names = registered.slice().sort();
const cmdTs = [
  "// 本文件由 tools/gen-bindings.mjs 生成，请勿手改。",
  "",
  `export type CommandName = ${names.map((n) => JSON.stringify(n)).join(" | ")};`,
  "",
].join("\n");
const cmdPath = join(genDir, "commands.ts");
let cmdDirty = false;
try {
  cmdDirty = read(cmdPath) !== cmdTs;
} catch {
  cmdDirty = true;
}
if (cmdDirty) {
  errors.push(
    "apps/shell/src/generated/commands.ts 与 generate_handler! 不一致：跑 `npm run bindings` 重新生成",
  );
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
    `\n共 ${errors.length} 条不一致。修复顺序：改 Rust → 改 contracts（走 changes/）→ npm run bindings → 重新跑门禁。\n`,
  );
  process.exit(1);
}

console.log(
  `✓ check-contracts 通过（${Object.keys(contract.rust_structs).length} 个结构体 × 3 方一致、${actualTypes.size} 个生成类型、${expectedCmds.size} 条命令）`,
);
