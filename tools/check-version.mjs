#!/usr/bin/env node
/**
 * 版本单一来源：Cargo.toml 的 workspace.package.version 是唯一真相。
 *
 * 四处版本错位会让灰度桶按错误的应用版本洗牌（架构评审 D-05）。
 * 本门禁不负责改写，只负责让漂移当场失败；生成动作由发布流程统一做。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT, rel } from "./paths.mjs";

const errors = [];
const read = (p) => readFileSync(join(ROOT, p), "utf8");

// 唯一真相：[workspace.package] 段里的 version。
const cargo = read("Cargo.toml");
const section = cargo.match(/\[workspace\.package\][^[]*/);
const source = section?.[0].match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!source) {
  console.error("✗ check-version 失败：Cargo.toml 的 [workspace.package].version 缺失，无法确定唯一版本源");
  process.exit(1);
}

// 三处副本必须与唯一源一致。
const copies = [
  ["package.json", (t) => JSON.parse(t).version],
  ["apps/shell/package.json", (t) => JSON.parse(t).version],
  ["apps/shell/src-tauri/tauri.conf.json", (t) => JSON.parse(t).version],
];
for (const [file, extract] of copies) {
  let actual;
  try {
    actual = extract(read(file));
  } catch {
    errors.push(`${rel(file)} 不存在或不是合法 JSON`);
    continue;
  }
  if (actual !== source) errors.push(`${rel(file)} 版本 ${actual} 与 Cargo.toml 的唯一源 ${source} 不一致`);
}

if (errors.length) {
  console.error("\n✗ check-version 失败\n");
  for (const e of errors) console.error("  • " + e);
  console.error("\n统一版本后重跑；不要只改其中一处。\n");
  process.exit(1);
}
console.log(`✓ check-version 通过（唯一源 Cargo.toml = ${source}，3 处副本一致）`);
