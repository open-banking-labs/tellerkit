#!/usr/bin/env bash
# 在首尔构建机上编译（经 Tailscale），本地不跑 cargo。
#
# 为什么不本地编译：本机是开发用的笔记本，2C/3G 的首尔机才是稳定的构建环境，
# 且它同时是 Linux 侧的验证机。规则写在这里而不是口头约定：
# 「要编译，就去 build host」。
#
# 用法：
#   scripts/remote-build.sh                       # 默认 cargo test --workspace
#   scripts/remote-build.sh cargo test -p tellerkit-core
#   scripts/remote-build.sh cargo build -p tellerkit --features custom-protocol
#
# 环境变量：
#   TK_BUILD_HOST  默认 tailscale-seoul
#   TK_BUILD_DIR   默认 ~/build/tk-work
set -euo pipefail

HOST="${TK_BUILD_HOST:-tailscale-seoul}"
DIR="${TK_BUILD_DIR:-$HOME/build/tk-work}"
REMOTE_DIR="\$HOME/build/tk-work"
CMD="${*:-cargo test --workspace}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "→ 同步源码到 $HOST:$REMOTE_DIR"
rsync -az --delete \
  --exclude target --exclude node_modules --exclude dist --exclude .git \
  ./ "$HOST:$REMOTE_DIR/"

echo "→ 远端执行：$CMD"
ssh "$HOST" "set -e; source \$HOME/.cargo/env; cd $REMOTE_DIR; $CMD"
