#!/usr/bin/env bash
# Windows packing uses the pinned tool downloader and PE checks in the PS helper.
set -euo pipefail
cd "$(dirname "$0")/.."
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) powershell.exe -NoProfile -File scripts/pack-upx.ps1 ;;
  *) echo "UPX packaging is currently verified for Windows x64. Use Git Bash for this entry point; native Linux/macOS builds use build.sh and package-release.sh." >&2; exit 2 ;;
esac
