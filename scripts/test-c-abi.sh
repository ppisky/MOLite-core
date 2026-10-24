#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
mkdir -p target/c-smoke
case "$(uname -s)" in
  Darwin) export DYLD_LIBRARY_PATH="$root/target/release${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" ;;
  Linux) export LD_LIBRARY_PATH="$root/target/release${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
  *) echo "Unsupported host" >&2; exit 1 ;;
esac
cc -Wall -Wextra -Werror -Iinclude examples/c_smoke.c -Ltarget/release -lmolite_core -o target/c-smoke/c_smoke
target/c-smoke/c_smoke target/c-smoke-data
