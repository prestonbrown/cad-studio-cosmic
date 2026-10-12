#!/usr/bin/env python3
"""Fake claude CLI for engine tests. Emits a scripted stream-json scenario.

Env: CAD_FAKE_SCENARIO = scenario .jsonl file, OR a directory (directory mode:
     picks <dir>/<n>.jsonl where n = value in <dir>/counter + 1, writing the
     counter back — starts at 1 on the first invocation).
     CAD_FAKE_ARGV_OUT = path where one JSON line {"argv": [...], "cwd": "...",
     "env": {...}} is APPENDED per invocation (never overwritten; spawn counts
     read line counts; env carries MPLBACKEND/PYTHONUNBUFFERED as received).
     CAD_FAKE_STDERR = optional file copied to stderr after the scenario.
Scenario directives (lines starting with '#'):  #sleep <ms>   pause before next line
                                                #exit <code>  exit code (default 0)
All other lines are written verbatim to stdout, flushed one by one.
"""
import json
import os
import sys
import time
from pathlib import Path

argv_out = os.environ.get("CAD_FAKE_ARGV_OUT")
if argv_out:
    # env: selected variables as the child actually received them
    env = {k: os.environ.get(k) for k in ("MPLBACKEND", "PYTHONUNBUFFERED")}
    with open(argv_out, "a") as f:
        f.write(json.dumps({"argv": sys.argv[1:], "cwd": os.getcwd(), "env": env}) + "\n")

scenario_path = Path(os.environ["CAD_FAKE_SCENARIO"])
if scenario_path.is_dir():
    counter = scenario_path / "counter"
    n = int(counter.read_text()) + 1 if counter.exists() else 1
    counter.write_text(str(n))
    scenario_path = scenario_path / f"{n}.jsonl"

code = 0
for line in scenario_path.read_text().splitlines():
    if line.startswith("#sleep"):
        time.sleep(int(line.split()[1]) / 1000.0)
        continue
    if line.startswith("#exit"):
        code = int(line.split()[1])
        continue
    sys.stdout.write(line + "\n")
    sys.stdout.flush()

stderr_copy = os.environ.get("CAD_FAKE_STDERR")
if stderr_copy:
    sys.stderr.write(Path(stderr_copy).read_text())
    sys.stderr.flush()
sys.exit(code)
