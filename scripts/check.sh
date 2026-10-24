#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python_bin="${PYTHON:-python3}"
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --release --locked
cargo doc --locked --no-deps
bash scripts/build.sh
"$python_bin" scripts/verify-packed.py target/release
case "$(uname -s)" in
  Linux|Darwin) bash scripts/test-c-abi.sh ;;
  MINGW*|MSYS*|CYGWIN*) cmd.exe //c 'scripts\test-c-abi.cmd target\release' ;;
  *) echo "Unsupported C host" >&2; exit 1 ;;
esac
