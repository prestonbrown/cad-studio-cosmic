#!/usr/bin/env python3
"""Golden-fixture sanity check: schemas, sizes, 3MF structure."""
import json
import sys
import zipfile
from pathlib import Path

golden = Path(__file__).resolve().parent.parent / "golden"

m = json.loads((golden / "manifest.json").read_text())
assert m["parameters"]["WALL"] == 2.0
assert {p["name"] for p in m["parts"]} == {"base", "lid"}
assert m["bed"] == [220.0, 220.0]

a = json.loads((golden / "animation.json").read_text())
assert len(a["frames"]) == 3
assert a["frames"][0]["parts"]["lid"]["translate"] == [0.0, 0.0, 0.0]

for name in ["model.3mf", "model.stl", "model.step", "plate.3mf", "schematic.svg",
             "schematic-2.svg", "schematic.pdf", "schematic.png"]:
    p = golden / name
    assert p.is_file() and p.stat().st_size > 200, f"{name} missing or tiny"

with zipfile.ZipFile(golden / "model.3mf") as z:
    xml = z.read("3D/3dmodel.model").decode()
n_objects = xml.count("<object ")
print(f"manifest+animation OK; 3MF objects: {n_objects}")
if n_objects < 2:
    sys.exit("3MF must contain >=2 objects for the parser spike")
