#!/usr/bin/env node
/**
 * 并行批次校验：分派之前先算清楚"谁改哪些文件"。
 *
 * 为什么需要它：一晚上并行三路子 agent，集成时改了三次签名不匹配
 * （AppState::new 少参数、audit 变量作用域、submit_transaction 借用冲突）。
 * 根因不是 agent 写得差，是我在**分派前没有把接口固化成代码**，
 * 且没人检查三路改动的文件是否真的互不相交。
 *
 * 这个工具解决后半段：给出批次内每个任务的文件主张，标出
 *   1) 被多个任务同时主张的文件（并行冲突）
 *   2) 被派去改"枢纽文件"的任务（本不该并行）
 *
 * 前半段（接口固化）靠 protocol：先提交 seam（签名 + unimplemented!()），
 * 再分派。见 docs/parallel-work.md。
 */
import { readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { ROOT, posix, walk } from "./paths.mjs";
import { parseTasksFile, TasksParseError } from "./lib/tasks.mjs";
import { matchAny } from "./lib/glob.mjs";

/** 这些文件被多处依赖，派给并行任务必然要人工合并。 */
const HOT_FILES = [
  "apps/shell/src-tauri/src/lib.rs",
  "apps/shell/src-tauri/src/commands.rs",
  "apps/shell/src/App.vue",
  "apps/shell/src/api.ts",
  "apps/shell/src/generated/**",
  "contracts/**",
  "package.json",
  "Cargo.toml",
  "packages/core/src/lib.rs",
];

const USAGE = `用法:
  node tools/plan-parallel.mjs --tasks <tasks.md> --ids T-01,T-02 [,T-03]

检查这批任务能否安全并行：文件主张是否互不相交、有没有碰到枢纽文件。
退出码：0 可并行 · 1 有冲突 · 64 用法错误`;

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "-h" || a === "--help") {
      console.log(USAGE);
      process.exit(0);
    }
    if (a === "--tasks") out.tasks = argv[++i];
    else if (a === "--ids") out.ids = argv[++i];
    else {
      console.error(`✗ 未知参数：${a}`);
      process.exit(64);
    }
  }
  if (!out.tasks || !out.ids) {
    console.error("✗ 需要 --tasks 与 --ids\n");
    console.log(USAGE);
    process.exit(64);
  }
  return out;
}

const opts = parseArgs(process.argv.slice(2));
// 支持跨规格的批次：--tasks a/tasks.md,b/tasks.md
const taskFiles = opts.tasks.split(",").map((s) => s.trim()).filter(Boolean);
let tasks = [];
try {
  for (const file of taskFiles) {
    const parsed = parseTasksFile(join(ROOT, file), (p) => readFileSync(p, "utf8"));
    // 记录来源文件：跨规格批次里 T-01 可能重名，靠来源区分
    for (const t of parsed) tasks.push({ ...t, source: file });
  }
} catch (e) {
  if (e instanceof TasksParseError) {
    console.error(`✗ ${e.message}`);
    process.exit(64);
  }
  throw e;
}

const wanted = opts.ids.split(",").map((s) => s.trim()).filter(Boolean);
const picked = wanted.map((raw) => {
  // 支持 `<规格目录>/T-01` 形式；只写 T-01 时要求全局唯一
  const qualified = raw.includes("/")
    ? tasks.find((t) => raw.endsWith(`/${t.id}`) && t.source.includes(raw.slice(0, raw.lastIndexOf("/"))))
    : (() => {
        const hits = tasks.filter((t) => t.id === raw);
        if (hits.length > 1) {
          console.error(`✗ 任务 ID ${raw} 在多个规格里重名，请用 <规格目录>/${raw} 限定`);
          process.exit(64);
        }
        return hits[0];
      })();
  if (!qualified) {
    console.error(`✗ 找不到任务 ${raw}`);
    process.exit(64);
  }
  return qualified;
});

if (picked.length < 2) {
  console.log("· 只有一个任务，谈不上并行；直接执行即可。");
  process.exit(0);
}

// 用仓库现有文件做具体化：glob 是否相交，靠"当前有哪些文件同时命中"判断
const repoFiles = walk(ROOT)
  .map((f) => posix(relative(ROOT, f)))
  .filter((f) => !f.startsWith("target/") && !f.startsWith("node_modules/"));

const claims = new Map(); // file -> task ids
for (const task of picked) {
  for (const file of repoFiles) {
    if (matchAny(file, task.allowed)) {
      const list = claims.get(file) ?? [];
      list.push(task.id);
      claims.set(file, list);
    }
  }
}

const conflicts = [...claims.entries()].filter(([, ids]) => ids.length > 1);
const hot = picked
  .map((task) => ({ task, hits: task.allowed.filter((g) => matchAny(g, HOT_FILES) || HOT_FILES.some((h) => matchAny(h, [g])) ) }))
  .filter((x) => x.hits.length);

const created = picked.map((task) => {
  // 任务声明的新文件（当前不存在、但会被创建）
  const files = repoFiles.filter((f) => matchAny(f, task.allowed));
  return { id: task.id, files: files.length, globs: task.allowed };
});

console.log(`\n并行批次：${picked.map((t) => t.id).join(" + ")}`);
for (const c of created) {
  console.log(`  ${c.id.padEnd(6)} 主张 ${String(c.files).padStart(3)} 个现有文件 · ${c.globs.join(", ")}`);
}

let failed = false;

if (conflicts.length) {
  failed = true;
  console.log("\n✗ 文件主张相交（并行会打架）：");
  for (const [file, ids] of conflicts.slice(0, 20)) {
    console.log(`  • ${file}  ← ${ids.join(" / ")}`);
  }
  if (conflicts.length > 20) console.log(`  … 还有 ${conflicts.length - 20} 个`);
  console.log("  处理：拆成串行，或把共享部分先固化成 seam（见 docs/parallel-work.md）");
}

if (hot.length) {
  failed = true;
  console.log("\n✗ 有任务在改枢纽文件（不该并行）：");
  for (const h of hot) {
    console.log(`  • ${h.task.id} 命中 ${h.hits.join(", ")}`);
  }
  console.log("  处理：枢纽文件由集成者串行改，或先提交 seam 再分派");
}

if (!failed) {
  console.log("\n✓ 可并行：文件主张互不相交，且都没碰枢纽文件");
  console.log("  提醒：先确认各自的 seam 已提交（签名 + unimplemented!()），再分派");
}

process.exit(failed ? 1 : 0);
