/**
 * tasks.md 解析（消费方共用）。
 *
 * 为什么单独抽出来：执行器要拿 `command/allowed` 判裁决，派发器要拿
 * `scenarios/layer` 拼提示词。两处各写一个解析器，格式一改就会静默分叉——
 * 而分叉的后果是"派活"和"验收"对同一张表理解不同。
 *
 * 门禁 `check-tasks.mjs` 不用这个：它需要行号与"收集全部错误"的行为，
 * 解析目标不同，刻意保留独立实现。
 */

export class TasksParseError extends Error {}

const stripTicks = (s) => s.replace(/`/g, "").trim();
const stripWrapTicks = (s) => s.trim().replace(/^`+/, "").replace(/`+$/, "").trim();
const isSeparator = (cells) => cells.every((c) => /^:?-{3,}:?$/.test(c));

function parseRow(line) {
  const s = line.trim();
  if (!s.startsWith("|")) return null;
  // 不闭合的行返回 null cells，交给调用方报"列数不符"而不是静默跳过
  if (!s.endsWith("|")) return { cells: null };
  return { cells: s.slice(1, -1).split("|").map((c) => c.trim()) };
}

/**
 * 解析 tasks.md，返回任务数组。
 *
 * 每个任务：{ id, title, scenarios, layer, command, allowed, line }
 * 解析失败抛 TasksParseError（消息已经是中文可读的）。
 */
export function parseTasksFile(file, readFile) {
  let text;
  try {
    text = readFile(file);
  } catch (e) {
    throw new TasksParseError(`读不到 tasks.md：${file}（${e.code || e.message}）`);
  }

  const lines = text.split(/\r?\n/);
  let header = null;
  let headerLine = 0;
  let inFence = false;

  for (let i = 0; i < lines.length; i++) {
    if (/^\s*```/.test(lines[i])) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;
    const row = parseRow(lines[i]);
    if (!row?.cells) continue;
    // 按列名找表头：列顺序以后可能调整，写死下标会静默错位
    const names = row.cells.map(stripTicks);
    if (names.includes("ID") && names.includes("验收命令")) {
      header = names;
      headerLine = i;
      break;
    }
  }
  if (!header) {
    throw new TasksParseError(`${file} 未找到表头：需要含「ID」与「验收命令」的表格行`);
  }

  for (const name of ["ID", "允许改动", "验收命令"]) {
    if (header.indexOf(name) < 0) {
      throw new TasksParseError(`${file}:${headerLine + 1} 缺少列「${name}」，无法执行`);
    }
    if (header.filter((h) => h === name).length > 1) {
      throw new TasksParseError(`${file}:${headerLine + 1} 列名「${name}」重复`);
    }
  }

  const at = (i) => `${file}:${i + 1}`;
  const col = (name) => header.indexOf(name);
  const tasks = [];
  const seen = new Set();

  for (let i = headerLine + 1; i < lines.length; i++) {
    const line = lines[i];
    if (!line.trim()) break; // 只读第一张表，空行即表尾
    const row = parseRow(line);
    if (!row) break;
    if (!row.cells) throw new TasksParseError(`${at(i)} 表格行必须以 | 开头并以 | 结尾`);
    if (row.cells.length !== header.length) {
      throw new TasksParseError(
        `${at(i)} 有 ${row.cells.length} 列、表头 ${header.length} 列：单元格内不允许出现「|」，多路径请用英文逗号分隔`,
      );
    }
    if (isSeparator(row.cells)) continue;

    const id = stripTicks(row.cells[col("ID")]);
    if (!id) throw new TasksParseError(`${at(i)} ID 为空`);
    if (seen.has(id)) throw new TasksParseError(`${at(i)} ID「${id}」重复：同一文件内必须唯一`);
    seen.add(id);

    const allowedCell = col("允许改动") >= 0 ? row.cells[col("允许改动")] : "";
    const allowed = allowedCell
      .replace(/`/g, "")
      .split(",")
      .map((p) => p.trim())
      .filter(Boolean);
    const command = col("验收命令") >= 0 ? stripWrapTicks(row.cells[col("验收命令")]) : "";
    if (!allowed.length) {
      throw new TasksParseError(`${at(i)} 任务 ${id} 的「允许改动」为空：没有白名单就无法判越界`);
    }
    if (!command) throw new TasksParseError(`${at(i)} 任务 ${id} 的「验收命令」为空`);

    const scenariosCell = col("场景") >= 0 ? row.cells[col("场景")] : "";
    const scenarios = scenariosCell
      .replace(/`/g, "")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);

    tasks.push({
      id,
      title: col("任务") >= 0 ? stripTicks(row.cells[col("任务")]) : "",
      scenarios,
      layer: col("layer") >= 0 ? stripTicks(row.cells[col("layer")]) : "",
      command,
      allowed,
      line: i + 1,
    });
  }

  if (!tasks.length) throw new TasksParseError(`${file} 表头下没有任务行`);
  return tasks;
}
