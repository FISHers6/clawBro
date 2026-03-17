#!/usr/bin/env bash
# rename-files.sh — 重命名文件和目录
# 必须在 rename-content.sh 之后运行
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
echo "=== 文件/目录重命名 ==="
echo ""

# ────────────────────────────────────────
# 1. 重命名 crates/ 目录（先重命名，后续文件自动跟随）
# ────────────────────────────────────────
echo "[1] 重命名 crates/ 目录..."

rename_dir() {
  local old="$ROOT/crates/$1"
  local new="$ROOT/crates/$2"
  if [ -d "$old" ]; then
    mv "$old" "$new"
    echo "  mv crates/$1  →  crates/$2"
  fi
}

rename_dir "qai-protocol"        "clawbro-protocol"
rename_dir "qai-session"         "clawbro-session"
rename_dir "qai-agent"           "clawbro-agent"
rename_dir "qai-runtime"         "clawbro-runtime"
rename_dir "qai-channels"        "clawbro-channels"
rename_dir "qai-skills"          "clawbro-skills"
rename_dir "qai-cron"            "clawbro-cron"
rename_dir "qai-server"          "clawbro-server"
rename_dir "quickai-agent-sdk"   "clawbro-agent-sdk"
rename_dir "quickai-rust-agent"  "clawbro-rust-agent"

echo ""

# ────────────────────────────────────────
# 2. 重命名 clawbro-server 下的 bin/*.rs 文件
#    内容已替换，路径引用已更新，现在同步实际文件名
# ────────────────────────────────────────
echo "[2] 重命名 bin/ 源文件..."

BIN_DIR="$ROOT/crates/clawbro-server/src/bin"

rename_rs() {
  local old="$BIN_DIR/$1"
  local new="$BIN_DIR/$2"
  if [ -f "$old" ]; then
    mv "$old" "$new"
    echo "  mv $1  →  $2"
  fi
}

# CLI 主入口
rename_rs "quickai_cli.rs"                          "clawbro_cli.rs"

# Team CLI
rename_rs "qai_team_cli.rs"                         "clawbro_team_cli.rs"

# 测试用 fixture binaries
rename_rs "qai_acp_approval_fixture.rs"             "clawbro_acp_approval_fixture.rs"
rename_rs "qai_acp_echo_fixture.rs"                 "clawbro_acp_echo_fixture.rs"
rename_rs "qai_acp_team_fixture.rs"                 "clawbro_acp_team_fixture.rs"
rename_rs "qai_native_echo_fixture.rs"              "clawbro_native_echo_fixture.rs"
rename_rs "qai_native_team_fixture.rs"              "clawbro_native_team_fixture.rs"
rename_rs "qai_native_team_missing_completion_fixture.rs" \
          "clawbro_native_team_missing_completion_fixture.rs"
rename_rs "qai_openclaw_gateway_fixture.rs"         "clawbro_openclaw_gateway_fixture.rs"

echo ""

# ────────────────────────────────────────
# 3. 删除 Cargo.lock（让 cargo 重新生成）
# ────────────────────────────────────────
echo "[3] 删除 Cargo.lock（将由 cargo 自动重新生成）..."
if [ -f "$ROOT/Cargo.lock" ]; then
  rm "$ROOT/Cargo.lock"
  echo "  ✓ Cargo.lock 已删除"
fi

echo ""

# ────────────────────────────────────────
# 4. 提示：重命名工作区目录本身（需从父目录操作）
# ────────────────────────────────────────
echo "[4] 工作区目录重命名（需手动执行）："
echo ""
echo "  cd /Users/fishers/Desktop/repo/quickai-openclaw"
echo "  mv quickai-gateway clawbro-gateway"
echo ""
echo "✓ 文件/目录重命名完成"
echo ""
echo "下一步：运行 ./verify-rename.sh"
