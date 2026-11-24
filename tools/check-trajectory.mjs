#!/usr/bin/env node
/**
 * 门禁 · 派发轨迹
 *
 * 区别于 check-ai-rules：那道只校验记录**结构**，这道校验记录**是否可信**。
 *
 * 做法是**复算**，不是读记录里的自报字段：
 *   1. 用当前任务清单重新编译提示词，比对 sha256 —— 任务或预设被改过就必须红；
 *   2. PASS 的记录不得有越界文件；
 *   3. OUT_OF_SCOPE 的记录必须有越界文件（否则判定与证据矛盾）。
 *
 * 这就是"轨迹门禁"的最小可用形态：不采集会话日志，只保证
 * 「记录里写的结论」与「现在还能复现的事实」一致。
 */
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { ROOT, rel } from "./paths.mjs";

const DIR = join(ROOT, ".agents/dispatches");
const errors = [];
const notes = [];

if (!existsSync(DIR)) {
  console.log("✓ check-trajectory 通过（还没有派发记录）");
  process.exit(0);
}

const files = readdirSync(DIR).filter((f) => f.endsWith(".json"));
let checked = 0;

for (const file of files) {
  const path = join(DIR, file);
  const where = `.agents/dispatches/${file}`;
  let record;
  try {
    record = JSON.parse(readFileSync(path, "utf8"));
  } catch {
    errors.push(`${where} 不是合法 JSON`);
    continue;
  }

  const verdict = record.verdict;
  const violations = verdict?.violations ?? [];

  // 2/3：判定与证据必须一致
  if (record.status === "PASS" && violations.length > 0) {
    errors.push(`${where} 判为 PASS 却有 ${violations.length} 个越界文件：${violations.join(", ")}`);
  }
  if (record.status === "OUT_OF_SCOPE" && violations.length === 0) {
    errors.push(`${where} 判为 OUT_OF_SCOPE 却没有记录任何越界文件：判定与证据矛盾`);
  }

  // 1：复算提示词指纹
  if (!record.tasks_file || !record.task_id) {
    errors.push(`${where} 缺少 tasks_file 或 task_id，无法复算`);
    continue;
  }
  const tasksPath = join(ROOT, record.tasks_file);
  if (!existsSync(tasksPath)) {
    // 任务清单被删/改名：不判红，但要说清楚这条记录已无法复算
    notes.push(`${where} 的任务清单已不存在，跳过复算`);
    continue;
  }

  const args = [
    join(ROOT, "tools/dispatch.mjs"),
    "--tasks",
    tasksPath,
    "--id",
    record.task_id,
    "--emit",
    "json",
  ];
  if (record.preset) args.push("--preset", record.preset);
  const run = spawnSync(process.execPath, args, { cwd: ROOT, encoding: "utf8" });
  if (run.status !== 0) {
    notes.push(`${where} 复算失败（任务可能已删除或预设改名），跳过`);
    continue;
  }
  let fresh;
  try {
    fresh = JSON.parse(run.stdout);
  } catch {
    notes.push(`${where} 复算输出无法解析，跳过`);
    continue;
  }
  checked++;
  const inputs = record.inputs;
  if (!inputs) {
    notes.push(`${where} 是早期记录（无输入指纹），只校验判定与证据`);
    continue;
  }
  const changed = ["tasks_sha256", "spec_sha256", "preset_sha256"].filter(
    (k) => inputs[k] && inputs[k] !== fresh.inputs?.[k],
  );
  if (changed.length) {
    // 输入变了：记录代表的是当时的事实，不是篡改，也不能当结论用
    notes.push(
      `${where} 的输入已变更（${changed.map((k) => k.replace("_sha256", "")).join(", ")}），记录仅作历史参考`,
    );
    continue;
  }
  if (fresh.prompt_sha256 !== record.prompt_sha256) {
    errors.push(
      `${where} 输入未变但提示词指纹不符：记录 ${record.prompt_sha256.slice(0, 12)}… vs 复算 ${fresh.prompt_sha256.slice(0, 12)}…（记录被改过）`,
    );
  }
}

if (errors.length) {
  console.error("\n✗ check-trajectory 失败\n");
  for (const e of errors) console.error("  • " + e);
  console.error(
    `\n共 ${errors.length} 条。修复建议：记录不可改——任务变了就重新派发并以新记录替换；` +
      `判定与证据矛盾说明有人手改过记录。\n`,
  );
  process.exit(1);
}

console.log(`✓ check-trajectory 通过（${checked}/${files.length} 条记录可复算，判定与证据一致）`);
for (const n of notes) console.log("  · " + n);
