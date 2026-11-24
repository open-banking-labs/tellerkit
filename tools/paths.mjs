#!/usr/bin/env node
/**
 * 门禁共用的路径发现。
 *
 * 门禁不该写死 `src/`、`src-tauri/` 这种旧路径——仓库是 monorepo，
 * 新增一个 app 或 package 时门禁必须自动覆盖到，否则"绿"就是假的。
 */
import { readdirSync, statSync, existsSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

export // Windows 上 URL().pathname 会给出 /C:/...，必须用 fileURLToPath
const ROOT = fileURLToPath(new URL("..", import.meta.url));
export const rel = (p) => relative(ROOT, p);
/** 统一成正斜杠：字符串比对不能依赖平台分隔符（Windows 是反斜杠）。 */
export const posix = (p) => p.split(sep).join("/");
export const exists = (p) => existsSync(p);

export function walk(dir, out = []) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return out;
  }
  for (const name of entries) {
    if (["node_modules", "target", ".git", "dist", "gen"].includes(name)) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

export function listDirs(dir) {
  try {
    return readdirSync(dir)
      .filter((n) => !n.startsWith("."))
      .map((n) => join(dir, n))
      .filter((p) => statSync(p).isDirectory());
  } catch {
    return [];
  }
}

export const apps = listDirs(join(ROOT, "apps"));
export const packages = listDirs(join(ROOT, "packages"));

/** 前端源码目录：apps/<app>/src */
export const frontendDirs = apps.map((a) => join(a, "src")).filter(exists);
/** 主壳 Rust 目录：apps/<app>/src-tauri/src */
export const shellRustDirs = apps.map((a) => join(a, "src-tauri/src")).filter(exists);
/** 核心 crate 源码目录：packages/<pkg>/src */
export const packageSrcDirs = packages.map((p) => join(p, "src")).filter(exists);

export const frontendFiles = frontendDirs.flatMap((d) =>
  walk(d).filter((f) => /\.(ts|vue)$/.test(f)),
);

export const rustFiles = [
  ...packageSrcDirs.flatMap((d) => walk(d)),
  ...shellRustDirs.flatMap((d) => walk(d)),
  ...walk(join(ROOT, "tools/mtree-tool/src")),
].filter((f) => f.endsWith(".rs"));

/** ts-rs 生成物目录（每个 app 一份） */
export const generatedDirs = frontendDirs.map((d) => join(d, "generated")).filter(exists);

/**
 * 命令层文件。
 *
 * 用 posix() 之后再比后缀：Windows 上路径是 `...\commands.rs`，
 * 直接 `endsWith("/commands.rs")` 恒为假——命令注册检查会静默变成空集。
 */
export const commandFiles = shellRustDirs
  .flatMap((d) => walk(d))
  .filter((f) => posix(f).endsWith("/commands.rs"));

/** Tauri 装配入口 */
export const handlerFiles = shellRustDirs
  .flatMap((d) => walk(d))
  .filter((f) => posix(f).endsWith("/lib.rs"));

/** 所有受管的"单位"：每个都要有 AGENTS.md + README.md */
export const units = [...apps, ...packages];
