#!/usr/bin/env node
/**
 * 门禁 · tasks.md 结构与覆盖率
 *
 * tasks.md 是"派活"与"验收"之间唯一的凭据：漏了场景，AI 会做无用功；
 * layer 或允许改动写错，执行器就分不清"越界"和"完成"。所以派活之前先把
 * 结构钉死，人工评审时只讨论内容，不再兜底格式。
 *
 * 规则来源：.specify/templates/tasks.md、.specify/README.md。
 * 零依赖：门禁的结论必须能一条命令复核。
 */
import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join } from "node:path";
import { ROOT, rel } from "./paths.mjs";

const COLUMNS = ["ID", "任务", "场景", "layer", "允许改动", "验收命令"];
// 工作台自身的工具（tools/*.mjs）也要有归属层；塞进 docs 或 ui 是语义错误
const LAYERS = ["contract", "core", "shell", "ui", "docs", "tools"];

const errors = [];
const read = (path) => readFileSync(path, "utf8").replace(/^\uFEFF/, "");

/** 反引号只是渲染用的；比较列名与取值前必须剥掉。 */
const strip = (cell) => (cell ?? "").replace(/`/g, "").trim();

/** 按未转义的 | 切单元格：\| 是单元格内的字面竖线。 */
function splitRow(line) {
  const body = line
    .trim()
    .replace(/^\|/, "")
    .replace(/(?<!\\)\|$/, "");
  return body.split(/(?<!\\)\|/).map((cell) => cell.replace(/\\\|/g, "|").trim());
}

/** 收集连续以 | 开头的行；表格之外的行会切断当前表格。 */
function parseTables(text) {
  const tables = [];
  let current = null;
  text.split(/\r?\n/).forEach((line, i) => {
    if (!/^\s*\|/.test(line)) {
      current = null;
      return;
    }
    if (!current) {
      current = { headerLine: i + 1, rows: [] };
      tables.push(current);
    }
    current.rows.push({ line: i + 1, cells: splitRow(line) });
  });
  return tables;
}

const isSeparator = (cells) =>
  cells.length > 0 && cells.every((cell) => /^:?-{2,}:?$/.test(cell));

/** 选"最像任务表"的表格：列名命中越多越像，并列取第一个。 */
function pickTaskTable(tables) {
  let best = null;
  let bestScore = -1;
  for (const table of tables) {
    const header = table.rows[0]?.cells ?? [];
    const score = COLUMNS.filter((name) => header.some((h) => strip(h) === name)).length;
    if (score > bestScore) {
      best = table;
      bestScore = score;
    }
  }
  return best;
}

function describeTable(table) {
  const header = table?.rows[0]?.cells ?? [];
  const cols = Object.fromEntries(
    COLUMNS.map((name) => [name, header.findIndex((h) => strip(h) === name)]),
  );
  const dataRows = (table?.rows ?? [])
    .slice(1)
    .filter((row) => !isSeparator(row.cells) && row.cells.some((cell) => cell.trim() !== ""));
  return { header, cols, dataRows };
}

/** spec.md 的场景只认 `### S-NN` 标题，别处的 S-NN 是叙述，不算定义。 */
function scenariosOf(specText) {
  const scenarios = new Set();
  for (const match of specText.matchAll(/^###\s+(S-\d{2})\b/gm)) scenarios.add(match[1]);
  return scenarios;
}

/**
 * 契约任务必须由人走提案。机器能查的只有"有没有留下提案痕迹"：
 * tasks.md 写明「提案」，或 changes/ 里的提案提到了这个任务 ID。
 */
function proposalsMention(taskIds) {
  const dir = join(ROOT, "changes");
  if (!existsSync(dir)) return false;
  for (const name of readdirSync(dir)) {
    if (name === "README.md" || !name.endsWith(".md")) continue;
    const text = read(join(dir, name));
    if (taskIds.some((taskId) => text.includes(taskId))) return true;
  }
  return false;
}

function listSpecDirs() {
  const dir = join(ROOT, ".specify/specs");
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => !name.startsWith("."))
    .map((name) => join(dir, name))
    .filter((path) => statSync(path).isDirectory());
}

const specDirs = listSpecDirs();
if (specDirs.length === 0) {
  console.log("✓ check-tasks 通过（0 个能力：.specify/specs/ 下暂无规格目录，起步阶段允许为空）");
  process.exit(0);
}

let taskCount = 0;
let scenarioCount = 0;

for (const dir of specDirs) {
  const tasksPath = join(dir, "tasks.md");
  const specPath = join(dir, "spec.md");
  const tasksRel = rel(tasksPath);
  const specRel = rel(specPath);

  if (!existsSync(tasksPath)) {
    errors.push(`${tasksRel} 不存在：每个能力都必须有任务表（模板见 .specify/templates/tasks.md）`);
    continue;
  }

  const tasksText = read(tasksPath);
  const specScenarios = existsSync(specPath) ? scenariosOf(read(specPath)) : null;
  if (specScenarios === null) errors.push(`${specRel} 不存在：无法校验场景引用与覆盖率`);
  else scenarioCount += specScenarios.size;

  const table = pickTaskTable(parseTables(tasksText));
  if (!table) errors.push(`${tasksRel} 没有找到任务表：需要 Markdown 表格与六列表头`);

  const { header, cols, dataRows } = describeTable(table);
  const missingColumns = COLUMNS.filter((name) => cols[name] < 0);
  if (table && missingColumns.length) {
    errors.push(`${tasksRel} 缺少列：${missingColumns.join("、")}`);
  }

  const seenIds = new Map();
  const referenced = new Set();
  const contractTasks = [];

  for (const row of dataRows) {
    taskCount += 1;
    const where = (taskId) => `${tasksRel}:${row.line}${taskId ? ` ${taskId}` : ""}`;
    const cellOf = (name) => (cols[name] >= 0 ? strip(row.cells[cols[name]]) : null);

    // 少一格/多一格说明有未转义的 |，按列名定位会整体错位，先拦住
    if (header.length && row.cells.length !== header.length) {
      errors.push(
        `${where()} 单元格数是 ${row.cells.length}，表头是 ${header.length}：单元格里的 | 会破坏表格`,
      );
      continue;
    }

    const rawId = cellOf("ID");
    let taskId = null;
    if (rawId !== null) {
      if (!rawId) {
        errors.push(`${where()} ID 为空：每行必须有 T-NN`);
      } else if (!/^T-\d+$/.test(rawId)) {
        errors.push(`${where()} ID「${rawId}」不是 T-NN 形式`);
      } else {
        taskId = rawId;
        if (seenIds.has(taskId)) {
          errors.push(
            `${tasksRel} ${taskId} 重复（第 ${seenIds.get(taskId)} 行与第 ${row.line} 行）：同一文件内 ID 必须唯一`,
          );
        } else {
          seenIds.set(taskId, row.line);
        }
      }
    }
    const label = where(taskId);

    const rawScenes = cellOf("场景");
    if (rawScenes !== null) {
      if (!rawScenes) {
        errors.push(`${label} 场景为空：至少引用一个 S-NN`);
      } else {
        for (const token of rawScenes
          .split(/[,，]/)
          .map((s) => s.trim())
          .filter(Boolean)) {
          if (!/^S-\d{2}$/.test(token)) {
            errors.push(`${label} 场景「${token}」不是 S-NN 形式（多个场景用英文逗号分隔）`);
            continue;
          }
          referenced.add(token);
          if (specScenarios && !specScenarios.has(token)) {
            errors.push(`${label} 引用了不存在的场景 ${token}`);
          }
        }
      }
    }

    const rawLayer = cellOf("layer");
    if (rawLayer !== null) {
      if (!LAYERS.includes(rawLayer)) {
        errors.push(
          `${label} layer「${rawLayer || "（空）"}」不合法：只能是 ${LAYERS.join(" / ")}`,
        );
      } else if (rawLayer === "contract") {
        contractTasks.push(taskId ?? `第 ${row.line} 行`);
      }
    }

    const rawPaths = cellOf("允许改动");
    if (rawPaths !== null) {
      if (!rawPaths) {
        errors.push(`${label}「允许改动」为空：必须列出可改文件或目录的 glob`);
      } else if (/，/.test(rawPaths)) {
        errors.push(`${label}「允许改动」用了中文逗号：多个 glob 用英文逗号分隔`);
      } else if (rawPaths.split(",").some((glob) => !glob.trim())) {
        errors.push(`${label}「允许改动」有空的 glob 项：检查逗号位置或结尾逗号`);
      }
    }

    const rawCommand = cellOf("验收命令");
    if (rawCommand !== null) {
      if (!rawCommand) {
        errors.push(`${label}「验收命令」为空：每个任务都要有一条可执行命令`);
      } else if (rawCommand.includes("|")) {
        errors.push(`${label}「验收命令」含 |：会破坏表格解析，改成一条不带管道的命令`);
      }
    }
  }

  if (specScenarios) {
    for (const scenario of specScenarios) {
      if (!referenced.has(scenario)) {
        errors.push(`${specRel} 的场景 ${scenario} 没有任何任务覆盖（场景未覆盖）`);
      }
    }
  }

  if (contractTasks.length) {
    const ids = contractTasks.filter((task) => /^T-\d+$/.test(task));
    const hasProposal = tasksText.includes("提案") || (ids.length > 0 && proposalsMention(ids));
    if (!hasProposal) {
      for (const task of contractTasks) {
        errors.push(
          `${tasksRel} ${task} 是 contract 层任务，但没有出现在 changes/ 提案里：契约只能由人走提案修改，AI 不得执行`,
        );
      }
    }
  }
}

// 模板是所有人写任务表的起点，模板缺列会一路传染到每个能力
const templatePath = join(ROOT, ".specify/templates/tasks.md");
if (!existsSync(templatePath)) {
  errors.push(".specify/templates/tasks.md 不存在：模板是所有人写任务的起点");
} else {
  const templateHeader = pickTaskTable(parseTables(read(templatePath)))?.rows[0]?.cells ?? [];
  const missing = COLUMNS.filter((name) => !templateHeader.some((h) => strip(h) === name));
  if (missing.length) {
    errors.push(
      `.specify/templates/tasks.md 缺少列：${missing.join("、")}（模板是所有人的起点，错了会传染）`,
    );
  }
}

if (errors.length) {
  console.error("\n✗ check-tasks 失败\n");
  for (const error of errors) console.error("  • " + error);
  console.error(
    `\n共 ${errors.length} 条。修复建议：改 tasks.md 补齐字段；场景确实不用做则在 spec.md 里删掉该场景。\n`,
  );
  process.exit(1);
}

console.log(
  `✓ check-tasks 通过（${specDirs.length} 个能力、${taskCount} 个任务、${scenarioCount} 个场景全覆盖）`,
);
