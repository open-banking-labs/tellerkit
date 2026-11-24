#!/usr/bin/env node
/**
 * 确定性裁决 · 按 tasks.md 执行验收命令
 *
 * 为什么不认 Agent 自报的"测试已通过"：自报无法复核。这里亲自跑命令拿 exit
 * code，再拿 git 改动与任务白名单比对——两件事都能重放。
 */
import { spawn, spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { constants as osConstants } from "node:os";
import { matchAny } from "./lib/glob.mjs";
import { parseTasksFile, TasksParseError } from "./lib/tasks.mjs";

const DEFAULT_TIMEOUT = 600000;
const TAIL_LINES = 20;
const OUTPUT_CAP = 256 * 1024; // 只留尾部，避免大日志撑爆内存
const USAGE = `用法:
  node tools/run-task.mjs --tasks <tasks.md> (--id T-01 | --all) [选项]

选项:
  --tasks <path>    tasks.md 路径（必填）
  --id <T-NN>       只执行指定任务
  --all             执行表中全部任务
  --base <ref>      改动基线；默认依次尝试 origin/main、main、HEAD
  --timeout <ms>    单条命令超时，默认 ${DEFAULT_TIMEOUT}
  --json            输出单个 JSON 对象，便于接 CI
  -h, --help        显示本帮助

退出码: 0 全部 PASS · 1 有 FAIL · 2 有 OUT_OF_SCOPE · 64 用法或解析错误
判定: 先查越界（白名单外有改动即 OUT_OF_SCOPE），再看命令 exit code。`;

const rawArgs = process.argv.slice(2);
if (rawArgs.includes("-h") || rawArgs.includes("--help")) {
  console.log(USAGE);
  process.exit(0);
}

function fatal(message) {
  // --json 时错误也要机器可读，否则 CI 只能从 stderr 猜
  if (rawArgs.includes("--json")) console.log(JSON.stringify({ error: message }, null, 2));
  else {
    console.error(`✗ ${message}`);
    console.error("  运行 node tools/run-task.mjs --help 查看用法。");
  }
  process.exit(64);
}

function parseArgs(argv) {
  const opts = { tasks: null, id: null, all: false, base: null, json: false, timeout: DEFAULT_TIMEOUT };
  const valueOf = (i, flag) => {
    const v = argv[i];
    if (v === undefined || v.startsWith("--")) fatal(`${flag} 缺少取值`);
    return v;
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--tasks") opts.tasks = valueOf(++i, arg);
    else if (arg === "--id") opts.id = valueOf(++i, arg);
    else if (arg === "--base") opts.base = valueOf(++i, arg);
    else if (arg === "--timeout") {
      const v = valueOf(++i, arg);
      if (!/^\d+$/.test(v) || Number(v) <= 0) fatal(`--timeout 需要正整数毫秒，收到「${v}」`);
      opts.timeout = Number(v);
    } else if (arg === "--all") opts.all = true;
    else if (arg === "--json") opts.json = true;
    else fatal(`未知参数「${arg}」`);
  }
  if (!opts.tasks) fatal("缺少 --tasks <tasks.md>");
  if (opts.id && opts.all) fatal("--id 与 --all 只能二选一");
  if (!opts.id && !opts.all) fatal("必须指定 --id <T-NN> 或 --all");
  return opts;
}

/** 解析失败直接终止：执行器没有"部分执行"的语义。 */
function parseTasks(file) {
  try {
    return parseTasksFile(file, (p) => readFileSync(p, "utf8"));
  } catch (e) {
    if (e instanceof TasksParseError) fatal(e.message);
    throw e;
  }
}

function git(args, root) {
  const r = spawnSync("git", args, { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (r.error) fatal(`无法执行 git：${r.error.message}`);
  return r;
}
function findRoot() {
  const r = git(["rev-parse", "--show-toplevel"], process.cwd());
  if (r.status !== 0) fatal(`当前目录不是 git 仓库：${process.cwd()}。请把工作目录切到仓库内再运行。`);
  return r.stdout.trim();
}
function pickBase(explicit, root) {
  // 默认基线按集成度从高到低试：origin/main 最接近 PR 的真实差异
  for (const ref of explicit ? [explicit] : ["origin/main", "main", "HEAD"]) {
    if (git(["rev-parse", "--verify", "--quiet", `${ref}^{commit}`], root).status === 0) return ref;
  }
  fatal(explicit ? `基线「${explicit}」不存在：先 git fetch，或换一个 --base。` : "找不到基线（已试 origin/main、main、HEAD）：当前仓库还没有提交。");
}
function parseStatusZ(raw) {
  const parts = raw.split("\0");
  const files = [];
  for (let i = 0; i < parts.length; i++) {
    const entry = parts[i];
    if (!entry) continue;
    const xy = entry.slice(0, 2);
    const file = entry.slice(3);
    if (file) files.push(file);
    // -z 下重命名/复制紧跟一条原路径。旧路径也算改动：移出白名单同样是越界
    if (/[RC]/.test(xy)) { const origin = parts[++i]; if (origin) files.push(origin); }
  }
  return files;
}
function changedFiles(root, base) {
  const diff = git(["diff", "--name-only", "--no-renames", "-z", base, "--"], root);
  if (diff.status !== 0) fatal(`git diff ${base} 失败：${(diff.stderr || "").trim()}`);
  // -uall 把未跟踪目录展开成文件，否则 "?? tools/lib/" 无法命中 glob
  const status = git(["status", "--porcelain=v1", "-z", "-uall"], root);
  if (status.status !== 0) fatal(`git status 失败：${(status.stderr || "").trim()}`);
  const files = [...diff.stdout.split("\0"), ...parseStatusZ(status.stdout)].filter(Boolean);
  return [...new Set(files)].sort();
}
const tailLines = (text, n) => {
  const clean = text.replace(/\r\n/g, "\n").replace(/\s+$/, "");
  return clean ? clean.split("\n").slice(-n).join("\n") : "";
};

function runCommand(command, cwd, timeoutMs) {
  return new Promise((done) => {
    const started = performance.now();
    let stdout = "", stderr = "", timedOut = false, settled = false;
    const append = (cur, chunk) => {
      const next = cur + chunk.toString("utf8");
      return next.length > OUTPUT_CAP ? next.slice(-OUTPUT_CAP) : next;
    };
    let child;
    try {
      child = spawn("sh", ["-c", command], {
        cwd,
        env: process.env,
        detached: process.platform !== "win32", // 独立进程组，超时才能连子孙一起杀
        stdio: ["ignore", "pipe", "pipe"],
      });
    } catch (e) {
      done({ exitCode: 127, stdout, stderr: `启动命令失败：${e.message}`, timedOut, durationMs: 0 });
      return;
    }
    const finish = (code, signal) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      const exitCode = timedOut ? 124 : (code ?? (signal ? 128 + (osConstants.signals[signal] ?? 0) : 1));
      done({ exitCode, stdout, stderr, timedOut, durationMs: Math.round(performance.now() - started) });
    };
    const timer = setTimeout(() => {
      timedOut = true;
      try { process.kill(-child.pid, "SIGKILL"); } catch { try { child.kill("SIGKILL"); } catch { /* 已退出 */ } }
    }, timeoutMs);
    child.stdout.on("data", (c) => (stdout = append(stdout, c)));
    child.stderr.on("data", (c) => (stderr = append(stderr, c)));
    child.on("error", (e) => {
      stderr = append(stderr, `\n启动命令失败：${e.message}\n`);
      finish(127, null);
    });
    child.on("close", finish);
  });
}

function judge(task, run, changed, timeoutMs) {
  // 越界优先：范围失控比测试失败更严重，必须先让人看到
  const violations = changed.filter((f) => !matchAny(f, task.allowed));
  const status = violations.length ? "OUT_OF_SCOPE" : run.timedOut || run.exitCode !== 0 ? "FAIL" : "PASS";
  return {
    id: task.id,
    status,
    exit_code: run.exitCode,
    duration_ms: run.durationMs,
    command: task.command,
    changed_files: changed,
    violations,
    stderr_tail: status === "PASS" ? "" : tailLines(run.stderr, TAIL_LINES),
    note: run.timedOut ? `超时：超过 ${timeoutMs}ms，已终止子进程` : "",
  };
}

const advice = (status) =>
  status === "OUT_OF_SCOPE"
    ? "把改动移回「允许改动」范围，或先在 tasks.md 更新白名单并走评审。"
    : "在工作区手动执行上面的命令复现；修好后重跑，本工具不自动重试。";
const duration = (ms) => (ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`);

function printHuman(task, r) {
  console.log(`\n→ ${r.id}  ${task.title}`.trimEnd());
  console.log(`  命令: ${r.command}`);
  console.log(`  exit: ${r.exit_code}${r.note ? `（${r.note}）` : ""}   耗时: ${duration(r.duration_ms)}`);
  if (!r.changed_files.length) console.log("  改动: 0 个文件（工作区干净）");
  else if (!r.violations.length) console.log(`  改动: ${r.changed_files.length} 个文件（全部在允许范围内）`);
  else {
    console.log(`  改动: ${r.changed_files.length} 个文件，其中 ${r.violations.length} 个越界`);
    for (const v of r.violations) console.log(`    - ${v}`);
  }
  if (r.status === "PASS") {
    console.log("  PASS");
    return;
  }
  console.log(`  ${r.status}`);
  if (r.stderr_tail) {
    console.log(`  stderr 末 ${TAIL_LINES} 行:`);
    for (const line of r.stderr_tail.split("\n")) console.log(`    ${line}`);
  }
  console.log(`  建议: ${advice(r.status)}`);
}

const opts = parseArgs(rawArgs);
const tasksPath = resolve(opts.tasks);
const allTasks = parseTasks(tasksPath);
const selected = opts.all ? allTasks : allTasks.filter((t) => t.id === opts.id);
if (!selected.length) fatal(`tasks.md 里没有 ID「${opts.id}」。可用：${allTasks.map((t) => t.id).join("、")}`);

const root = findRoot();
const base = pickBase(opts.base, root);
// 执行前拍快照：验收命令自己可能写生成物，不能让它污染范围判定
const changed = changedFiles(root, base);
const results = [];
for (const task of selected) {
  const run = await runCommand(task.command, root, opts.timeout);
  results.push(judge(task, run, changed, opts.timeout));
}

const summary = { pass: 0, fail: 0, out_of_scope: 0 };
for (const r of results) summary[r.status === "PASS" ? "pass" : r.status === "FAIL" ? "fail" : "out_of_scope"]++;
if (opts.json) {
  console.log(JSON.stringify({ tasks_file: tasksPath, base, results, summary }, null, 2));
} else {
  results.forEach((r, i) => printHuman(selected[i], r));
  if (summary.pass === results.length) console.log(`\n✓ 全部 PASS（${results.length} 个任务）`);
  else {
    console.log(`\n✗ ${results.length} 个任务：PASS ${summary.pass}、FAIL ${summary.fail}、OUT_OF_SCOPE ${summary.out_of_scope}`);
    if (summary.out_of_scope) console.log("  越界优先于命令失败：先处理白名单外的文件。");
  }
}
process.exit(summary.out_of_scope ? 2 : summary.fail ? 1 : 0);
