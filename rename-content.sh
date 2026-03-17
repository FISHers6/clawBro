#!/usr/bin/env bash
# rename-content.sh — 批量替换文件内容（不改文件名）
# 排除：quick-ai 子目录、.git、target、Cargo.lock
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
echo "=== 内容替换 ==="
echo "根目录: $ROOT"
echo ""

# 找出所有需要处理的文本文件
FILES=$(find "$ROOT" -type f \
  -not -path "*/.git/*" \
  -not -path "*/target/*" \
  -not -name "Cargo.lock" \
  -not -name "*.png" -not -name "*.jpg" -not -name "*.ico" \
  -not -name "*.db" -not -name "*.sqlite" \
  -not -name "rename-content.sh" \
  -not -name "rename-files.sh" \
  -not -name "verify-rename.sh" \
)

COUNT=0
for f in $FILES; do
  # 跳过二进制文件
  if file "$f" | grep -q "binary\|ELF\|Mach-O"; then
    continue
  fi

  # 五条替换规则（按优先级：PascalCase → UPPER → lower → kebab → snake）
  sed -i '' \
    -e 's/QuickAI/ClawBro/g' \
    -e 's/QUICKAI/CLAWBRO/g' \
    -e 's/quickai/clawbro/g' \
    -e 's/qai-/clawbro-/g' \
    -e 's/qai_/clawbro_/g' \
    "$f" 2>/dev/null || true

  COUNT=$((COUNT + 1))
done

echo "✓ 已处理 $COUNT 个文件"
echo ""
echo "下一步：运行 ./rename-files.sh"
