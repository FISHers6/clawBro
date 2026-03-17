#!/usr/bin/env bash
# verify-rename.sh — 检查是否还有残留的旧名称
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
echo "=== 重命名验证 ==="
echo ""

FAIL=0

check_pattern() {
  local label="$1"
  local pattern="$2"
  # 排除脚本本身、.git、target、Cargo.lock 和 verify 脚本
  local HITS
  HITS=$(grep -r --include="*.rs" --include="*.toml" --include="*.md" \
    --include="*.sh" --include="*.json" --include="*.ts" \
    -l "$pattern" "$ROOT" \
    --exclude-dir=".git" \
    --exclude-dir="target" \
    --exclude="Cargo.lock" \
    --exclude="rename-content.sh" \
    --exclude="rename-files.sh" \
    --exclude="verify-rename.sh" \
    2>/dev/null || true)

  if [ -n "$HITS" ]; then
    echo "  ✗ 还有 '$label' 残留："
    echo "$HITS" | sed 's/^/      /'
    FAIL=1
  else
    echo "  ✓ $label 已全部替换"
  fi
}

echo "[1] 检查旧名称是否残留..."
check_pattern "quickai"   "quickai"
check_pattern "QuickAI"   "QuickAI"
check_pattern "QUICKAI"   "QUICKAI"
check_pattern "qai-"      "qai-"
check_pattern "qai_"      "qai_"

echo ""
echo "[2] 检查旧目录是否残留..."
for d in qai-protocol qai-session qai-agent qai-runtime qai-channels \
          qai-skills qai-cron qai-server quickai-agent-sdk quickai-rust-agent; do
  if [ -d "$ROOT/crates/$d" ]; then
    echo "  ✗ 目录仍存在: crates/$d"
    FAIL=1
  else
    echo "  ✓ crates/$d 已不存在"
  fi
done

echo ""
echo "[3] 检查旧 bin 文件是否残留..."
BIN_DIR="$ROOT/crates/clawbro-server/src/bin"
for f in quickai_cli qai_team_cli qai_acp_approval_fixture qai_acp_echo_fixture \
         qai_acp_team_fixture qai_native_echo_fixture qai_native_team_fixture \
         qai_native_team_missing_completion_fixture qai_openclaw_gateway_fixture; do
  if [ -f "$BIN_DIR/${f}.rs" ]; then
    echo "  ✗ 文件仍存在: src/bin/${f}.rs"
    FAIL=1
  else
    echo "  ✓ ${f}.rs 已不存在"
  fi
done

echo ""
echo "[4] 尝试 cargo check..."
if cargo check -p clawbro-server 2>&1 | grep -q "^error"; then
  echo "  ✗ cargo check 有编译错误，请查看上方输出"
  FAIL=1
else
  echo "  ✓ cargo check 通过"
fi

echo ""
if [ $FAIL -eq 0 ]; then
  echo "✅ 全部验证通过！重命名完成。"
else
  echo "❌ 存在问题，请根据上方提示修复。"
  exit 1
fi
