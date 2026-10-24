"""Create a platform archive with source provenance and dependency license texts."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import zipfile

root = Path(__file__).resolve().parent.parent
os.chdir(root)
label = sys.argv[1]
source = Path(sys.argv[2] if len(sys.argv) > 2 else "target/release")
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version=1"]))
package = next(p for p in metadata["packages"] if p["name"] == "molite-core")
version = package["version"]
tag = os.environ.get("GITHUB_REF", "")
if tag.startswith("refs/tags/") and tag != "refs/tags/v" + version:
    raise SystemExit("tag must match Cargo.toml version")
name = f"molite-core-{version}-{label}"
stage = root / "target" / "packages" / name
if stage.exists():
    raise SystemExit(f"package staging directory already exists: {stage}")
stage.mkdir(parents=True)
for filename in ["LICENSE", "NOTICE", "README.md", "README.zh-CN.md"]:
    shutil.copy2(root / filename, stage)
shutil.copytree(root / "include", stage / "include")
shutil.copytree(root / "docs", stage / "docs")
for filename in ["molite-server.exe", "molite-server", "molite_core.dll", "molite_core.dll.lib", "molite_core.lib", "libmolite_core.so", "libmolite_core.dylib", "libmolite_core.a"]:
    if (source / filename).is_file():
        shutil.copy2(source / filename, stage)
licenses = []
for dependency in metadata["packages"]:
    if dependency["name"] == "molite-core":
        continue
    base = Path(dependency["manifest_path"]).parent
    target = stage / "licenses" / (dependency["name"] + "-" + dependency["version"])
    texts = []
    for path in base.rglob("*"):
        if path.is_file() and path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT")):
            relative = path.relative_to(base)
            destination = target / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, destination)
            texts.append(str(relative))
    licenses.append({"name": dependency["name"], "version": dependency["version"], "license": dependency["license"], "repository": dependency["repository"], "texts": texts})
(stage / "THIRD-PARTY.json").write_text(json.dumps(licenses, indent=2) + "\n", encoding="utf-8")
(stage / "BUILD.json").write_text(json.dumps({"commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(), "rustc": subprocess.check_output(["rustc", "-V"], text=True).strip(), "target": label, "lock_sha256": hashlib.sha256(Path("Cargo.lock").read_bytes()).hexdigest()}, indent=2) + "\n", encoding="utf-8")
dist = root / "dist"
dist.mkdir(exist_ok=True)
if "windows" in label:
    archive = dist / (name + ".zip")
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9, strict_timestamps=False) as output:
        for path in stage.rglob("*"):
            if path.is_file():
                output.write(path, path.relative_to(stage.parent))
else:
    archive = dist / (name + ".tar.gz")
    with tarfile.open(archive, "w:gz") as output:
        output.add(stage, arcname=name)
checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
archive.with_name(archive.name + ".sha256").write_text(f"{checksum}  {archive.name}\n", encoding="utf-8")
print(archive)
