"""Bounded, synthetic SQLite comparison of old/new duplicate keeper lookup.

Run with bundled Python, passing a NEW absolute directory directly beneath
D:/Ai/illuTag-refactor-work (name must start duplicates-1.5.1-sql-).
No app is launched, no real database/images are opened, and no files are deleted.
Fixture construction intentionally leaves SQLite statistics absent, matching a
new bulk journal. OS caches are warm from construction; this is NOT a cold-disk
benchmark. Separate reader connections reset only SQLite's own page caches.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import sqlite3
import time


BASELINE = """SELECT DISTINCT k.image_pk,k.payload
FROM bulk_members m
JOIN bulk_group_keepers g ON g.operation=m.operation AND g.group_no=m.group_no
JOIN bulk_keepers k ON k.operation=g.operation AND k.image_pk=g.image_pk
WHERE m.operation=?1 AND m.image_pk=?2 ORDER BY k.image_pk"""

OPTIMIZED = """SELECT DISTINCT k.image_pk,k.payload
FROM bulk_members m INDEXED BY bulk_members_image
CROSS JOIN bulk_group_keepers g ON g.operation=m.operation AND g.group_no=m.group_no
CROSS JOIN bulk_keepers k ON k.operation=g.operation AND k.image_pk=g.image_pk
WHERE m.operation=?1 AND m.image_pk=?2 ORDER BY k.image_pk"""

SCHEMA = """
CREATE TABLE bulk_members(
 operation INTEGER NOT NULL,group_no INTEGER NOT NULL,image_pk INTEGER NOT NULL,
 PRIMARY KEY(operation,group_no,image_pk)) WITHOUT ROWID;
CREATE INDEX bulk_members_image ON bulk_members(operation,image_pk,group_no);
CREATE TABLE bulk_group_keepers(
 operation INTEGER NOT NULL,group_no INTEGER NOT NULL,image_pk INTEGER NOT NULL,
 PRIMARY KEY(operation,group_no,image_pk)) WITHOUT ROWID;
CREATE TABLE bulk_keepers(
 operation INTEGER NOT NULL,image_pk INTEGER NOT NULL,payload TEXT NOT NULL,
 PRIMARY KEY(operation,image_pk)) WITHOUT ROWID;
"""


def output(record):
    print(json.dumps(record, ensure_ascii=False), flush=True)


def fenced_root(value):
    supplied = Path(value)
    fence = Path("D:/Ai/illuTag-refactor-work").resolve(strict=True)
    if not supplied.is_absolute():
        raise ValueError("Absolute fresh benchmark root required")
    # Resolve the existing parent, including Windows junctions, BEFORE creating
    # anything. A root nested under a link or an existing test is not accepted.
    if supplied.exists() or supplied.is_symlink():
        raise ValueError("Benchmark root must be completely new")
    if supplied.parent.resolve(strict=True) != fence:
        raise ValueError("Benchmark root must be a direct child of the real QA fence")
    if not supplied.name.startswith("duplicates-1.5.1-sql-"):
        raise ValueError("Expected duplicates-1.5.1-sql-* root name")
    resolved = supplied.resolve(strict=False)
    if resolved.parent != fence:
        raise ValueError("Resolved benchmark output escapes QA fence")
    return resolved


def reader(database):
    connection = sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)
    connection.execute("PRAGMA query_only=ON")
    connection.execute("PRAGMA cache_size=-2048")
    connection.execute("PRAGMA mmap_size=0")
    connection.execute("PRAGMA temp_store=FILE")
    return connection


def digest(rows):
    return hashlib.sha256(json.dumps(rows, separators=(",", ":")).encode()).hexdigest()


def payload(pk, size):
    return json.dumps({"pk": pk, "syntheticOnly": True, "padding": "q" * size}, separators=(",", ":"))


def run_batch(connection, query, candidates, expected, rounds, seconds_limit):
    ticks = 0
    started = time.perf_counter()
    deadline = started + seconds_limit

    def progress():
        nonlocal ticks
        ticks += 1
        return time.perf_counter() > deadline

    connection.set_progress_handler(progress, 1000)
    completed = 0
    signatures = {}
    interrupted = False
    try:
        for _ in range(rounds):
            for label, image_pk in candidates:
                rows = connection.execute(query, (1, image_pk)).fetchall()
                assert rows == expected[image_pk], f"Different keeper set for {label}"
                signatures[label] = {"keeperIds": [row[0] for row in rows], "resultDigest": digest(rows)}
                completed += 1
    except sqlite3.OperationalError as error:
        if "interrupt" not in str(error).lower():
            raise
        interrupted = True
    finally:
        connection.set_progress_handler(None, 0)
    elapsed = time.perf_counter() - started
    return {
        "seconds": elapsed,
        "completedQueries": completed,
        "requestedQueries": rounds * len(candidates),
        "secondsPerCompletedQuery": elapsed / completed if completed else None,
        "vmProgressCallbacks": ticks,
        "vmInstructionsEstimate": ticks * 1000,
        "vmInstructionsEstimateGranularity": 1000,
        "timeLimitSeconds": seconds_limit,
        "timeLimitReached": interrupted,
        "results": signatures,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root")
    parser.add_argument("--groups", type=int, default=25414)
    parser.add_argument("--samples", type=int, default=12)
    parser.add_argument("--payload-bytes", type=int, default=1024)
    parser.add_argument("--baseline-seconds", type=float, default=12)
    parser.add_argument("--optimized-rounds", type=int, default=100)
    arguments = parser.parse_args()
    assert 1000 <= arguments.groups <= 30000
    assert 6 <= arguments.samples <= 32
    assert 128 <= arguments.payload_bytes <= 2048
    assert 1 <= arguments.baseline_seconds <= 20
    assert 1 <= arguments.optimized_rounds <= 200
    root = fenced_root(arguments.root)
    root.mkdir()
    temporary = root / "temp"
    temporary.mkdir()
    os.environ["TEMP"] = str(temporary)
    os.environ["TMP"] = str(temporary)
    os.environ["SQLITE_TMPDIR"] = str(temporary)
    database = root / "synthetic-planning.sqlite"
    report_path = root / "duplicate-planning-benchmark.json"
    report = {
        "passed": False,
        "syntheticOnly": True,
        "productionTouched": False,
        "appLaunched": False,
        "startedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "sqliteVersion": sqlite3.sqlite_version,
        "root": str(root),
        "baselineSql": BASELINE,
        "optimizedSql": OPTIMIZED,
        "cacheLimitKiB": 2048,
        "timingScope": "SQL-only; fixture generation excluded; OS file cache warm from construction. Fresh SQLite reader caches per variant, followed by warm repetitions. No OS cache flushing.",
        "vmMeasurement": "SQLite progress-handler callbacks every 1,000 virtual-machine instructions, including queries used for equivalence assertions; estimates have 1,000-instruction granularity.",
        "phases": [],
    }
    began = time.perf_counter()
    connection = None
    try:
        n = arguments.groups
        connection = sqlite3.connect(database)
        connection.executescript("PRAGMA journal_mode=DELETE; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;" + SCHEMA)
        members = [(1, group, pk) for group in range(1, n + 1) for pk in (group * 2 - 1, group * 2)]
        # Default: 50,828 members plus 4,172 additional distinct versions.
        extras = min(55000 - len(members), n)
        for offset in range(max(0, extras)):
            members.append((1, offset + 1, n * 2 + offset + 1))
        overlap_pk = n * 3 + 1
        members.extend((1, group, overlap_pk) for group in (1, 2, 3))
        # Keeper 1 occurs in three groups, so the optimized query must preserve
        # DISTINCT semantics even when several indexed paths find the same image.
        members.extend([(1, 2, 1), (1, 3, 1)])
        connection.executemany("INSERT INTO bulk_members VALUES(?,?,?)", members)
        connection.commit()
        report["fixture"] = {"groups": n, "memberRows": len(members), "payloadPaddingBytes": arguments.payload_bytes, "overlappingCandidatePk": overlap_pk}
        source_groups = {}
        for _, group, pk in members:
            source_groups.setdefault(pk, []).append(group)
        phases = sorted(set([1000, min(10000, n), n]))
        installed = 0
        rng = random.Random(20261015)
        for count in phases:
            with connection:
                for group in range(installed + 1, count + 1):
                    pk = group * 2 - 1
                    connection.execute("INSERT INTO bulk_keepers VALUES(1,?,?)", (pk, payload(pk, arguments.payload_bytes)))
                    connection.execute("INSERT INTO bulk_group_keepers VALUES(1,?,?)", (group, pk))
                if installed == 0:
                    connection.executemany("INSERT INTO bulk_group_keepers VALUES(1,?,1)", [(2,), (3,)])
            installed = count
            candidates = [("overlap-deduplicated", overlap_pk), ("first-group", 2), ("last-populated-group", count * 2)]
            if count < n:
                candidates.append(("no-keeper-in-unvisited-group", (count + 1) * 2))
            while len(candidates) < arguments.samples:
                group = rng.randint(4, count)
                candidate = (f"group-{group}", group * 2)
                if candidate not in candidates:
                    candidates.append(candidate)
            expected = {}
            for _, image_pk in candidates:
                keeper_pks = set()
                for group in source_groups[image_pk]:
                    if group <= count:
                        keeper_pks.add(group * 2 - 1)
                    if group in (2, 3):
                        keeper_pks.add(1)
                expected[image_pk] = [(pk, payload(pk, arguments.payload_bytes)) for pk in sorted(keeper_pks)]
            baseline_reader, optimized_reader = reader(database), reader(database)
            try:
                old_plan = baseline_reader.execute("EXPLAIN QUERY PLAN " + BASELINE, (1, overlap_pk)).fetchall()
                new_plan = optimized_reader.execute("EXPLAIN QUERY PLAN " + OPTIMIZED, (1, overlap_pk)).fetchall()
                first = new_plan[0][3]
                assert "bulk_members_image" in first and "operation=?" in first and "image_pk=?" in first, new_plan
                assert "group_no=?" in new_plan[1][3], new_plan
                phase = {"keeperCount": count, "baselinePlan": old_plan, "optimizedPlan": new_plan, "candidateLookupAsserted": True, "expectedOverlapUniqueKeeperCount": 3}
                # Both variants start with their own freshly opened SQLite cache.
                phase["baselineFreshReader"] = run_batch(baseline_reader, BASELINE, candidates, expected, 1, arguments.baseline_seconds)
                phase["optimizedFreshReader"] = run_batch(optimized_reader, OPTIMIZED, candidates, expected, 1, 5)
                # Repeated optimized work is long enough to provide measurable
                # VM counts without requiring an entire slow legacy batch.
                phase["optimizedWarmRepeated"] = run_batch(optimized_reader, OPTIMIZED, candidates, expected, arguments.optimized_rounds, 10)
                assert phase["optimizedFreshReader"]["completedQueries"] == len(candidates)
                assert phase["optimizedWarmRepeated"]["completedQueries"] == len(candidates) * arguments.optimized_rounds
                assert "overlap-deduplicated" in phase["baselineFreshReader"]["results"], "Baseline budget too short to verify overlap equivalence"
                assert phase["baselineFreshReader"]["results"]["overlap-deduplicated"] == phase["optimizedFreshReader"]["results"]["overlap-deduplicated"]
                for label, result in phase["baselineFreshReader"]["results"].items():
                    assert result == phase["optimizedFreshReader"]["results"][label]
                old_cost = phase["baselineFreshReader"]["secondsPerCompletedQuery"]
                new_cost = phase["optimizedWarmRepeated"]["secondsPerCompletedQuery"]
                phase["observedSecondsPerQueryRatio"] = old_cost / new_cost if old_cost and new_cost else None
                report["phases"].append(phase)
                output({"keeperCount": count, "baselineQueries": phase["baselineFreshReader"]["completedQueries"], "baselineSeconds": phase["baselineFreshReader"]["seconds"], "optimizedWarmQueries": phase["optimizedWarmRepeated"]["completedQueries"], "optimizedWarmSeconds": phase["optimizedWarmRepeated"]["seconds"], "observedRatio": phase["observedSecondsPerQueryRatio"], "candidateLookupAsserted": True})
            finally:
                baseline_reader.close()
                optimized_reader.close()
        report["passed"] = True
    except Exception as error:
        report["error"] = repr(error)
        raise
    finally:
        if connection is not None:
            connection.close()
        report["durationSeconds"] = time.perf_counter() - began
        report["databaseBytes"] = database.stat().st_size if database.exists() else 0
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        output({"passed": report["passed"], "report": str(report_path), "durationSeconds": report["durationSeconds"]})


if __name__ == "__main__":
    main()
