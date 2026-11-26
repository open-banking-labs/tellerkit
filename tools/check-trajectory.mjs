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
 * 无法复算（任务清单被删、复算失败、指纹不符）一律判红，不再"记一条 note 放行"。
 * 确实已过时的记录，在 .agents/dispatch-superseded.json 显式登记原因；这是
 * 唯一允许跳过复算的出口，且会在输出里点名，不会伪装成"全部可复算"。
 *
 * 这就是"轨迹门禁"的最小可用形态：不采集会话日志，只保证
 * 「记录里写的结论」与「现在还能复现的事实」一致。
 */
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { ROOT, rel } from "./paths.mjs";

const DIR = join(ROOT, ".agents/dispatches");
const SUPERSEDED = join(ROOT, ".agents/dispatch-superseded.json");
const errors = [];
const notes = [];
let superseded = {};
if (existsSync(SUPERSEDED)) {
  try {
    superseded = JSON.parse(readFileSync(SUPERSEDED, "utf8"));
  } catch {
    errors.push(".agents/dispatch-superseded.json 不是合法 JSON");
  }
}

if (!existsSync(DIR)) {
  console.log("✓ check-trajectory 通过（还没有派发记录）");
  process.exit(0);
}

const files = readdirSync(DIR).filter((f) => f.endsWith(".json"));
let checked = 0;
let supersededCount = 0;
for (const name of Object.keys(superseded)) {
  if (!files.includes(name)) {
    errors.push(`.agents/dispatch-superseded.json 登记了不存在的记录：${name}`);
  }
}

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

  // 已显式作废的记录不参与复算；作废必须写原因，避免"静默跳过"
  if (Object.hasOwn(superseded, file)) {
    const reason = superseded[file];
    if (typeof reason !== "string" || reason.trim() === "") {
      errors.push(`${where} 已登记作废但没有写原因`);
    } else {
      supersededCount++;
      notes.push(`${where} 已作废：${reason}`);
    }
    continue;
  }

  // 1：复算提示词指纹
  if (!record.tasks_file || !record.task_id) {
    errors.push(`${where} 缺少 tasks_file 或 task_id，无法复算`);
    continue;
  }
  const tasksPath = join(ROOT, record.tasks_file);
  if (!existsSync(tasksPath)) {
    errors.push(`${where} 的任务清单已不存在，无法复算（记录需重新派发或显式作废）`);
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
    errors.push(`${where} 复算失败（任务已删除或预设改名），记录无法复核`);
    continue;
  }
  let fresh;
  try {
    fresh = JSON.parse(run.stdout);
  } catch {
    errors.push(`${where} 复算输出无法解析，记录无法复核`);
    continue;
  }
  if (!record.prompt_sha256) {
    errors.push(`${where} 缺少 prompt_sha256，无法复核`);
    continue;
  }
  const inputs = record.inputs;
  if (inputs) {
    const changed = ["tasks_sha256", "spec_sha256", "preset_sha256"].filter(
      (k) => inputs[k] && inputs[k] !== fresh.inputs?.[k],
    );
    if (changed.length) {
      errors.push(
        `${where} 的输入已变更（${changed.map((k) => k.replace("_sha256", "")).join(", ")}）：记录不再代表当前任务，请重新派发或显式作废`,
      );
      continue;
    }
  }
  if (fresh.prompt_sha256 !== record.prompt_sha256) {
    errors.push(
      `${where} 提示词指纹不符：记录 ${record.prompt_sha256.slice(0, 12)}… vs 复算 ${fresh.prompt_sha256.slice(0, 12)}…（任务被改过或记录被手改）`,
    );
    continue;
  }
  checked++;
  if (!inputs) {
    notes.push(`${where} 是早期记录（无输入指纹），已按当前任务复算提示词并通过`);
  }
}

if (errors.length) {
  console.error("\n✗ check-trajectory 失败\n");
  for (const e of errors) console.error("  • " + e);
  console.error(
    `\n共 ${errors.length} 条。修复建议：记录不可改——任务变了就重新派发并以新记录替换，` +
      `或在 .agents/dispatch-superseded.json 登记作废原因；判定与证据矛盾说明有人手改过记录。\n`,
  );
  process.exit(1);
}

const active = files.length - supersededCount;
console.log(
  `✓ check-trajectory 通过（${checked}/${active} 条在册记录可复算，判定与证据一致` +
    (supersededCount ? `；${supersededCount} 条已作废` : "") +
    `）`,
);
for (const n of notes) console.log("  · " + n);
