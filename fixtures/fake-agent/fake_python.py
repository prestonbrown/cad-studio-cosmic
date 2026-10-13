#!/usr/bin/env python3
"""Fake venv python for engine tests (M5 T2/T6).

Env: CAD_FAKE_PY_MODE   = copy | fail | fail_stdout | hang | verify (default copy)
     CAD_FAKE_PY_FIXTURES = dir whose files are copied into ./output/ (mode copy)
     CAD_FAKE_PY_ARGV_OUT = file to append one JSON line {"argv","cwd","env"} per run
"""
import json, os, shutil, sys, time
from pathlib import Path

argv_out = os.environ.get("CAD_FAKE_PY_ARGV_OUT")
if argv_out:
    env = {k: os.environ.get(k) for k in ("MPLBACKEND", "PYTHONUNBUFFERED")}
    with open(argv_out, "a") as f:
        f.write(json.dumps({"argv": sys.argv[1:], "cwd": os.getcwd(), "env": env}) + "\n")

mode = os.environ.get("CAD_FAKE_PY_MODE", "copy")
if mode == "fail":
    sys.stdout.write("generating model\n")
    sys.stderr.write("Traceback (most recent call last):\n  File \"model.py\", line 5\n"
                     "ValueError: bad shape\n")
    sys.exit(1)
if mode == "fail_stdout":
    sys.stdout.write("line one\nboom: no stderr at all\n")
    sys.exit(3)
if mode == "hang":
    while True:
        time.sleep(0.2)
if mode == "verify":
    print("0.8.1")
    print("Python 3.12.4, build123d 0.8.1, matplotlib 3.9.2, numpy 2.1.0")
    sys.exit(0)
# copy
src = Path(os.environ["CAD_FAKE_PY_FIXTURES"])
out = Path("output")
out.mkdir(exist_ok=True)
for name in ("model.3mf", "model.stl", "model.step", "schematic.svg", "schematic.png",
             "schematic.pdf", "manifest.json"):
    dst = out / name
    if not dst.exists():
        # copyfile, not copy2: preserving the fixture's mtime would read
        # as content older than an output watch and never emit a reload
        # (fake_claude.py's copy_outputs carries the same comment)
        shutil.copyfile(src / name, dst)
sys.exit(0)
