"""M0 golden fixture generator: two-part colored model, plate, drawing, manifest, animation.

Run: ~/.local/share/cad-studio/venv/bin/python fixtures/generator/model.py  (from repo root)
"""
import json
import math
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.backends.backend_pdf import PdfPages

from build123d import Box, Color, Compound, Mesher, Pos, Rot, export_step, export_stl

OUT = Path(__file__).resolve().parent.parent / "golden"
OUT.mkdir(parents=True, exist_ok=True)

WALL = 2.0    # wall thickness (mm)
HEIGHT = 8.0  # base height (mm)
LID_H = 10.0  # lid height (mm)

base = Box(60, 60, HEIGHT)
base.label = "base"
base.color = Color((0.12, 0.47, 0.71))

lid = Pos(0, 0, HEIGHT + LID_H / 2 + 4) * Box(40, 40, LID_H)
lid.label = "lid"
lid.color = Color((0.84, 0.15, 0.16))

mesher = Mesher()
mesher.add_shape([base, lid])
mesher.write(OUT / "model.3mf")
export_stl(base + lid, OUT / "model.stl")
export_step(Compound(children=[base, lid]), OUT / "model.step")

plate = Mesher()
plate.add_shape([base, Pos(0, 45, LID_H / 2) * Rot(0, 0, 0) * Box(40, 40, LID_H)])
plate.write(OUT / "plate.3mf")

# --- drawing (fixture quality: shapes + dims, not full ISO) ---
def page(ax, title, w, d, h):
    ax.add_patch(plt.Rectangle((-w / 2, -h / 2), w, h, fill=False, lw=0.5))
    ax.annotate(f"{w} x {d} x {h}", (0, 0), ha="center", va="center", fontsize=6)
    ax.set_title(title, fontsize=8)
    ax.set_xlim(-50, 50); ax.set_ylim(-50, 50); ax.set_aspect("equal")
    ax.axis("off")

fig1, ax1 = plt.subplots(figsize=(11.69, 8.27))
page(ax1, "fixture-box — assembly", 60, 60, HEIGHT + LID_H)
fig1.savefig(OUT / "schematic.svg")
fig1.savefig(OUT / "schematic.png", dpi=300)
fig2, ax2 = plt.subplots(figsize=(11.69, 8.27))
page(ax2, "fixture-box — lid", 40, 40, LID_H)
fig2.savefig(OUT / "schematic-2.svg")
with PdfPages(OUT / "schematic.pdf") as pdf:
    pdf.savefig(fig1); pdf.savefig(fig2)

assembly = base + lid
bb = assembly.bounding_box()
manifest = {
    "name": "fixture-box",
    "description": "M0 golden fixture: two-part box",
    "units": "mm",
    "bounding_box": {"x": round(bb.size.X, 1), "y": round(bb.size.Y, 1), "z": round(bb.size.Z, 1)},
    "volume_mm3": round(float(base.volume + lid.volume), 1),
    "surface_area_mm2": round(float(base.area + lid.area), 1),
    "parameters": {"WALL": WALL, "HEIGHT": HEIGHT, "LID_H": LID_H},
    "iteration": 1,
    "warnings": ["fixture warning"],
    "parts": [
        {"name": "base", "quantity": 1, "color": "#1f77b4"},
        {"name": "lid", "quantity": 1, "color": "#d62728"},
    ],
    "drawings": [
        {"file": "schematic.svg", "title": "Assembly"},
        {"file": "schematic-2.svg", "title": "Lid"},
    ],
    "bed": [220.0, 220.0],
}
(OUT / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

animation = {
    "title": "lid lift",
    "frames": [
        {"t": 0.0, "label": "closed", "parts": {"lid": {"translate": [0, 0, 0], "rotate": [0, 0, 0, 1]}},
         "values": {"Lift (mm)": 0.0}, "collisions": []},
        {"t": 1.0, "label": "open", "parts": {"lid": {"translate": [0, 0, 10.0], "rotate": [0, 0, 0, 1]}},
         "values": {"Lift (mm)": 10.0}, "collisions": [{"parts": ["base", "lid"], "volume": 0.5}]},
        {"t": 2.0, "label": "open", "parts": {"lid": {"translate": [0, 0, 10.0], "rotate": [0, 0, 0, 1]}},
         "values": {"Lift (mm)": 10.0}, "collisions": []},
    ],
}
(OUT / "animation.json").write_text(json.dumps(animation, indent=2) + "\n")
print("fixtures written to", OUT)
