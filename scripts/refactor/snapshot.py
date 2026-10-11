"""Create a consistent, non-mutating recovery snapshot of a portable install."""
import argparse
import hashlib
import json
import shutil
import sqlite3
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("source", type=Path)
parser.add_argument("destination", type=Path)
args = parser.parse_args()
source, destination = args.source.resolve(), args.destination.resolve()
if source == destination or destination.is_relative_to(source):
    raise SystemExit("Snapshot destination must be outside the installation")
destination.mkdir(parents=True, exist_ok=True)
db = source / "illutag.sqlite"
target = destination / db.name
if target.exists():
    raise SystemExit("Refusing to replace an existing snapshot")
with sqlite3.connect(db.as_uri() + "?mode=ro", uri=True) as src:
    with sqlite3.connect(target) as dst:
        src.backup(dst, pages=8192)
        check = dst.execute("PRAGMA quick_check").fetchone()[0]
        if check != "ok":
            raise SystemExit(f"Snapshot validation failed: {check}")
        counts = {table: dst.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()[0]
                  for (table,) in dst.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        version = dst.execute("PRAGMA user_version").fetchone()[0]
files = {}
for relative in ["illutag.exe", "illutag.portable.json", "window-state.json"]:
    path = source / relative
    if path.is_file():
        shutil.copy2(path, destination / relative)
for relative in ["scripts", "clipboard"]:
    if (source / relative).is_dir():
        shutil.copytree(source / relative, destination / relative, dirs_exist_ok=True)
for path in (source / "wd-swinv2-tagger-v3").glob("*.json"):
    (destination / "model-config").mkdir(exist_ok=True)
    shutil.copy2(path, destination / "model-config" / path.name)
for root in [source / "scripts", source / "wd-swinv2-tagger-v3", source / "model"]:
    for path in root.rglob("*"):
        if path.is_file() and path.suffix in {".py", ".json", ".csv", ".onnx", ".txt"}:
            digest = hashlib.file_digest(path.open("rb"), "sha256").hexdigest()
            files[str(path.relative_to(source))] = {"bytes": path.stat().st_size, "sha256": digest}
manifest = {"source": str(source), "schemaVersion": version, "quickCheck": check,
            "tableCounts": counts, "resourceManifest": files,
            "externalOriginalsIncluded": False,
            "frontendSettings": "Capture from the existing WebView before production replacement."}
(destination / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
print(json.dumps({"snapshot": str(target), "quickCheck": check, "schemaVersion": version, "counts": counts}))
