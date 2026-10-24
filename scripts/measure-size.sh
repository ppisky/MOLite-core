#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/build.sh
"${PYTHON:-python3}" scripts/measure-size.py
