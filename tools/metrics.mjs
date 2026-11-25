#!/usr/bin/env node
/**
 * 可观测层：把派发记录聚合成指标。
 *
 * 目的不是画仪表盘，而是回答一个具体问题：**打回率上升时，是提示词不好、
 * 任务拆得太粗，还是验收命令本身跑不起来？** 所以输出除了数字，还必须给判断。
 *
 * 数据源：.agents/dispatches/*.json（由 tools/dispatch.mjs 写入）
 * 只读：本工具不修改任何记录。
 */
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { parseTasksFile } from "./lib/tasks.mjs";

// Windows 上 URL().pathname 会给出 /C:/...，必须用 fileURLToPath
const ROOT = fileURLToPath(new URL("..", import.meta.url));
const USAGE = `用法:
  node tools/metrics.mjs [选项]

选项:
  --dir <path>    记录目录（默认 .agents/dispatches）
  --since <日期>   只看该日期之后的记录（ISO 或 YYYY-MM-DD）
  --json          输出结构化结果
  -h, --help      显示本帮助`;

function parseArgs(argv) {
  const out = { dir: join(ROOT, ".agents/dispatches") };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "-h" || a === "--help") {
      console.log(USAGE);
      process.exit(0);
    }
    if (a === "--dir") out.dir = argv[++i];
    else if (a === "--since") out.since = argv[++i];
    else if (a === "--json") out.json = true;
    else {
      console.error(`✗ 未知参数：${a}`);
      process.exit(64);
    }
  }
  return out;
}

const pct = (n, d) => (d === 0 ? null : Math.round((n / d) * 1000) / 10);
const median = (xs) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  const mid = Math.floor(s.length / 2);
  return s.length % 2 ? s[mid] : Math.round((s[mid - 1] + s[mid]) / 2);
};
const p90 = (xs) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.ceil(s.length * 0.9) - 1)];
};

/** 规格里登记的任务总数：用来算"派发记录覆盖率"。 */
function totalTasks() {
  const specsDir = join(ROOT, ".specify", "specs");
  if (!existsSync(specsDir)) return 0;
  let total = 0;
  for (const entry of readdirSync(specsDir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const file = join(specsDir, entry.name, "tasks.md");
    if (!existsSync(file)) continue;
    try {
      total += parseTasksFile(file, (p) => readFileSync(p, "utf8")).length;
    } catch {
      // 解析不了的规格不参与统计，但也不算 0
    }
  }
  return total;
}

function loadRecords(dir) {
  if (!existsSync(dir)) return { records: [], broken: [] };
  const files = readdirSync(dir).filter((f) => f.endsWith(".json"));
  const records = [];
  const broken = [];
  for (const f of files) {
    try {
      records.push(JSON.parse(readFileSync(join(dir, f), "utf8")));
    } catch {
      broken.push(f);
    }
  }
  return { records, broken };
}

/**
 * 追溯检查：记录引用的任务是否还在清单里。
 * 任务被删/改名后，旧记录就成了"无主证据"，必须能看见。
 */
function traceCheck(records) {
  const cache = new Map();
  const issues = [];
  for (const r of records) {
    if (!r.tasks_file) continue;
    if (!cache.has(r.tasks_file)) {
      const abs = join(ROOT, r.tasks_file);
      try {
        cache.set(r.tasks_file, parseTasksFile(abs, (p) => readFileSync(p, "utf8")).map((t) => t.id));
      } catch {
        cache.set(r.tasks_file, null);
      }
    }
    const ids = cache.get(r.tasks_file);
    if (ids === null) {
      issues.push({ task_id: r.task_id, reason: `清单已不存在或无法解析：${r.tasks_file}` });
    } else if (!ids.includes(r.task_id)) {
      issues.push({ task_id: r.task_id, reason: `任务已不在 ${r.tasks_file}（被删或改名）` });
    }
  }
  return issues;
}

function summarize(records) {
  const decided = records.filter((r) => r.status !== "dispatched");
  const byStatus = { PASS: 0, FAIL: 0, OUT_OF_SCOPE: 0, dispatched: 0 };
  for (const r of records) byStatus[r.status] = (byStatus[r.status] ?? 0) + 1;

  const durations = decided.map((r) => r.verdict?.duration_ms).filter((n) => typeof n === "number");
  const rejected = byStatus.FAIL + byStatus.OUT_OF_SCOPE;

  const group = (key) => {
    const map = new Map();
    for (const r of records) {
      const k = r[key] ?? "（未标注）";
      const g = map.get(k) ?? { count: 0, pass: 0, rejected: 0, outOfScope: 0 };
      g.count++;
      if (r.status === "PASS") g.pass++;
      if (r.status === "FAIL") g.rejected++;
      if (r.status === "OUT_OF_SCOPE") {
        g.rejected++;
        g.outOfScope++;
      }
      map.set(k, g);
    }
    return Object.fromEntries([...map.entries()].sort());
  };

  return {
    total: records.length,
    decided: decided.length,
    undecided: byStatus.dispatched,
    by_status: byStatus,
    reject_rate_pct: pct(rejected, decided.length),
    out_of_scope_rate_pct: pct(byStatus.OUT_OF_SCOPE, decided.length),
    duration_ms: {
      median: median(durations),
      p90: p90(durations),
      max: durations.length ? Math.max(...durations) : null,
    },
    by_preset: group("preset"),
    by_layer: group("layer"),
    by_task: group("task_id"),
  };
}

/** 指标必须带判断，否则只是数字堆。 */
function hints(m, traceIssues) {
  const out = [];
  if (m.total === 0) {
    out.push("还没有派发记录：先跑一次 `npm run dispatch -- --tasks … --id … --record`。");
    return out;
  }
  if (m.decided < 5) {
    out.push(`已裁决 ${m.decided} 条，样本不足 5 条：先看单条明细，别对比例下结论。`);
  }
  if (m.undecided > 0) {
    out.push(
      `${m.undecided} 条派发没有裁决：要么任务被遗弃，要么裁决步骤被跳过。未裁决等于没有结论。`,
    );
  }
  if (m.reject_rate_pct !== null && m.reject_rate_pct > 30) {
    out.push(
      `打回率 ${m.reject_rate_pct}% 偏高：先看验收命令是否可执行、再看任务是否拆得太粗（>1 个文件/1 个 trait）。`,
    );
  }
  if (m.out_of_scope_rate_pct !== null && m.out_of_scope_rate_pct > 10) {
    out.push(
      `越界率 ${m.out_of_scope_rate_pct}% 偏高：提示词里的白名单说明可能不够硬，或任务本身需要跨文件却只给了一个文件。`,
    );
  }
  if (m.duration_ms.median !== null && m.duration_ms.median > 600000) {
    out.push("单任务耗时中位数超过 10 分钟：验收命令可能包含全量构建，建议拆出更快的针对性命令。");
  }
  for (const issue of traceIssues) {
    out.push(`记录 ${issue.task_id} 的追溯已失效：${issue.reason}。要么补回任务，要么归档这条记录。`);
  }
  return out;
}

const opts = parseArgs(process.argv.slice(2));
const { records, broken } = loadRecords(opts.dir);
const filtered = opts.since
  ? records.filter((r) => (r.created_at ?? "").slice(0, 10) >= opts.since.slice(0, 10))
  : records;

const metrics = summarize(filtered);
const traceIssues = traceCheck(filtered);
const advice = hints(metrics, traceIssues);

if (opts.json) {
  console.log(
    JSON.stringify(
      { dir: relative(ROOT, opts.dir), broken, metrics, trace_issues: traceIssues, hints: advice },
      null,
      2,
    ),
  );
  process.exit(0);
}

const fmtMs = (n) => (n === null ? "—" : n >= 1000 ? `${(n / 1000).toFixed(1)}s` : `${n}ms`);
const fmtPct = (n) => (n === null ? "—" : `${n}%`);

console.log(`\n派发记录 · ${metrics.total} 条（已裁决 ${metrics.decided} / 未裁决 ${metrics.undecided}）`);
if (broken.length) console.log(`  ⚠ 无法解析：${broken.join(", ")}`);

{
  const tasks = totalTasks();
  const covered = metrics.total;
  const pctText = tasks === 0 ? "—" : `${Math.round((covered / tasks) * 1000) / 10}%`;
  console.log(`  覆盖率      ${covered}/${tasks} 个任务有派发记录（${pctText}）`);
  if (tasks > 0 && covered / tasks < 0.5) {
    console.log("             ⚠ 多数任务没有留痕：轨迹门禁只校验有记录的，别当成全覆盖");
  }
}

console.log(`
  结论分布    PASS ${metrics.by_status.PASS} · FAIL ${metrics.by_status.FAIL} · OUT_OF_SCOPE ${metrics.by_status.OUT_OF_SCOPE}
  打回率      ${fmtPct(metrics.reject_rate_pct)}
  越界率      ${fmtPct(metrics.out_of_scope_rate_pct)}
  耗时        中位 ${fmtMs(metrics.duration_ms.median)} · P90 ${fmtMs(metrics.duration_ms.p90)} · 最长 ${fmtMs(metrics.duration_ms.max)}`);

const table = (title, obj) => {
  const keys = Object.keys(obj);
  if (!keys.length) return;
  console.log(`\n按 ${title}`);
  for (const k of keys) {
    const g = obj[k];
    const rate = g.count ? `${Math.round((g.rejected / g.count) * 100)}%` : "—";
    console.log(`  ${k.padEnd(14)} ${String(g.count).padStart(3)} 条 · 打回 ${rate}`);
  }
};

table("preset", metrics.by_preset);
table("layer", metrics.by_layer);
table("任务", metrics.by_task);

if (advice.length) {
  console.log("\n判断");
  for (const a of advice) console.log(`  · ${a}`);
}
console.log("");
