#!/usr/bin/env node
/**
 * 门禁 · 具体银行名称/缩写
 *
 * 为什么是门禁而不是口头约定：客户名、案例名、评审标题一旦进仓库，
 * 就会随 PR、镜像、安装包一起扩散；事后清理要改历史。所以每次提交前查。
 *
 * 规则：
 *   - 中文词按字面匹配（简称与全称都算）；
 *   - 3 个字母以内的英文缩写只匹配大写独立词（避免哈希/变量名误伤）；
 *   - 4 个字母以上的英文缩写不区分大小写（覆盖全小写写法）；
 *   - 命中不自动改写：改写成“大型银行 / 某银行 / 监管”等泛称，或
 *     在 tools/bank-names.json 的 allow 里登记路径与理由。
 *
 * 用法：node tools/check-bank-names.mjs
 */
import { readFileSync, existsSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { extname, join } from "node:path";
import { ROOT, rel } from "./paths.mjs";

const CONFIG = join(ROOT, "tools/bank-names.json");
const TEXT_EXT = new Set([
  ".md",
  ".markdown",
  ".txt",
  ".html",
  ".vue",
  ".ts",
  ".tsx",
  ".js",
  ".mjs",
  ".cjs",
  ".rs",
  ".toml",
  ".json",
  ".yml",
  ".yaml",
  ".sh",
  ".ps1",
  ".css",
  ".sql",
  ".xml",
]);
/** 锁文件与生成物里全是哈希，扫它们只会制造误报。 */
const SKIP = [
  /^package-lock\.json$/,
  /^Cargo\.lock$/,
  /^pnpm-lock\.yaml$/,
  /^apps\/shell\/src\/generated\//,
  /^apps\/shell\/src-tauri\/gen\//,
  /^apps\/shell\/src\/fixtures\//,
];

function loadConfig() {
  if (!existsSync(CONFIG)) {
    console.error("✗ 缺少 tools/bank-names.json：无法执行银行名称检查");
    process.exit(2);
  }
  let cfg;
  try {
    cfg = JSON.parse(readFileSync(CONFIG, "utf8"));
  } catch (e) {
    console.error(`✗ tools/bank-names.json 不是合法 JSON：${e.message}`);
    process.exit(2);
  }
  if (!Array.isArray(cfg.terms) || !cfg.terms.length) {
    console.error("✗ tools/bank-names.json 的 terms 为空");
    process.exit(2);
  }
  const allow = cfg.allow ?? [];
  if (!Array.isArray(allow)) {
    console.error("✗ tools/bank-names.json 的 allow 必须是数组");
    process.exit(2);
  }
  for (const [i, entry] of allow.entries()) {
    if (
      !entry ||
      typeof entry !== "object" ||
      typeof entry.path !== "string" ||
      !entry.path.trim() ||
      typeof entry.reason !== "string" ||
      !entry.reason.trim()
    ) {
      console.error(`✗ tools/bank-names.json 的 allow[${i}] 缺少非空 path 或 reason：豁免必须写理由`);
      process.exit(2);
    }
  }
  return cfg;
}

/** 中英文之间空格、全角/半角空格都不该让禁用词漏网。 */
function normalize(text) {
  return text
    .replace(/[　   -   ﻿]/g, " ")
    .replace(/([^\x00-\x7F])\s+([A-Za-z0-9])/g, "$1$2")
    .replace(/([A-Za-z0-9])\s+([^\x00-\x7F])/g, "$1$2")
    .replace(/\s/g, "");
}

function escapeRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function compile(term) {
  const text = typeof term === "string" ? term : term.text;
  const ascii = /^[\x20-\x7e]+$/.test(text);
  if (!ascii) return new RegExp(escapeRe(text), "g");
  const upperOnly = /^[A-Z]{1,3}$/.test(text);
  return new RegExp(
    `(?<![A-Za-z0-9])${escapeRe(text)}(?![A-Za-z0-9])`,
    upperOnly ? "g" : "gi",
  );
}

function gitFiles() {
  const run = spawnSync(
    "git",
    ["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    { cwd: ROOT, encoding: "utf8", maxBuffer: 32 * 1024 * 1024 },
  );
  if (run.status !== 0) {
    console.error("✗ 无法列出仓库文件（需要在 git 仓库根目录运行）");
    process.exit(2);
  }
  return run.stdout.split("\0").filter(Boolean);
}

const cfg = loadConfig();
const patterns = cfg.terms.map((t) => ({ text: typeof t === "string" ? t : t.text, re: compile(t) }));
const allow = new Set((cfg.allow ?? []).map((a) => String(a.path).replace(/\\/g, "/")));

const hits = [];
let scanned = 0;
let skippedDirFiles = 0;
let skippedLarge = 0;

for (const file of gitFiles()) {
  const key = file.replace(/\\/g, "/");
  if (allow.has(key)) continue;
  if (SKIP.some((re) => re.test(key))) {
    if (key.includes("/")) skippedDirFiles++;
    continue;
  }
  if (!TEXT_EXT.has(extname(key).toLowerCase())) continue;
  const abs = join(ROOT, file);
  if (!existsSync(abs)) continue;
  const st = statSync(abs);
  if (!st.isFile()) continue;
  if (st.size > 2 * 1024 * 1024) {
    skippedLarge++;
    continue;
  }
  const buf = readFileSync(abs);
  if (buf.includes(0)) continue;
  const lines = buf.toString("utf8").split(/\r?\n/);
  scanned++;

  for (let i = 0; i < lines.length; i++) {
    const line = normalize(lines[i]);
    if (!line) continue;
    for (const { text, re } of patterns) {
      re.lastIndex = 0;
      if (!re.test(line)) continue;
      hits.push({ file: key, line: i + 1, term: text, text: lines[i].trim().slice(0, 120) });
    }
  }
}
const skipInfo = `跳过目录内文件 ${skippedDirFiles} 个、大文件 ${skippedLarge} 个`;

if (!hits.length) {
  console.log(
    `✓ check-bank-names 通过（扫描 ${scanned} 个文本文件，${skipInfo}，未发现具体银行名称/缩写）`,
  );
  process.exit(0);
}

console.error(`✗ check-bank-names 失败：发现 ${hits.length} 处具体银行名称/缩写（${skipInfo}）\n`);
for (const h of hits.slice(0, 50)) {
  console.error(`  ${h.file}:${h.line}  [${h.term}]  ${h.text}`);
}
if (hits.length > 50) console.error(`  … 另有 ${hits.length - 50} 处未列出`);
console.error(`
改写要求：
  1. 客户/案例/评审/截图说明一律用泛称：大型银行、商业银行、某银行、营业网点；
  2. 法规原文用“监管 317 号文”这类不带机构名的写法；
  3. 确需保留的原文，在 tools/bank-names.json 的 allow 中登记路径与理由，评审时说明。
`);
process.exit(1);
