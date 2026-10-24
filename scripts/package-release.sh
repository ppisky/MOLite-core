#!/usr/bin/env bash
# Pass the platform label, then optionally an already-verified binary directory.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "Usage: bash scripts/package-release.sh PLATFORM [BINARY_DIRECTORY]" >&2
  exit 2
fi
"${PYTHON:-python3}" scripts/package-release.py "$@"
