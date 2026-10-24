"""Measure native release artifacts on Windows, Linux or macOS after build.sh."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parent.parent
os.chdir(root)
files = (["molite_core.dll", "molite-server.exe"] if os.name == "nt" else
         ["libmolite_core.dylib" if sys.platform == "darwin" else "libmolite_core.so", "molite-server"])
output = Path(tempfile.mkdtemp(prefix="size-audit-", dir=root / "target"))
result = {
    "measured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "rustc": subprocess.check_output(["rustc", "-V"], text=True).strip(),
    "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "working_tree_changes": subprocess.check_output(["git", "status", "--porcelain"], text=True).splitlines(),
    "sqlite_flags_environment_override": os.environ.get("LIBSQLITE3_FLAGS"),
    "hashes": {name: hashlib.sha256(Path(name).read_bytes()).hexdigest() for name in ["Cargo.toml", "Cargo.lock", ".cargo/config.toml"]},
    "files": [],
}
for name in files:
    source = root / "target/release" / name
    result["files"].append({"file": name, "bytes": source.stat().st_size})
    shutil.copy2(source, output / name)
(output / "sizes.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
print(json.dumps(result, indent=2))
print("Results:", output)
