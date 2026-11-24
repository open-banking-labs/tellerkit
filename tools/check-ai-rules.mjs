#!/usr/bin/env node
/**
 * 门禁 3/3 · AI 规则
 *
 * 对应宪法第 3/6/10 条：文档必须在场、规则必须可查、代码里不许留调试残留、
 * 端侧 AI 不许偷偷实现。这一道检查的是"约束本身还在不在"。
 */
import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, relative } from "node:path";

const ROOT = new URL("..", import.meta.url).pathname;
const rel = (p) => relative(ROOT, p);
const errors = [];
const notes = [];

function walk(dir, out = []) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return out;
  }
  for (const name of entries) {
    if (["node_modules", "target", ".git", "dist"].includes(name)) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}
const all = walk(ROOT);
const read = (p) => readFileSync(p, "utf8");

// 1. 每个受管目录都要有 AGENTS.md
const REQUIRED_AGENTS = ["", "crates/tellerkit-core", "src-tauri", "src"];
for (const dir of REQUIRED_AGENTS) {
  const p = join(ROOT, dir, "AGENTS.md");
  if (!existsSync(p)) errors.push(`缺少 ${dir || "."}/AGENTS.md：Agent 进场没有局部规则可读`);
}

// 2. 根 AGENTS.md 要短（长了没人读，AI 会按截断的上下文行事）
const rootAgents = join(ROOT, "AGENTS.md");
if (existsSync(rootAgents)) {
  const lines = read(rootAgents).split("\n").length;
  if (lines > 60) errors.push(`根 AGENTS.md 有 ${lines} 行，超过 60 行上限：规则要短，细节放 docs/constitution.md`);
  else notes.push(`根 AGENTS.md ${lines} 行`);
}

// 3. 宪法十条必须在场
const constitution = join(ROOT, "docs/constitution.md");
if (!existsSync(constitution)) {
  errors.push("缺少 docs/constitution.md");
} else {
  const text = read(constitution);
  for (let i = 1; i <= 10; i++) {
    if (!text.includes(`## 第 ${i} 条`)) errors.push(`docs/constitution.md 缺少「第 ${i} 条」`);
  }
}

// 4. 安全边界的关键表述不得被删除（这是对外话术的统一口径）
const boundary = join(ROOT, "docs/security-boundary.md");
if (!existsSync(boundary)) {
  errors.push("缺少 docs/security-boundary.md");
} else {
  const text = read(boundary);
  for (const phrase of ["不是安全边界", "本机管理员", "服务端对每一次请求独立鉴权"]) {
    if (!text.includes(phrase)) errors.push(`docs/security-boundary.md 缺少关键表述：「${phrase}」`);
  }
}

// 5. 端侧 AI 只预留不实现：src/ 里不得出现模型调用或 assist 实现
const AI_TOKENS = ["openai", "anthropic", "chat/completions", "assist.suggestTx", "assist.prefill"];
for (const file of all.filter((f) => f.startsWith(join(ROOT, "src")) && /\.(ts|vue)$/.test(f))) {
  const text = read(file);
  for (const token of AI_TOKENS) {
    if (text.includes(token)) {
      errors.push(`${rel(file)} 出现「${token}」：端侧 AI 当前只预留契约位置，不写实现（宪法第 10 条）`);
    }
  }
}

// 6. 代码里不得留调试残留
// 不扫 tools/ 自身：门禁脚本里必然出现规则字面量（例如 dbg! 的模式串），
// 扫自己会造成自我误报。门禁脚本由人评审，不属 AI 生成产物。
for (const file of all.filter((f) => /\.(rs|ts|vue|mjs)$/.test(f) && !f.includes(join(ROOT, "tools")))) {
  if (file.includes("/target/") || file.includes("/node_modules/")) continue;
  const text = read(file);
  text.split("\n").forEach((line, i) => {
    if (/\bdbg!\(/.test(line)) errors.push(`${rel(file)}:${i + 1} 残留 dbg!()`);
    if (/\bconsole\.log\(/.test(line) && file.includes(join(ROOT, "src"))) {
      errors.push(`${rel(file)}:${i + 1} 残留 console.log：运行日志走 App.vue 的 log()，不要散落`);
    }
  });
}

// 7. 禁止在非测试代码里 unwrap（core 不强制，但提示）
const coreRs = all.filter((f) => f.includes("/crates/") && f.endsWith(".rs"));
for (const file of coreRs) {
  const text = read(file);
  const idx = text.indexOf("#[cfg(test)]");
  const body = idx >= 0 ? text.slice(0, idx) : text;
  const count = (body.match(/\.unwrap\(\)/g) || []).length;
  if (count > 0) notes.push(`${rel(file)} 非测试代码有 ${count} 处 unwrap()`);
}

if (errors.length) {
  console.error("\n✗ check-ai-rules 失败\n");
  for (const e of errors) console.error("  • " + e);
  console.error(`\n共 ${errors.length} 条。修复建议：补文档，或删掉越界的实现。\n`);
  process.exit(1);
}

console.log(`✓ check-ai-rules 通过（AGENTS.md ${REQUIRED_AGENTS.length} 份、宪法 10 条、安全边界表述齐全）`);
for (const n of notes) console.log("  · " + n);
