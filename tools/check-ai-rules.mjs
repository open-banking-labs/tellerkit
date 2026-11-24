#!/usr/bin/env node
/**
 * 门禁 3/3 · AI 规则与仓库形态
 *
 * 对应宪法第 3/6/10 条：文档必须在场、规则必须可查、代码里不许留调试残留、
 * 端侧 AI 不许偷偷实现。另外守住 monorepo 的形态本身——形态破了，
 * 后面的门禁作用域就会静默失效。
 */
import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join } from "node:path";
import { ROOT, rel, units, frontendFiles, rustFiles } from "./paths.mjs";

const errors = [];
const notes = [];
const read = (p) => readFileSync(p, "utf8");

function walkAll(dir, out = []) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return out;
  }
  for (const name of entries) {
    if (["node_modules", "target", ".git", "dist", "gen"].includes(name)) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walkAll(p, out);
    else out.push(p);
  }
  return out;
}
const all = walkAll(ROOT);

// 1. 每个单位都要有 AGENTS.md + README.md（AI 进场先读最近的规则）
const REQUIRED = [
  { dir: ROOT, agents: true, readme: false },
  { dir: join(ROOT, "apps/shell/src"), agents: true, readme: false },
  { dir: join(ROOT, "apps/shell/src-tauri"), agents: true, readme: false },
];
for (const u of units) REQUIRED.push({ dir: u, agents: true, readme: true });

for (const r of REQUIRED) {
  if (r.agents && !existsSync(join(r.dir, "AGENTS.md"))) {
    errors.push(`${rel(r.dir) || "."} 缺少 AGENTS.md：Agent 进场没有局部规则可读`);
  }
  if (r.readme && !existsSync(join(r.dir, "README.md"))) {
    errors.push(`${rel(r.dir)} 缺少 README.md：单位职责不明，团队无法分工`);
  }
}

// 2. 形态守卫：旧布局不得回流
for (const legacy of ["src", "src-tauri", "crates"]) {
  if (existsSync(join(ROOT, legacy))) {
    errors.push(`根目录出现 ${legacy}/：仓库已迁到 apps/ + packages/ + tools/，旧布局不得回流`);
  }
}

// 3. 根 package.json 必须是 workspace 根
try {
  const pkg = JSON.parse(read(join(ROOT, "package.json")));
  if (!Array.isArray(pkg.workspaces) || !pkg.workspaces.length) {
    errors.push("根 package.json 没有 workspaces：monorepo 无法按 app 独立构建");
  }
  if (!pkg.private) errors.push("根 package.json 必须 private=true，避免被误发布");
} catch {
  errors.push("根 package.json 不存在或不是合法 JSON");
}

// 4. 每个 app 必须有 tauri.conf.json（主壳身份）
for (const u of units.filter((p) => p.includes(join(ROOT, "apps")))) {
  const conf = join(u, "src-tauri/tauri.conf.json");
  if (!existsSync(conf)) errors.push(`${rel(u)} 缺少 src-tauri/tauri.conf.json`);
  else {
    const c = JSON.parse(read(conf));
    if (!c.build?.frontendDist) errors.push(`${rel(conf)} 缺少 build.frontendDist`);
    if (!c.identifier) errors.push(`${rel(conf)} 缺少 identifier`);
  }
}

// 5. 根 AGENTS.md 要短（长了没人读，AI 会按被截断的上下文行事）
const rootAgents = join(ROOT, "AGENTS.md");
if (existsSync(rootAgents)) {
  const lines = read(rootAgents).split("\n").length;
  if (lines > 60) errors.push(`根 AGENTS.md 有 ${lines} 行，超过 60 行上限：规则要短，细节放 docs/constitution.md`);
  else notes.push(`根 AGENTS.md ${lines} 行`);
}

// 6. 宪法十条必须在场
const constitution = join(ROOT, "docs/constitution.md");
if (!existsSync(constitution)) {
  errors.push("缺少 docs/constitution.md");
} else {
  const text = read(constitution);
  for (let i = 1; i <= 10; i++) {
    if (!text.includes(`## 第 ${i} 条`)) errors.push(`docs/constitution.md 缺少「第 ${i} 条」`);
  }
}

// 7. 安全边界的关键表述不得被删除（对外话术必须统一）
const boundary = join(ROOT, "docs/security-boundary.md");
if (!existsSync(boundary)) {
  errors.push("缺少 docs/security-boundary.md");
} else {
  const text = read(boundary);
  for (const phrase of ["不是安全边界", "本机管理员", "服务端对每一次请求独立鉴权"]) {
    if (!text.includes(phrase)) errors.push(`docs/security-boundary.md 缺少关键表述：「${phrase}」`);
  }
}

// 8. 团队定义与 Agent 预设（AI 主力团队的可执行部分）
const teamDoc = join(ROOT, "docs/team.md");
if (!existsSync(teamDoc)) errors.push("缺少 docs/team.md：团队分工未定义");

const presetDir = join(ROOT, ".agents/presets");
const presets = existsSync(presetDir) ? readdirSync(presetDir).filter((f) => f.endsWith(".md")) : [];
if (presets.length < 4) {
  errors.push(`.agents/presets/ 只有 ${presets.length} 份预设，至少需要 4 份（Schema/Engine/Test/Review）`);
}
for (const f of presets) {
  const text = read(join(presetDir, f));
  for (const key of ["toolFilter", "输出", "验收"]) {
    if (!text.includes(key)) errors.push(`.agents/presets/${f} 缺少「${key}」小节：预设必须声明工具边界、产物和验收方式`);
  }
}

// 8.5 派发记录必须结构完整（W4 的审计凭据）
const dispatchDir = join(ROOT, ".agents/dispatches");
let dispatchCount = 0;
if (existsSync(dispatchDir)) {
  const STATUSES = ["dispatched", "PASS", "FAIL", "OUT_OF_SCOPE"];
  for (const f of readdirSync(dispatchDir).filter((x) => x.endsWith(".json"))) {
    dispatchCount++;
    const where = `.agents/dispatches/${f}`;
    let rec;
    try {
      rec = JSON.parse(read(join(dispatchDir, f)));
    } catch {
      errors.push(`${where} 不是合法 JSON`);
      continue;
    }
    for (const key of ["task_id", "preset", "acceptance_cmd", "prompt_sha256", "status"]) {
      if (!rec[key]) errors.push(`${where} 缺少字段 ${key}`);
    }
    if (!Array.isArray(rec.allowed_files) || rec.allowed_files.length === 0) {
      errors.push(`${where} 的 allowed_files 必须是非空数组：没有白名单就没有裁决依据`);
    }
    if (!STATUSES.includes(rec.status)) {
      errors.push(`${where} 的 status「${rec.status}」非法（应为 ${STATUSES.join(" / ")}）`);
    }
    if (rec.prompt_sha256 && !/^[0-9a-f]{64}$/.test(rec.prompt_sha256)) {
      errors.push(`${where} 的 prompt_sha256 不是 sha256`);
    }
    if (rec.status !== "dispatched" && !rec.verdict) {
      errors.push(`${where} 已裁决但没有 verdict：结论无法复核`);
    }
  }
}

// 9. 端侧 AI 只预留不实现
const AI_TOKENS = ["openai", "anthropic", "chat/completions", "assist.suggestTx", "assist.prefill"];
for (const file of frontendFiles) {
  const text = read(file);
  for (const token of AI_TOKENS) {
    if (text.includes(token)) {
      errors.push(`${rel(file)} 出现「${token}」：端侧 AI 当前只预留契约位置，不写实现（宪法第 10 条）`);
    }
  }
}

// 10. 代码里不得留调试残留（不扫 tools/ 自身：规则字面量会自我误报）
for (const file of all.filter(
  (f) => /\.(rs|ts|vue|mjs)$/.test(f) && !f.includes(join(ROOT, "tools")),
)) {
  const text = read(file);
  text.split("\n").forEach((line, i) => {
    if (/\bdbg!\(/.test(line)) errors.push(`${rel(file)}:${i + 1} 残留 dbg!()`);
    if (/\bconsole\.log\(/.test(line) && frontendFiles.includes(file)) {
      errors.push(`${rel(file)}:${i + 1} 残留 console.log：运行日志走 App.vue 的 log()，不要散落`);
    }
  });
}

// 11. 核心包非测试代码里的 unwrap 提示（不阻断）
for (const file of rustFiles.filter((f) => f.includes(join(ROOT, "packages")))) {
  const text = read(file);
  const idx = text.indexOf("#[cfg(test)]");
  const body = idx >= 0 ? text.slice(0, idx) : text;
  const count = (body.match(/\.unwrap\(\)/g) || []).length;
  if (count > 0) notes.push(`${rel(file)} 非测试代码有 ${count} 处 unwrap()`);
}

if (errors.length) {
  console.error("\n✗ check-ai-rules 失败\n");
  for (const e of errors) console.error("  • " + e);
  console.error(`\n共 ${errors.length} 条。修复建议：补文档/补预设，或删掉越界的实现。\n`);
  process.exit(1);
}

console.log(
  `✓ check-ai-rules 通过（${units.length} 个单位、${presets.length} 份 Agent 预设、${dispatchCount} 条派发记录、宪法 10 条、安全边界表述齐全）`,
);
for (const n of notes) console.log("  · " + n);
