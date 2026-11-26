#!/usr/bin/env node
/**
 * 门禁 8/8 · 模块深度与可见性（宪法第 11 条、ADR-0009）
 *
 * 默认观测模式：打印计数与 Top 清单，exit 0。
 * `--strict`：任一指标比 `tools/module-depth-baseline.json` 记录的既有违规更差就 exit 1。
 *
 * 指标：
 *   1. `pub mod` 平铺数 vs `pub(crate)` / 私有 `mod` 数
 *   2. 含 `pub` 字段的结构体（含 `pub struct X(pub T)` 元组字段）
 *   3. 跨模块字段访问（启发式：字段名唯一归属时，其他文件访问它）
 *   4. 单文件 `pub fn` 数（阈值 PUB_FN_LIMIT，基线内超额者也逐文件记账）
 *
 * 基线是「存量债务清单」，不是豁免权：新增违规、或在已有违规文件里加计数都会红。
 * 更新基线必须人工确认（不是 `--update-allow` 静音）。跨模块字段访问是启发式，
 * 只作趋势提示，不阻断。真正的构建裁决仍由 CI 与 deslop-please 给出。
 */
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT, rel, rustFiles } from "./paths.mjs";

const ARGS = process.argv.slice(2);
const STRICT = ARGS.includes("--strict");
const WRITE_BASELINE = ARGS.includes("--write-baseline");
const PUB_FN_LIMIT = 12;
const BASELINE_PATH = join(ROOT, "tools/module-depth-baseline.json");

const read = (p) => readFileSync(p, "utf8");
const stripTest = (text) => {
  const i = text.indexOf("#[cfg(test)]");
  return i >= 0 ? text.slice(0, i) : text;
};
const countByFile = (entries) => {
  const out = {};
  for (const [file, n] of entries) if (n > 0) out[file] = n;
  return out;
};

const files = rustFiles.map((p) => ({ path: p, rel: rel(p).replace(/\\/g, "/"), text: read(p) }));

// —— 1. 模块可见性（按文件记账，便于发现新增平铺） ——
let pubMods = 0;
let crateMods = 0;
const pubModsByFile = [];
for (const f of files) {
  const n = (f.text.match(/^pub mod \w+\s*[{;]/gm) || []).length;
  pubMods += n;
  if (n) pubModsByFile.push([f.rel, n]);
  crateMods += (f.text.match(/^(?:pub\(crate\)|pub\(super\)) mod \w+;|^mod \w+;/gm) || []).length;
}

// —— 2. 结构体字段（命名结构体 + 元组结构体） ——
/** @returns {{name: string, isPub: boolean, isCrate: boolean}[]} */
function parseFields(body, tuple = false) {
  const fields = [];
  if (tuple) {
    for (const raw of body.split(",")) {
      const item = raw.trim();
      if (!item || item.startsWith("#[")) continue;
      const m = /^(pub(?:\([^)]*\))?\s+)/.exec(item);
      if (!m) continue;
      const vis = m[1].trim();
      fields.push({ name: null, isPub: vis === "pub", isCrate: vis.startsWith("pub(") });
    }
    return fields;
  }
  for (const raw of body.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#[") || line.startsWith("//")) continue;
    const fm = /^(pub(?:\([^)]*\))?\s+)?(\w+)\s*:/.exec(line);
    if (!fm) continue;
    const vis = (fm[1] || "").trim();
    fields.push({ name: fm[2], isPub: vis === "pub", isCrate: vis.startsWith("pub(") });
  }
  return fields;
}

const structs = [];
for (const f of files) {
  const text = stripTest(f.text);
  const brace = /\bpub struct (\w+)(?:<[^{;]*>)?\s*\{([^{}]*)\}/g;
  let m;
  while ((m = brace.exec(text)) !== null) {
    structs.push({ name: m[1], file: f.rel, fields: parseFields(m[2]) });
  }
  const tupleRe = /\bpub struct (\w+)(?:<[^()]*>)?\s*\(([^)]*)\)\s*;/g;
  while ((m = tupleRe.exec(text)) !== null) {
    structs.push({ name: m[1], file: f.rel, fields: parseFields(m[2], true), tuple: true });
  }
}

const pubFieldCounts = new Map();
for (const s of structs) {
  const n = s.fields.filter((x) => x.isPub).length;
  if (n) pubFieldCounts.set(s.file, (pubFieldCounts.get(s.file) || 0) + n);
}
const pubFieldStructs = structs
  .map((s) => ({
    name: s.name,
    file: s.file,
    pub: s.fields.filter((x) => x.isPub).length,
    crate: s.fields.filter((x) => x.isCrate).length,
    total: s.fields.length,
  }))
  .filter((s) => s.pub > 0)
  .sort((a, b) => b.pub - a.pub);

// —— 3. 跨模块字段访问（启发式） ——
const fieldOwners = new Map();
for (const s of structs) {
  for (const f of s.fields) {
    if (!f.isPub || !f.name) continue;
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

// —— 4. 单文件 pub fn（排除与字段同名的 getter / set_<field>） ——
const fieldsByFile = new Map();
for (const s of structs) {
  if (!fieldsByFile.has(s.file)) fieldsByFile.set(s.file, new Set());
  for (const f of s.fields) if (f.name) fieldsByFile.get(s.file).add(f.name);
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

const metrics = {
  pub_fields: countByFile(pubFieldCounts),
  pub_fns: countByFile(fnCounts.filter((x) => x.n > PUB_FN_LIMIT).map((x) => [x.file, x.n])),
  pub_mods: countByFile(pubModsByFile),
};

if (WRITE_BASELINE) {
  writeFileSync(
    BASELINE_PATH,
    JSON.stringify(
      {
        $comment:
          "模块深度存量债务基线：逐文件计数。strict 只在计数变差时失败；降低计数请同步更新本文件并说明原因。",
        pub_fields: metrics.pub_fields,
        pub_fns: metrics.pub_fns,
        pub_mods: metrics.pub_mods,
      },
      null,
      2,
    ) + "\n",
  );
  console.log(`✓ 已写入基线 tools/module-depth-baseline.json（请人工复核后提交）\n`);
  process.exit(0);
}

let baseline;
try {
  baseline = JSON.parse(read(BASELINE_PATH));
  for (const key of Object.keys(metrics)) {
    if (!baseline[key] || typeof baseline[key] !== "object" || Array.isArray(baseline[key])) {
      throw new Error(`缺少 ${key} 计数表`);
    }
    // Windows 上 rel() 产出反斜杠，基线统一按 POSIX 路径记账。
    baseline[key] = Object.fromEntries(
      Object.entries(baseline[key]).map(([k, v]) => [k.replace(/\\/g, "/"), v]),
    );
  }
} catch (e) {
  console.error(`✗ check-module-depth 基线不可用（${BASELINE_PATH}）：${e.message}`);
  process.exit(1);
}

const baselineSum = (key) => Object.values(baseline[key]).reduce((a, b) => a + Number(b), 0);
console.log("\n· 模块深度（宪法第 11 条）");
console.log(`  平铺 pub mod：${pubMods}   pub(crate)/私有 mod：${crateMods}`);
console.log(
  `  基线：纯 pub 字段 ${baselineSum("pub_fields")}；pub fn 超限文件 ${Object.keys(baseline.pub_fns).length}；平铺 pub mod ${baselineSum("pub_mods")}（${rel(BASELINE_PATH)}）`,
);
console.log(`  含纯 pub 字段的结构体：${pubFieldStructs.length} / ${structs.length}（pub(crate) ${structs.filter((s) => s.fields.some((x) => x.isCrate)).length}）`);
for (const s of pubFieldStructs.slice(0, 8)) {
  console.log(`    ${s.name}${s.tuple ? "(元组)" : ""} (${s.file}) pub ${s.pub}/${s.total}`);
}
if (leaks.length) {
  console.log(`  跨模块字段访问（启发式，仅趋势）：${leaks.length} 处`);
  for (const l of leaks.slice(0, 10)) {
    console.log(`    .${l.field}  ${l.file}  (定义于 ${l.owner}) ×${l.hits}`);
  }
}
const over = fnCounts.filter((x) => x.n > PUB_FN_LIMIT);
if (over.length) {
  console.log(`  pub fn > ${PUB_FN_LIMIT} 的文件：`);
  for (const x of over) {
    const was = baseline.pub_fns[x.file.replace(/\\/g, "/")] ?? 0;
    console.log(`    ${x.file}: ${x.n}${was ? `（基线 ${was}）` : ""}`);
  }
}

const failures = [];
if (STRICT) {
  const labels = {
    pub_fields: "纯 pub 字段",
    pub_fns: `pub fn > ${PUB_FN_LIMIT}`,
    pub_mods: "顶层 pub mod",
  };
  for (const [key, label] of Object.entries(labels)) {
    for (const [file, n] of Object.entries(metrics[key])) {
      const was = Number(baseline[key][file.replace(/\\/g, "/")] ?? 0);
      if (n > was) failures.push(`${label}：${file} 现在 ${n}，基线 ${was}`);
    }
  }
}

if (failures.length) {
  console.error("\n✗ check-module-depth --strict 失败（比基线更差）\n");
  for (const e of failures) console.error("  • " + e);
  console.error("\n修复方向：字段私有 + 构造函数/访问器；按能力域收口 pub 面。见 ADR-0009。\n");
  process.exit(1);
}

console.log(
  `✓ check-module-depth ${STRICT ? "严格模式通过（未新增违规）" : "观测模式通过（未阻断；gate 使用 --strict）"}\n`,
);
