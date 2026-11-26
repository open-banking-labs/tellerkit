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
  const cfg = JSON.parse(readFileSync(CONFIG, "utf8"));
  if (!Array.isArray(cfg.terms) || !cfg.terms.length) {
    console.error("✗ tools/bank-names.json 的 terms 为空");
    process.exit(2);
  }
  return cfg;
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

for (const file of gitFiles()) {
  const key = file.replace(/\\/g, "/");
  if (allow.has(key)) continue;
  if (SKIP.some((re) => re.test(key))) continue;
  if (!TEXT_EXT.has(extname(key).toLowerCase())) continue;
  const abs = join(ROOT, file);
  if (!existsSync(abs)) continue;
  const st = statSync(abs);
  if (!st.isFile() || st.size > 2 * 1024 * 1024) continue;
  const buf = readFileSync(abs);
  if (buf.includes(0)) continue;
  const lines = buf.toString("utf8").split(/\r?\n/);
  scanned++;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!line) continue;
    for (const { text, re } of patterns) {
      re.lastIndex = 0;
      if (!re.test(line)) continue;
      hits.push({ file: key, line: i + 1, term: text, text: line.trim().slice(0, 120) });
    }
  }
}

if (!hits.length) {
  console.log(`✓ check-bank-names 通过（扫描 ${scanned} 个文本文件，未发现具体银行名称/缩写）`);
  process.exit(0);
}

console.error(`✗ check-bank-names 失败：发现 ${hits.length} 处具体银行名称/缩写\n`);
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
