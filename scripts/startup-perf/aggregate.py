"""Aggregate illuTag startup harness summaries into a P50/P90 table.

Usage:  python aggregate.py [labelPrefix]
Env:    ILLUTAG_OUT (defaults to %TEMP%\\illutag-startup-perf)
"""
import json, glob, os, re, sys, math

OUT = os.environ.get("ILLUTAG_OUT") or os.path.join(os.environ.get("TEMP", "."), "illutag-startup-perf")

def load(label):
    with open(os.path.join(OUT, f"{label}.summary.json"), encoding="utf-8") as f:
        s = json.load(f)
    cons = s.get("console_startup_prof", [])
    stages = s.get("stderr_stages", [])
    def find_console(sub):
        return next((c for c in cons if sub in str(c.get("text", ""))), None)
    def find_stage(sub):
        return next((c for c in stages if sub in c["line"]), None)
    m = {"label": label}
    if s.get("mem") and "peak" in s["mem"]:
        m["ws_mb"] = round(s["mem"]["ws"] / 1048576)
        m["peak_mb"] = round(s["mem"]["peak"] / 1048576)
        m["cpu_s"] = round(s["mem"].get("cpu", 0), 1)
    db = find_stage("[database] using")
    lib = find_stage("list_library_command begin")
    ls = find_stage("load_store folders_ms")
    ll = find_stage("list_library_from_state")
    if db: m["db_ready_ms"] = round(db["t_ms"])
    if lib: m["lib_begin_ms"] = round(lib["t_ms"])
    if db and lib: m["setup_block_ms"] = round(lib["t_ms"] - db["t_ms"])
    if ls:
        for k in ["folders_ms", "images_ms", "user_folders_ms", "image_folders_ms", "total_ms"]:
            mm = re.search(k + r"=(\d+)", ls["line"])
            if mm: m[k] = int(mm.group(1))
        mm = re.search(r"images:(\d+)/(\d+)", ls["line"])
        if mm: m["images_loaded"], m["images_total"] = int(mm.group(1)), int(mm.group(2))
    if ll:
        mm = re.search(r"load_store_ms=(\d+)", ll["line"])
        if mm: m["rust_load_store_ms"] = int(mm.group(1))
    c = find_stage("list_library_command total_ms")
    if c: m["rust_cmd_total_ms"] = int(re.search(r"total_ms=(\d+)", c["line"]).group(1))
    ic = find_console("loadLibrary invoke_start_ms")
    ir = find_console("loadLibrary invoke_return_ms")
    if ic and ir:
        m["ipc_roundtrip_ms"] = round(ir["t_ms"] - ic["t_ms"])
        m["invoke_return_at_ms"] = round(ir["t_ms"])
    cm = find_console("SegmentedMasonry mounted_ms")
    if cm: m["masonry_mounted_at_ms"] = round(cm["t_ms"])
    ihl = find_console("index.html inline_head_ms")
    if ihl: m["inline_head_at_ms"] = round(ihl["t_ms"])
    return m

def pctl(vals, p):
    vals = sorted(v for v in vals if v is not None)
    if not vals:
        return None
    idx = min(len(vals) - 1, int(math.ceil(p / 100 * len(vals))) - 1)
    return vals[max(0, idx)]

prefix = sys.argv[1] if len(sys.argv) > 1 else ""
labels = sorted(set(os.path.basename(p).split(".")[0] for p in glob.glob(os.path.join(OUT, "*.summary.json"))))
labels = [l for l in labels if l.startswith(prefix)]
if not labels:
    print("no summaries found in", OUT)
    sys.exit(1)
rows = [load(l) for l in labels]
keys = sorted({k for r in rows for k in r})
for r in rows:
    print(" | ".join(f"{k}={r.get(k)}" for k in keys if k in r))

print("\n=== P50 / P90 across runs ===")
for k in ["db_ready_ms", "setup_block_ms", "rust_load_store_ms", "image_folders_ms",
          "ipc_roundtrip_ms", "invoke_return_at_ms", "masonry_mounted_at_ms", "peak_mb", "cpu_s"]:
    vals = [r.get(k) for r in rows if r.get(k) is not None]
    if vals:
        print(f"{k:22s} P50={pctl(vals,50)} P90={pctl(vals,90)} min={min(vals)} max={max(vals)} n={len(vals)}")
