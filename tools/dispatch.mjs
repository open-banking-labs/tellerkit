#!/usr/bin/env node
/**
 * 派发器：把「预设 + 任务 + 场景」编译成一份可直接交给子 Agent 的提示词，
 * 并留下一条可审计的派发记录。
 *
 * 为什么不是"编排器"：编排由宿主（DeepSeek Harness 的 subagent）承担，
 * 这里只解决两件它解决不了的事——
 *   1. 派活的输入必须可复现（同一任务永远得到同一份约束文本）
 *   2. 派活必须留痕（谁、基于哪个 commit、被允许改什么、最后判成什么）
 *
 * 边界：本工具**不调用模型**，只产出提示词与记录；裁决由 run-task.mjs 负责。
 */
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { posix } from "./paths.mjs";
import { fileURLToPath } from "node:url";
import { parseTasksFile, TasksParseError } from "./lib/tasks.mjs";

// Windows 上 URL().pathname 会给出 /C:/...，必须用 fileURLToPath
const ROOT = fileURLToPath(new URL("..", import.meta.url));
const DISPATCH_DIR = join(ROOT, ".agents/dispatches");
const USAGE = `用法:
  node tools/dispatch.mjs --tasks <tasks.md> --id T-01 [选项]

选项:
  --tasks <path>     tasks.md 路径（必填）
  --id <T-NN>        要派发的任务（必填）
  --preset <name>    预设名（默认按 layer 推断；预设位于 .agents/presets/）
  --emit prompt|json 输出提示词（默认）或结构化记录
  --record           写入 .agents/dispatches/<utc>-<id>.json
  --adjudicate       派发后立即用 run-task.mjs 裁决，并把结果写回记录
  --base <ref>       裁决基线（默认 origin/main）
  -h, --help         显示本帮助

退出码: 0 成功 · 1 裁决失败 · 2 越界 · 3 拒绝派发（如 contract 层）· 64 用法错误`;

function fatal(msg, code = 64) {
  console.error(`✗ ${msg}`);
  process.exit(code);
}

function parseArgs(argv) {
  const out = { emit: "prompt", record: false, adjudicate: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "-h" || a === "--help") {
      console.log(USAGE);
      process.exit(0);
    }
    const need = (name) => {
      const v = argv[++i];
      if (!v || v.startsWith("--")) fatal(`${name} 需要一个值`);
      return v;
    };
    if (a === "--tasks") out.tasks = need("--tasks");
    else if (a === "--id") out.id = need("--id");
    else if (a === "--preset") out.preset = need("--preset");
    else if (a === "--base") out.base = need("--base");
    else if (a === "--emit") {
      out.emit = need("--emit");
      if (!["prompt", "json"].includes(out.emit)) fatal("--emit 只能是 prompt 或 json");
    } else if (a === "--record") out.record = true;
    else if (a === "--adjudicate") out.adjudicate = true;
    else fatal(`未知参数：${a}`);
  }
  if (!out.tasks) fatal("缺少 --tasks");
  if (!out.id) fatal("缺少 --id");
  return out;
}

/** contract 层只能由人走提案：把它派给 Agent 等于让 AI 改契约。 */
function presetForLayer(layer, explicit) {
  if (explicit) return explicit;
  if (layer === "contract") {
    fatal(
      "contract 层的任务不能派发给 Agent：契约变更只能由人走 changes/ 提案（宪法第 1 条）。",
      3,
    );
  }
  return "engine-agent";
}

/** 从 spec.md 里取出被引用的场景正文（GIVEN/WHEN/THEN 代码块）。 */
function scenariosFrom(specPath, wanted) {
  if (!existsSync(specPath)) return [];
  const text = readFileSync(specPath, "utf8");
  const blocks = [];
  const re = /^### (S-\d+)\s*(.*)$/gm;
  const hits = [...text.matchAll(re)];
  for (let i = 0; i < hits.length; i++) {
    const id = hits[i][1];
    if (!wanted.includes(id)) continue;
    const start = hits[i].index + hits[i][0].length;
    const end = i + 1 < hits.length ? hits[i + 1].index : text.length;
    const body = text.slice(start, end).trim();
    blocks.push({ id, title: hits[i][2].trim(), body });
  }
  return blocks;
}

function composePrompt({ preset, presetText, task, scenarios, tasksRel, specRel }) {
  const scenarioBlock = scenarios.length
    ? scenarios
        .map((s) => `### ${s.id} ${s.title}\n\n${s.body}`)
        .join("\n\n")
    : "（本任务未引用场景；若你发现缺少验收依据，停下来报告，不要自行发明标准。）";

  return `你是 ${preset}。先读仓库根 AGENTS.md 与你将改动目录最近的 AGENTS.md，
再读下面这份任务单。仓库是 TellerKit（Tauri + Rust + Vue 的柜面终端平台）。

## 预设职责

${presetText.trim()}

## 任务

- 来源：\`${tasksRel}\` · ${task.id}
- 内容：${task.title}
- 层次：${task.layer}

## 场景（必须满足）

${scenarioBlock}

## 允许改动（硬约束）

${task.allowed.map((p) => `- \`${p}\``).join("\n")}

只能改上面这些文件。改到别处，执行器会判 \`OUT_OF_SCOPE\`——命令跑绿了也算失败。
如果任务确实需要额外文件，**停下来报告**，不要自行扩大范围。

## 验收命令（由工具执行，不是由你执行）

\`\`\`
${task.command}
\`\`\`

不要自报"已通过"。结论只来自这条命令的真实 exit code 与 git 改动比对。

## 禁止

- \`git commit\` / \`git push\`（提交由人做）
- 修改 \`contracts/\`、\`docs/adr/\`、\`docs/constitution.md\`、\`.agents/presets/\`
- 修改本任务清单之外的文件（包括顺手"优化"）
- 为了让验收通过而改测试断言或改验收命令

## 完成时回报

1. 改了哪些文件、每个改动为什么必要
2. 你对验收命令结果的判断（预期通过/不通过，以及依据）
3. 你认为验收命令覆盖不到的风险

> 规格：\`${specRel}\`
`;
}

/** 文件内容哈希；读不到返回 null（记录仍可写，只是不可复算）。 */
function sha256Of(path) {
  try {
    const text = readFileSync(path, "utf8").replace(/\r\n/g, "\n");
    return createHash("sha256").update(text).digest("hex");
  } catch {
    return null;
  }
}

function latestRecordFor(taskId) {
  if (!existsSync(DISPATCH_DIR)) return null;
  const files = readdirSync(DISPATCH_DIR)
    .filter((f) => f.endsWith(`-${taskId}.json`))
    .sort();
  return files.length ? join(DISPATCH_DIR, files[files.length - 1]) : null;
}

function gitSha(ref) {
  const r = spawnSync("git", ["rev-parse", ref], { cwd: ROOT, encoding: "utf8" });
  return r.status === 0 ? r.stdout.trim() : null;
}

const opts = parseArgs(process.argv.slice(2));
const tasksPath = resolve(opts.tasks);
const tasksRel = posix(relative(ROOT, tasksPath));

let tasks;
try {
  tasks = parseTasksFile(tasksPath, (p) => readFileSync(p, "utf8"));
} catch (e) {
  if (e instanceof TasksParseError) fatal(e.message);
  throw e;
}

const task = tasks.find((t) => t.id === opts.id);
if (!task) fatal(`在 ${tasksRel} 里找不到任务 ${opts.id}`);

const preset = presetForLayer(task.layer, opts.preset);
const presetPath = join(ROOT, ".agents/presets", `${preset}.md`);
if (!existsSync(presetPath)) {
  fatal(`没有预设 ${preset}：可用的是 ${readdirSync(join(ROOT, ".agents/presets")).filter((f) => f.endsWith(".md")).map((f) => f.replace(/\.md$/, "")).join(" / ")}`);
}

const specPath = join(dirname(tasksPath), "spec.md");
const specRel = posix(relative(ROOT, specPath));
const scenarios = scenariosFrom(specPath, task.scenarios);
const missing = task.scenarios.filter((s) => !scenarios.some((b) => b.id === s));
if (missing.length) {
  fatal(`任务 ${task.id} 引用了场景 ${missing.join(", ")}，但 ${specRel} 里没有对应小节`);
}

const prompt = composePrompt({
  preset,
  presetText: readFileSync(presetPath, "utf8"),
  task,
  scenarios,
  tasksRel,
  specRel,
}).replace(/\r\n/g, "\n");

const record = {
  task_id: task.id,
  tasks_file: tasksRel,
  spec: specRel,
  preset,
  layer: task.layer,
  scenarios: task.scenarios,
  allowed_files: task.allowed,
  acceptance_cmd: task.command,
  base: opts.base || "origin/main",
  base_sha: gitSha(opts.base || "origin/main"),
  prompt_sha256: createHash("sha256").update(prompt).digest("hex"),
  // 输入指纹：任务清单、规格、预设三个文件在派发时刻的内容哈希。
  // 有了它，轨迹门禁才能区分"输入变了导致记录失效"与"有人改了记录"。
  inputs: {
    tasks_sha256: sha256Of(tasksPath),
    spec_sha256: sha256Of(specPath),
    preset_sha256: sha256Of(presetPath),
  },
  created_at: new Date().toISOString(),
  status: "dispatched",
  verdict: null,
};

let recordPath = null;
if (opts.record || opts.adjudicate) {
  mkdirSync(DISPATCH_DIR, { recursive: true });
  const stamp = record.created_at.replace(/[:.]/g, "-");
  recordPath = join(DISPATCH_DIR, `${stamp}-${task.id}.json`);
  writeFileSync(recordPath, JSON.stringify(record, null, 2) + "\n");
}

if (opts.emit === "json") {
  console.log(JSON.stringify(record, null, 2));
} else {
  console.log(prompt);
  if (recordPath) console.error(`\n· 派发记录：${relative(ROOT, recordPath)}`);
}

if (opts.adjudicate) {
  const base = opts.base || "origin/main";
  const r = spawnSync(
    process.execPath,
    [join(ROOT, "tools/run-task.mjs"), "--tasks", tasksPath, "--id", task.id, "--base", base, "--json"],
    { cwd: ROOT, encoding: "utf8" },
  );
  let outcome;
  try {
    outcome = JSON.parse(r.stdout);
  } catch {
    fatal(`裁决输出无法解析，run-task stderr：\n${r.stderr}`);
  }
  const result = outcome.results?.[0];
  record.status = result?.status ?? "FAIL";
  record.verdict = {
    exit_code: result?.exit_code ?? null,
    duration_ms: result?.duration_ms ?? null,
    changed_files: result?.changed_files ?? [],
    violations: result?.violations ?? [],
    finished_at: new Date().toISOString(),
  };
  if (recordPath) writeFileSync(recordPath, JSON.stringify(record, null, 2) + "\n");
  console.error(
    `· 裁决：${record.status}（exit ${record.verdict.exit_code}）` +
      (recordPath ? ` · 记录 ${relative(ROOT, recordPath)}` : ""),
  );
  process.exit(r.status === 0 ? 0 : record.status === "OUT_OF_SCOPE" ? 2 : 1);
}
