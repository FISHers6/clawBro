#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <artifact-dir>" >&2
  exit 1
fi

ARTIFACT_DIR="$1"

cd "$ARTIFACT_DIR"
LC_ALL=C LANG=C shasum -a 256 ./*.tar.gz > SHA256SUMS
