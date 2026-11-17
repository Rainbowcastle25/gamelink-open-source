#!/usr/bin/env python3
"""Run the hermetic Tailcat resource profile and save its /proc snapshots.

Requires GAMELINK_AGENT_BINARY to point at the built Rust agent. Each phase
emits a snapshot for the test process (clients + relay) and for the agent child.
"""

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("performance-results.json"))
    parser.add_argument("--go", default="go", help="Go executable (default: go on PATH)")
    args = parser.parse_args()

    env = os.environ.copy()
    env["GAMELINK_RESOURCE_PROFILE"] = "1"
    proc = subprocess.run(
        [args.go, "test", "-run", "^TestResourceProfile$", "-count=1", "-v", "./tests/integration"],
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    print(proc.stdout, end="")
    if proc.returncode:
        return proc.returncode

    samples = [json.loads(item) for item in re.findall(r"RESOURCE (\{.*\})", proc.stdout)]
    if not samples:
        print("resource profile produced no samples", file=sys.stderr)
        return 1
    args.output.write_text(json.dumps(samples, indent=2) + "\n", encoding="utf-8")
    print(f"Saved {len(samples)} snapshots to {args.output}")
    print("Process       Phase                    Path      RSS MiB   Heap MiB   Goroutines   Threads   FDs")
    for sample in samples:
        print(
            f"{sample.get('process', ''):<13} {sample['phase']:<24} {sample['path']:<9} "
            f"{sample['rss_bytes'] / 1048576:8.1f} "
            f"{sample['alloc_bytes'] / 1048576:10.1f} "
            f"{sample['goroutines']:12d} {sample['threads']:9d} {sample['open_fds']:5d}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
