#!/usr/bin/env bash
# P0 验收脚本：不依赖 GUI，验证核心链路的"硬门槛"。
#
#   1. 核心库单元测试（过滤边界、验签、篡改、原子替换、版本保留、回滚）
#   2. 造包 → 验签 → 查看（服务端构建环节）
#   3. 改一个字节 → 必须被拒绝
#   4. 版本号被改 → 必须被拒绝
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; exit 1; }
step() { printf '\n\033[1m%s\033[0m\n' "$1"; }

step "1/4 核心库测试"
cargo test -q -p tellerkit-core 2>&1 | tail -3
pass "tellerkit-core 测试通过"

step "2/4 造包 → 验签 → 查看"
cargo run -q -p mtree-tool -- demo "$TMP/tree.json" >/dev/null
cargo run -q -p mtree-tool -- build "$TMP/tree.json" "$TMP/menu.7.mtree" --version 7 >/dev/null
cargo run -q -p mtree-tool -- verify "$TMP/menu.7.mtree" | grep -q "验签通过" || fail "正常包验签失败"
pass "合法菜单包验签通过"
cargo run -q -p mtree-tool -- inspect "$TMP/menu.7.mtree" | sed 's/^/    /'

step "3/4 篡改 payload 一个字节"
python3 - "$TMP/menu.7.mtree" "$TMP/bad-payload.mtree" <<'PY'
import sys
src, dst = sys.argv[1], sys.argv[2]
data = bytearray(open(src, "rb").read())
idx = len(data) // 2
data[idx] ^= 0x01
open(dst, "wb").write(data)
print(f"    翻转第 {idx} 字节")
PY
if cargo run -q -p mtree-tool -- verify "$TMP/bad-payload.mtree" 2>/dev/null | grep -q "验签通过"; then
  fail "篡改后的包竟然验签通过"
fi
pass "篡改 payload 被拒绝"

step "4/4 篡改版本号"
python3 - "$TMP/menu.7.mtree" "$TMP/bad-version.mtree" <<'PY'
import sys
src, dst = sys.argv[1], sys.argv[2]
data = bytearray(open(src, "rb").read())
data[14] ^= 0x01          # menu_version 最低字节
open(dst, "wb").write(data)
print("    修改 menu_version 最低字节")
PY
if cargo run -q -p mtree-tool -- verify "$TMP/bad-version.mtree" 2>/dev/null | grep -q "验签通过"; then
  fail "改版本号的包竟然验签通过"
fi
pass "版本号篡改被拒绝（签名覆盖头部）"

printf '\n\033[32mP0 自动化验收全部通过\033[0m\n'
