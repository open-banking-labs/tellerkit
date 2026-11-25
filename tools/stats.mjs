#!/usr/bin/env node
/**
 * 统计口径：项目规模数字只从这里出，不手抄。
 *
 * 起因：文档与阶段性汇报里长期引用"能力/任务/场景"数字，靠人记，
 * 结果和 `check-tasks` 的真实输出对不上（差了好几个）。
 * 数字一旦手写就一定会漂，所以做成命令。
 */
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseTasksFile } from "./lib/tasks.mjs";

const ROOT = fileURLToPath(new URL("..", import.meta.url));

function countSpecs() {
  const dir = join(ROOT, ".specify", "specs");
  if (!existsSync(dir)) return { capabilities: 0, tasks: 0, scenarios: 0 };
  let capabilities = 0;
  let tasks = 0;
  let scenarios = 0;

  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const specPath = join(dir, entry.name, "spec.md");
    const tasksPath = join(dir, entry.name, "tasks.md");
    if (!existsSync(specPath) || !existsSync(tasksPath)) continue;
    capabilities += 1;

    const spec = readFileSync(specPath, "utf8");
    scenarios += (spec.match(/^### S-\d+/gm) ?? []).length;

    try {
      tasks += parseTasksFile(tasksPath, (p) => readFileSync(p, "utf8")).length;
    } catch {
      // 解析失败的规格不计入，但由 check-tasks 负责报错
    }
  }
  return { capabilities, tasks, scenarios };
}

function contractCounts() {
  const file = join(ROOT, "contracts", "fields.json");
  if (!existsSync(file)) return { structs: 0, generatedTypes: 0, commands: 0 };
  const c = JSON.parse(readFileSync(file, "utf8"));
  return {
    structs: Object.keys(c.rust_structs ?? {}).length,
    generatedTypes: (c.generated_types ?? []).length,
    commands: (c.commands ?? []).length,
  };
}

const specs = countSpecs();
const contracts = contractCounts();

console.log(`能力（.specify/specs）      ${specs.capabilities}`);
console.log(`任务（tasks.md）            ${specs.tasks}`);
console.log(`场景（spec.md S-xx）        ${specs.scenarios}`);
console.log(`契约结构体                  ${contracts.structs}`);
console.log(`生成的 TS 类型              ${contracts.generatedTypes}`);
console.log(`IPC 命令                    ${contracts.commands}`);
console.log("");
console.log("测试数与门禁以 `npm run gate` 的退出码为准，不要在这里猜。");
