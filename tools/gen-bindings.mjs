#!/usr/bin/env node
/**
 * 契约即代码：由 Rust 生成前端类型。
 *
 * 流程：
 *   1. 跑两个 crate 的测试，触发 ts-rs 把 `#[ts(export)]` 的类型写到
 *      apps/shell/src/generated/（导出目录由 .cargo/config.toml 固定）
 *   2. 生成 index.ts（类型桶文件）
 *   3. 生成 manifest.json（每个文件的 sha256）
 *
 * manifest 是"提交时的期望值"：门禁拿它和磁盘上的文件比对，
 * 手改生成物或忘了重新生成都会红。
 */
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, writeFileSync, unlinkSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

// Windows 上 URL().pathname 会给出 /C:/...，必须用 fileURLToPath
const ROOT = fileURLToPath(new URL("..", import.meta.url));
const DIR = join(ROOT, "apps/shell/src/generated");

const run = (args) => {
  execFileSync("cargo", args, { cwd: ROOT, stdio: "inherit" });
};

const RESERVED = new Set(["index.ts", "commands.ts"]);

// 先清旧类型：删掉 #[ts(export)] 的字段/类型后，ts-rs 不会主动删除旧文件，
// 陈旧 .ts 会一直"同步"通过 manifest 与契约检查，掩盖契约漂移。
const stale = readdirSync(DIR).filter((f) => f.endsWith(".ts") && !RESERVED.has(f));
for (const f of stale) unlinkSync(join(DIR, f));
if (stale.length) console.log(`· 清理 ${stale.length} 个旧生成类型（重新导出中）`);

console.log("· 导出 ts-rs 绑定");
run(["test", "-q", "-p", "tellerkit-core", "--lib"]);
run(["test", "-q", "-p", "tellerkit", "--lib"]);

const typeFiles = readdirSync(DIR)
  .filter((f) => f.endsWith(".ts") && !RESERVED.has(f))
  .sort();

if (!typeFiles.length) {
  console.error("✗ 没有生成任何类型文件：检查 #[ts(export)] 是否还在");
  process.exit(1);
}

const barrel = [
  "// 本文件由 tools/gen-bindings.mjs 生成，请勿手改。",
  "// 重新生成：npm run bindings",
  "",
  ...typeFiles.map((f) => `export type { ${f.replace(/\.ts$/, "")} } from "./${f.replace(/\.ts$/, "")}";`),
  "",
].join("\n");
writeFileSync(join(DIR, "index.ts"), barrel);

// 命令名由 generate_handler! 派生，前端拼错命令名会直接是类型错误
const libRs = readFileSync(join(ROOT, "apps/shell/src-tauri/src/lib.rs"), "utf8");
const handler = /generate_handler!\[([\s\S]*?)\]/.exec(libRs);
if (!handler) {
  console.error("✗ 在 lib.rs 里找不到 generate_handler!，无法生成命令名");
  process.exit(1);
}
const commandNames = handler[1]
  .split(",")
  .map((s) => s.trim().replace(/^commands::/, ""))
  .filter(Boolean)
  .sort();
writeFileSync(
  join(DIR, "commands.ts"),
  [
    "// 本文件由 tools/gen-bindings.mjs 生成，请勿手改。",
    "",
    `export type CommandName = ${commandNames.map((n) => JSON.stringify(n)).join(" | ")};`,
    "",
  ].join("\n"),
);

const files = ["index.ts", "commands.ts", ...typeFiles];
const manifest = { generator: "tools/gen-bindings.mjs", files: {} };
for (const f of files) {
  const buf = readFileSync(join(DIR, f));
  manifest.files[f] = createHash("sha256").update(buf).digest("hex");
}
writeFileSync(join(DIR, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");

console.log(`✓ 已生成 ${typeFiles.length} 个类型 + index.ts + manifest.json`);
