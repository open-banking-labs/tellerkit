#!/usr/bin/env node
/**
 * 门禁 2/3 · 契约一致性（三方 + 参数个数）
 *
 * 对应宪法第 1 条。三份东西必须对齐：
 *   1. contracts/fields.json   人写的契约（当前格式只登记字段名）
 *   2. Rust 结构体             实现
 *   3. apps/shell/src/generated 由 ts-rs 从 Rust 生成的前端类型
 *
 * 字段名与 fields.json 对齐；字段**类型与可选性**在 Rust 与生成 TS 之间逐字段比对
 * （fields.json 是只读契约，升级为结构化字段需走 changes/ 提案）。
 * `#[tauri::command]` 的有效参数个数必须与 contracts/ipc-commands.md 表格一致。
 * 生成物 manifest 同时防手改与漏重新生成。
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

// —— 2. 结构体字段：Rust 实现（只索引参与序列化的 DTO，内部同名 struct 不算契约） ——
const normType = (s) => String(s ?? "").replace(/\s+/g, " ").trim();
const PRIMITIVES = new Map([
  ["String", "string"],
  ["str", "string"],
  ["bool", "boolean"],
  ["u8", "number"],
  ["u16", "number"],
  ["u32", "number"],
  ["u64", "number"],
  ["usize", "number"],
  ["i8", "number"],
  ["i16", "number"],
  ["i32", "number"],
  ["i64", "number"],
  ["isize", "number"],
  ["f32", "number"],
  ["f64", "number"],
]);
const unwrapGenerics = (s, open, close) => {
  if (!s.startsWith(open) || !s.endsWith(close)) return null;
  const o = open[open.length - 1];
  const c = close[close.length - 1];
  let depth = 0;
  for (let i = 0; i < s.length; i++) {
    if (s[i] === o) depth++;
    else if (s[i] === c && --depth === 0 && i !== s.length - 1) return null;
  }
  return s.slice(open.length, -close.length);
};
/** Rust 字段类型 → TS 形状；nullable 表示 Option / `| null`。 */
function rustType(raw) {
  const s = normType(raw);
  const inner = unwrapGenerics(s, "Option<", ">");
  if (inner !== null) {
    const t = rustType(inner);
    return { type: t.type, nullable: true };
  }
  const vec = unwrapGenerics(s, "Vec<", ">");
  if (vec !== null) return { type: `Array<${rustType(vec).type}>`, nullable: false };
  const mapOpen = s.startsWith("BTreeMap<") ? "BTreeMap<" : s.startsWith("HashMap<") ? "HashMap<" : null;
  if (mapOpen) {
    const args = unwrapGenerics(s, mapOpen, ">");
    const parts = splitTopLevel(args ?? "", ",");
    if (parts.length === 2) {
      return {
        type: `{ [key in ${rustType(parts[0]).type}]: ${rustType(parts[1]).type} }`,
        nullable: false,
      };
    }
  }
  return { type: PRIMITIVES.get(s) ?? s, nullable: false };
}
/** 按顶层分隔符切（忽略 < > { } [ ] ( ) 内部的分隔符）。 */
function splitTopLevel(text, sep) {
  const out = [];
  let depth = 0;
  let current = "";
  for (const ch of text) {
    if ("<({[".includes(ch)) depth++;
    if (">)}]".includes(ch)) depth--;
    if (ch === sep && depth === 0) {
      out.push(current.trim());
      current = "";
    } else current += ch;
  }
  if (current.trim()) out.push(current.trim());
  return out;
}
function tsType(raw) {
  const s = normType(raw);
  const parts = s.split("|").map((x) => x.trim());
  const nullable = parts.some((p) => /^(null|undefined)$/.test(p));
  const type = parts.filter((p) => !/^(null|undefined)$/.test(p)).join(" | ").trim();
  return { type, nullable };
}

const rustStructs = new Map();
const structNames = new Map(); // name -> [file]，重复 DTO 直接报错，不静默覆盖
for (const file of rustFiles) {
  const text = read(file);
  const re = /\bpub struct (\w+)(?:<[^{;]*>)?\s*\{([\s\S]*?)^\}/gm;
  let m;
  while ((m = re.exec(text)) !== null) {
    const before = text.slice(Math.max(0, m.index - 500), m.index).split(/\n\s*\n/).pop();
    if (!/#\[derive\([^\]]*\b(?:Serialize|TS)\b/.test(before) && !/#\[ts\(export\)\]/.test(before)) {
      continue;
    }
    const [, name, body] = m;
    if (!structNames.has(name)) structNames.set(name, []);
    structNames.get(name).push(rel(file));
    const fields = new Map();
    let rename = null;
    let tsOverride = null;
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
      if (line.startsWith("#[ts(")) {
        const t = /type\s*=\s*"([^"]+)"/.exec(line);
        if (t) tsOverride = t[1];
        continue;
      }
      if (line.startsWith("#[") || line.startsWith("//")) continue;
      // 字段可见性按宪法第 11 条默认私有：同时接受 `pub x:` 与 `x:`。
      const f = /^(?:pub(?:\([^)]*\))?\s+)?(\w+)\s*:\s*(.+?),?\s*$/.exec(line);
      if (!f) continue;
      const shape = tsOverride ? tsType(tsOverride) : rustType(f[2]);
      if (!skip) fields.set(rename ?? f[1], { ...shape, raw: normType(f[2]) });
      rename = null;
      tsOverride = null;
      skip = false;
    }
    rustStructs.set(name, { file: rel(file), fields });
  }
}
for (const [name, filesOfName] of structNames) {
  if (filesOfName.length > 1) {
    errors.push(`DTO 结构体 ${name} 重名（${filesOfName.join("、")}）：按名字索引会静默覆盖，必须改名或明确归属`);
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
  const fields = new Map();
  for (const x of m[2].matchAll(/([A-Za-z_]\w*)(\?)?\s*:\s*([^,;\n]+)/g)) {
    const shape = tsType(x[3]);
    fields.set(x[1], { ...shape, optionalMark: Boolean(x[2]) });
  }
  if (ttStructs.has(m[1])) errors.push(`生成类型 ${m[1]} 在 generated/ 中重名（${ttStructs.get(m[1]).file} 与 ${file}）`);
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
/** fields.json 当前是字符串数组；也接受将来的 `{name,type,optional}` 对象条目。 */
const contractFields = (list) =>
  (list ?? []).map((v) => (typeof v === "string" ? { name: v } : v));
const shapeOf = (f) => (typeof f === "string" ? { type: f, nullable: false } : f);
const gotType = (s) => `${s.type}${s.nullable ? " | null" : ""}`;

for (const [name, list] of Object.entries(contract.rust_structs)) {
  const expected = contractFields(list);
  const found = rustStructs.get(name);
  if (!found) {
    errors.push(`契约登记了 Rust 结构体 ${name}，但代码里找不到（或没有序列化实现）`);
    continue;
  }
  diff(`Rust ${name}`, expected.map((f) => f.name), [...found.fields.keys()], found.file);
  for (const f of expected) {
    if (!f.type || !found.fields.has(f.name)) continue;
    const want = shapeOf(f.type);
    const got = found.fields.get(f.name);
    if (normType(want.type) !== got.type || Boolean(f.optional) !== got.nullable) {
      errors.push(
        `Rust ${name}.${f.name}（${found.file}）类型/可选性不符：契约 ${gotType(want)}，实现 ${gotType(got)}`,
      );
    }
  }
}

for (const [name, list] of Object.entries(contract.ts_interfaces)) {
  const expected = contractFields(list);
  const found = ttStructs.get(name);
  if (!found) {
    errors.push(`契约登记了 TS 类型 ${name}，但 generated/ 里没有 ${name}.ts（跑 npm run bindings）`);
    continue;
  }
  diff(`TS ${name}`, expected.map((f) => f.name), [...found.fields.keys()], found.file);
  for (const f of expected) {
    if (!f.type || !found.fields.has(f.name)) continue;
    const want = shapeOf(f.type);
    const got = found.fields.get(f.name);
    if (normType(want.type) !== got.type || Boolean(f.optional) !== got.nullable) {
      errors.push(
        `TS ${name}.${f.name}（${found.file}）类型/可选性不符：契约 ${gotType(want)}，实现 ${gotType(got)}`,
      );
    }
  }
}

let comparedFields = 0;
for (const [name, rust] of rustStructs) {
  const ts = ttStructs.get(name);
  if (!ts) continue; // MenuNode 只在 Rust 侧：契约没登记 TS 生成物
  for (const [field, want] of rust.fields) {
    const got = ts.fields.get(field);
    if (!got) continue; // 缺字段已由 diff 报出
    comparedFields++;
    if (normType(want.type) !== normType(got.type) || want.nullable !== got.nullable) {
      errors.push(
        `${name}.${field} 类型/可选性不一致：Rust(${rust.file}) ${gotType(want)} vs TS(${ts.file}) ${gotType(got)}`,
      );
    }
  }
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

// —— 5. 命令清单与参数个数 ——
const declared = new Map(); // name -> 有效参数个数（剔除 Tauri 注入的 State/AppHandle/Window）
for (const file of rustFiles) {
  const text = read(file);
  const re = /#\[tauri::command\][\s\S]{0,400}?pub (?:async )?fn (\w+)\s*\(([\s\S]*?)\)\s*(?:->|\{)/g;
  let m;
  while ((m = re.exec(text)) !== null) {
    const params = splitTopLevel(m[2], ",").filter(Boolean);
    const userParams = params.filter((p) => {
      const type = p.includes(":") ? p.slice(p.indexOf(":") + 1) : "";
      return !/\b(?:State|AppHandle|Window)\s*</.test(type);
    });
    if (declared.has(m[1])) errors.push(`命令 ${m[1]} 在 ${rel(file)} 重复实现`);
    declared.set(m[1], userParams.length);
  }
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

// ipc-commands.md 的「参数」列是契约的一部分：Rust 新增/删参数必须同步改契约（走 changes/）
const declaredArity = new Map();
for (const line of read(join(ROOT, "contracts/ipc-commands.md")).split(/\r?\n/)) {
  const m = /^\|\s*`([a-z_][a-z0-9_]*)`\s*\|([^|]*)\|/.exec(line);
  if (!m) continue;
  const params = m[2].replace(/`/g, "").trim();
  declaredArity.set(
    m[1],
    /^(?:—|-|无)?$/.test(params)
      ? 0
      : params.split(/[,，、]/).map((s) => s.trim()).filter(Boolean).length,
  );
}

const expectedCmds = new Set(contract.commands);
let comparedArity = 0;
for (const cmd of expectedCmds) {
  if (!declared.has(cmd)) errors.push(`契约列了命令 ${cmd}，但没有 #[tauri::command] 实现`);
  if (!registered.includes(cmd)) errors.push(`命令 ${cmd} 没有在 generate_handler! 注册，前端会报 command not found`);
  if (!declaredArity.has(cmd)) {
    errors.push(`命令 ${cmd} 在 contracts/ipc-commands.md 缺少参数登记`);
  } else if (declared.has(cmd) && declared.get(cmd) !== declaredArity.get(cmd)) {
    errors.push(
      `命令 ${cmd} 参数个数不符：Rust ${declared.get(cmd)} 个，契约 ipc-commands.md ${declaredArity.get(cmd)} 个（Tauri 注入的 state 不计）`,
    );
  }
  if (declared.has(cmd) && declaredArity.has(cmd)) comparedArity++;
}
for (const cmd of declared.keys()) {
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
  `✓ check-contracts 通过（${Object.keys(contract.rust_structs).length} 个结构体、${comparedFields} 个字段类型/可选性一致、` +
    `${actualTypes.size} 个生成类型、${expectedCmds.size} 条命令/${comparedArity} 个参数签名一致）`,
);
