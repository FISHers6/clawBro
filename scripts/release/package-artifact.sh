#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <target-triple> <version> <output-dir>" >&2
  exit 1
fi

TARGET="$1"
VERSION="$2"
OUT_DIR="$3"

case "$TARGET" in
  aarch64-apple-darwin)
    ASSET_BASENAME="clawbro-darwin-aarch64"
    BIN_NAME="clawbro"
    ;;
  x86_64-apple-darwin)
    ASSET_BASENAME="clawbro-darwin-x86_64"
    BIN_NAME="clawbro"
    ;;
  x86_64-unknown-linux-gnu)
    ASSET_BASENAME="clawbro-linux-x86_64"
    BIN_NAME="clawbro"
    ;;
  *)
    echo "unsupported target: $TARGET" >&2
    exit 1
    ;;
esac

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BIN_PATH="$ROOT_DIR/target/$TARGET/release/clawbro"
STAGE_DIR="$ROOT_DIR/target/release-stage/$TARGET"

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR" "$ROOT_DIR/$OUT_DIR"

cp "$BIN_PATH" "$STAGE_DIR/$BIN_NAME"
chmod +x "$STAGE_DIR/$BIN_NAME"

cat > "$STAGE_DIR/README.txt" <<EOF
clawbro $VERSION

Quick start:
  ./clawbro --version
  ./clawbro setup
  ./clawbro serve
EOF

tar -C "$STAGE_DIR" -czf "$ROOT_DIR/$OUT_DIR/$ASSET_BASENAME.tar.gz" .
