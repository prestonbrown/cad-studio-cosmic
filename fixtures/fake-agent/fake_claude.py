#!/usr/bin/env python3
"""Fake claude CLI for engine tests. Emits a scripted stream-json scenario.

Env: CAD_FAKE_SCENARIO = scenario .jsonl file, OR a directory (directory mode:
     picks <dir>/<n>.jsonl where n = value in <dir>/counter + 1, writing the
     counter back — starts at 1 on the first invocation).
     CAD_FAKE_ARGV_OUT = path where one JSON line {"argv": [...], "cwd": "...",
     "env": {...}} is APPENDED per invocation (never overwritten; spawn counts
     read line counts; env carries MPLBACKEND/PYTHONUNBUFFERED as received).
     CAD_FAKE_STDERR = optional file copied to stderr after the scenario.
     CAD_FAKE_COPY_FROM = source dir for the #copy-outputs directive.
Scenario directives (lines starting with '#'):  #sleep <ms>   pause before next line
                                                #exit <code>  exit code (default 0)
                                                #copy-outputs write model.py + the output set
All other lines are written verbatim to stdout, flushed one by one.
"""
import json
import os
import shutil
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


def copy_outputs():
    """The agent's tool side-effects: model.py at the project root and the
    complete output set (the 7 expected outputs + plate + page 2 — the
    state missing_requirements reports nothing for) into ./output/. The
    file list extends fake_python.py's copy mode, which the engine's model
    runner tests use for the same fixtures."""
    src = Path(os.environ["CAD_FAKE_COPY_FROM"])
    out = Path("output")
    out.mkdir(exist_ok=True)
    if not Path("model.py").exists():
        Path("model.py").write_text("# model.py — written by the fake agent\n")
    for name in ("model.3mf", "model.stl", "model.step", "schematic.svg",
                 "schematic.png", "schematic.pdf", "manifest.json",
                 "schematic-2.svg", "plate.3mf"):
        dst = out / name
        if not dst.exists():
            # copyfile, not copy2: preserving the fixture's mtime would read
            # as content older than the watch and never emit a reload
            shutil.copyfile(src / name, dst)


code = 0
for line in scenario_path.read_text().splitlines():
    if line.startswith("#sleep"):
        time.sleep(int(line.split()[1]) / 1000.0)
        continue
    if line.startswith("#exit"):
        code = int(line.split()[1])
        continue
    if line.startswith("#copy-outputs"):
        copy_outputs()
        continue
    sys.stdout.write(line + "\n")
    sys.stdout.flush()

stderr_copy = os.environ.get("CAD_FAKE_STDERR")
if stderr_copy:
    sys.stderr.write(Path(stderr_copy).read_text())
    sys.stderr.flush()
sys.exit(code)
