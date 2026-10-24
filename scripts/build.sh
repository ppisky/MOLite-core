#!/usr/bin/env bash
# Linux, macOS and Git Bash: build the single supported release profile.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked --lib --bin molite-server
