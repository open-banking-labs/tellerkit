/**
 * 极简 glob：用于校验"改了哪些文件"是否落在任务的允许范围内。
 *
 * 只支持两种通配符，足够表达本仓库的白名单：
 *   `*`  匹配单个路径段内任意字符（不跨 `/`）
 *   `**` 匹配任意多级目录
 *
 * 刻意不引入第三方依赖：白名单守卫是安全边界的一部分，
 * 依赖越少，行为越可预测。
 */

/** 把 glob 编译成正则。 */
export function globToRegExp(glob) {
  const normalized = glob.replace(/^\.\//, "");
  let out = "^";
  for (let i = 0; i < normalized.length; i++) {
    const ch = normalized[i];
    if (ch === "*") {
      if (normalized[i + 1] === "*") {
        // `**/` 吞掉若干层目录；`**` 结尾则吞掉剩余全部
        if (normalized[i + 2] === "/") {
          out += "(?:.*/)?";
          i += 2;
        } else {
          out += ".*";
          i += 1;
        }
      } else {
        out += "[^/]*";
      }
      continue;
    }
    if (ch === "?") {
      out += "[^/]";
      continue;
    }
    out += ch.replace(/[.+^${}()|[\]\\]/g, "\\$&");
  }
  out += "$";
  return new RegExp(out);
}

/** 路径是否命中任一模式。 */
export function matchAny(path, patterns) {
  const normalized = path.replace(/^\.\//, "");
  return patterns.some((p) => globToRegExp(p).test(normalized));
}

/** 找出所有未命中的路径（即越界文件）。 */
export function findViolations(paths, patterns) {
  return paths.filter((p) => !matchAny(p, patterns));
}
