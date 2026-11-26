#!/usr/bin/env node
/**
 * 门禁 7/7 · 模块深度与可见性（宪法第 11 条、ADR-0009）
 *
 * 默认**观测模式**：打印计数与 Top 清单，exit 0（整改期间允许基线存在）。
 * `--strict`：任一阈值超标 exit 1（整改完成后在 gate 中启用）。
 *
 * 指标：
 *   1. `pub mod` 平铺数 vs `pub(crate)` / 私有 `mod` 数
 *   2. 含 `pub` 字段的结构体（目标 0，DTO 同样私有）
 *   3. 跨模块字段访问（启发式：字段名唯一归属时，其他文件访问它）
 *   4. 单文件 `pub fn` 数（阈值 PUB_FN_LIMIT）
 *
 * 为什么是启发式：精确的字段归属需要 SCIP/编译器索引；本机不跑 cargo，
 * 这里用「字段名唯一归属 + `.field` 访问」近似，作为趋势指标而非精确裁决。
 * 真正的裁决由构建机 `deslop-please` 给出。
 */
import { readFileSync } from "node:fs";
import { rel, rustFiles } from "./paths.mjs";

const STRICT = process.argv.includes("--strict");
const PUB_FN_LIMIT = 12;

const read = (p) => readFileSync(p, "utf8");
const stripTest = (text) => {
  const i = text.indexOf("#[cfg(test)]");
  return i >= 0 ? text.slice(0, i) : text;
};

const files = rustFiles.map((p) => ({ path: p, rel: rel(p), text: read(p) }));

// —— 1. 模块可见性 ——
let pubMods = 0;
let crateMods = 0;
for (const f of files) {
  pubMods += (f.text.match(/^pub mod \w+;/gm) || []).length;
  crateMods += (f.text.match(/^(?:pub\(crate\)|pub\(super\)) mod \w+;|^mod \w+;/gm) || []).length;
}

// —— 2. 结构体字段 ——
const structs = new Map();
for (const f of files) {
  const text = stripTest(f.text);
  const re = /pub struct (\w+)\s*\{([\s\S]*?)^\}/gm;
  let m;
  while ((m = re.exec(text)) !== null) {
    const [, name, body] = m;
    const fields = [];
    for (const raw of body.split("\n")) {
      const line = raw.trim();
      if (!line || line.startsWith("#[") || line.startsWith("//")) continue;
      const fm = /^(pub(?:\([^)]*\))?\s+)?(\w+)\s*:/.exec(line);
      if (fm) fields.push({ name: fm[2], isPub: Boolean(fm[1]) });
    }
    structs.set(name, { file: f.rel, fields });
  }
}

const pubFieldStructs = [...structs.entries()]
  .map(([name, s]) => ({
    name,
    file: s.file,
    pub: s.fields.filter((x) => x.isPub).length,
    total: s.fields.length,
  }))
  .filter((s) => s.pub > 0)
  .sort((a, b) => b.pub - a.pub);

// —— 3. 跨模块字段访问（启发式） ——
const fieldOwners = new Map();
for (const s of structs.values()) {
  for (const f of s.fields) {
    if (!fieldOwners.has(f.name)) fieldOwners.set(f.name, new Set());
    fieldOwners.get(f.name).add(s.file);
  }
}
const uniqueFields = [...fieldOwners.entries()]
  .filter(([, owners]) => owners.size === 1)
  .map(([name]) => name);

const leaks = [];
for (const f of files) {
  const text = stripTest(f.text);
  for (const field of uniqueFields) {
    const owners = fieldOwners.get(field);
    if (owners.has(f.rel)) continue;
    const hits = text.match(new RegExp(`\\.${field}\\b(?!\\s*\\()`, "g"));
    if (hits) leaks.push({ field, file: f.rel, owner: [...owners][0], hits: hits.length });
  }
}
leaks.sort((a, b) => b.hits - a.hits);

// —— 4. 单文件 pub fn（排除与字段同名的 getter / set_<field>，它们不算接口宽度） ——
const fieldsByFile = new Map();
for (const s of structs.values()) {
  if (!fieldsByFile.has(s.file)) fieldsByFile.set(s.file, new Set());
  for (const f of s.fields) fieldsByFile.get(s.file).add(f.name);
}
const fnCounts = files
  .map((f) => {
    const fieldNames = fieldsByFile.get(f.rel) || new Set();
    const fns = [...stripTest(f.text).matchAll(/^\s*pub (?:async )?fn (\w+)/gm)].map((m) => m[1]);
    const accessors = fns.filter(
      (n) => fieldNames.has(n) || (n.startsWith("set_") && fieldNames.has(n.slice(4))),
    );
    return { file: f.rel, n: fns.length - accessors.length };
  })
  .filter((x) => x.n > 0)
  .sort((a, b) => b.n - a.n);

// —— 报告 ——
console.log("\n· 模块深度（宪法第 11 条）");
console.log(`  平铺 pub mod：${pubMods}   pub(crate)/私有 mod：${crateMods}`);
console.log(`  含 pub 字段的结构体：${pubFieldStructs.length} / ${structs.size}`);
if (pubFieldStructs.length) {
  console.log("  Top pub 字段：");
  for (const s of pubFieldStructs.slice(0, 8)) {
    console.log(`    ${s.name} (${s.file}) pub ${s.pub}/${s.total}`);
  }
}
if (leaks.length) {
  console.log(`  跨模块字段访问（启发式）：${leaks.length} 处`);
  for (const l of leaks.slice(0, 10)) {
    console.log(`    .${l.field}  ${l.file}  (定义于 ${l.owner}) ×${l.hits}`);
  }
}
const over = fnCounts.filter((x) => x.n > PUB_FN_LIMIT);
if (over.length) {
  console.log(`  pub fn > ${PUB_FN_LIMIT} 的文件：`);
  for (const x of over) console.log(`    ${x.file}: ${x.n}`);
}

const failures = [];
if (STRICT) {
  if (pubFieldStructs.length) failures.push(`${pubFieldStructs.length} 个结构体仍有 pub 字段（目标 0）`);
  if (over.length) failures.push(`${over.length} 个文件 pub fn 超过 ${PUB_FN_LIMIT}`);
  if (leaks.length) failures.push(`${leaks.length} 处跨模块字段访问`);
}

if (failures.length) {
  console.error("\n✗ check-module-depth 失败\n");
  for (const e of failures) console.error("  • " + e);
  console.error("\n修复方向：字段私有 + 构造函数/访问器；按能力域收口 pub 面。见 ADR-0009。\n");
  process.exit(1);
}

console.log(
  `✓ check-module-depth ${STRICT ? "通过" : "基线已输出（观测模式；整改完成后加 --strict）"}\n`,
);
