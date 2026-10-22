#!/usr/bin/env python3
"""Read-only history RPC probe. Emits timings/counts, never chat text or IDs."""
import argparse
import json
from pathlib import Path
import resource
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from kai import AppServer


def cpu_seconds(pid):
    # The npm CLI is a wrapper around the native server. Include descendants
    # so the CPU result does not accidentally measure only its idle wrapper.
    raw = subprocess.check_output(["/bin/ps", "-axo", "pid=,ppid=,time="], text=True)
    rows = []
    for line in raw.splitlines():
        child, parent, duration = line.split()
        seconds = sum(float(part)*60**index for index,part in enumerate(reversed(duration.split(":"))))
        rows.append((int(child), int(parent), seconds))
    family = {pid}
    while True:
        expanded = family | {child for child,parent,_ in rows if parent in family}
        if expanded == family:
            break
        family = expanded
    return sum(seconds for child,_,seconds in rows if child in family)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex", default="codex")
    parser.add_argument("--thread")
    parser.add_argument("--catalog", action="store_true", help="Compare routine catalog scanning with indexed metadata reads")
    parser.add_argument("--repeat", type=int, default=3)
    args = parser.parse_args()
    thread = None if args.catalog else args.thread or subprocess.check_output(["/usr/bin/defaults", "read", "com.kai.notchprototype", "lastOpenedChatID"], text=True).strip()
    if not args.catalog and (not thread or thread.startswith("code:")):
        raise SystemExit("Select a Codex chat in Kai first, or pass --thread.")
    server = AppServer(args.codex)
    results = []
    try:
        metadata = {} if args.catalog else server.call("thread/read", {"threadId":thread, "includeTurns":False}).get("thread") or {}
        raw_path = metadata.get("path")
        size = Path(raw_path).stat().st_size if raw_path and Path(raw_path).is_file() else None
        operations = [
                ("scan_catalog", "thread/list", {"limit":100,"sourceKinds":["cli","vscode","exec"],"sortKey":"updated_at","useStateDbOnly":False}),
                ("indexed_catalog", "thread/list", {"limit":100,"sourceKinds":["cli","vscode","exec"],"sortKey":"updated_at","useStateDbOnly":True}),
            ] if args.catalog else [
                ("full_history", "thread/read", {"threadId":thread, "includeTurns":True}),
                ("recent_turns", "thread/turns/list", {"threadId":thread, "limit":3, "itemsView":"full", "sortDirection":"desc"}),
            ]
        for _ in range(max(1, min(args.repeat, 10))):
            for name, method, params in operations:
                backend_before = cpu_seconds(server.proc.pid)
                own_before = resource.getrusage(resource.RUSAGE_SELF)
                started = time.monotonic()
                result = server.call(method, params)
                elapsed = time.monotonic() - started
                own_after = resource.getrusage(resource.RUSAGE_SELF)
                turns = result.get("data", []) if method.endswith("/list") else (result.get("thread") or {}).get("turns", [])
                record = {"operation":name, "wall_ms":round(elapsed*1000, 2),
                          "backend_cpu_ms":round((cpu_seconds(server.proc.pid)-backend_before)*1000, 2),
                          "python_cpu_ms":round((own_after.ru_utime+own_after.ru_stime-own_before.ru_utime-own_before.ru_stime)*1000, 2),
                          "response_bytes":len(json.dumps(result, separators=(",", ":")).encode()),
                          "turns":len(turns), "items":sum(len(turn.get("items") or []) for turn in turns)}
                results.append(record)
                print(json.dumps(record), flush=True)
                del result, turns
        print(json.dumps({"rollout_bytes":size, "runs":len(results)}), flush=True)
    finally:
        server.close()


if __name__ == "__main__":
    main()
