#!/usr/bin/env python3
"""Fake codex CLI for engine tests. Emits a scripted codex-JSON scenario
(the thread.started/item.*/turn.* lines CodexTranslator consumes).

Env: CAD_FAKE_SCENARIO = scenario .jsonl file.
     CAD_FAKE_ARGV_OUT = path where one JSON line {"argv": [...], "cwd":
     "..."} is APPENDED per invocation (spawn counts read line counts).
Scenario directives (lines starting with '#'):  #sleep <ms>  pause before next line
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
    with open(argv_out, "a") as f:
        f.write(json.dumps({"argv": sys.argv[1:], "cwd": os.getcwd()}) + "\n")

code = 0
for line in Path(os.environ["CAD_FAKE_SCENARIO"]).read_text().splitlines():
    if line.startswith("#sleep"):
        time.sleep(int(line.split()[1]) / 1000.0)
        continue
    if line.startswith("#exit"):
        code = int(line.split()[1])
        continue
    sys.stdout.write(line + "\n")
    sys.stdout.flush()
sys.exit(code)
