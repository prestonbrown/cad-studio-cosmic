#!/usr/bin/env python3
"""Handwritten 3MF fixtures for the full-parser tests (M1 task 4).

Each archive carries the minimal OPC parts a 3MF reader touches:
[Content_Types].xml + 3D/3dmodel.model. Member order and timestamps are
fixed so reruns are byte-identical (the artifacts are committed).
"""

import zipfile
from pathlib import Path

OUT = Path(__file__).resolve().parent

CONTENT_TYPES = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="model" '
    'ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>'
    '</Types>'
)

XMLNS = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02"

# The unit cube shared by every fixture: 0..1 per axis, 12 triangles.
CUBE_VERTS = (
    "<vertices>"
    '<vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/>'
    '<vertex x="1" y="1" z="0"/><vertex x="0" y="1" z="0"/>'
    '<vertex x="0" y="0" z="1"/><vertex x="1" y="0" z="1"/>'
    '<vertex x="1" y="1" z="1"/><vertex x="0" y="1" z="1"/>'
    "</vertices>"
)

CUBE_TRIS = [
    (0, 2, 1), (0, 3, 2),
    (4, 5, 6), (4, 6, 7),
    (0, 1, 5), (0, 5, 4),
    (1, 2, 6), (1, 6, 5),
    (2, 3, 7), (2, 7, 6),
    (3, 0, 4), (3, 4, 7),
]


def cube_mesh(extra=None):
    """Cube mesh XML; extra maps triangle index -> extra attribute string."""
    extra = extra or {}
    tris = "".join(
        f'<triangle v1="{a}" v2="{b}" v3="{c}"{extra.get(i, "")}/>'
        for i, (a, b, c) in enumerate(CUBE_TRIS)
    )
    return f"<mesh>{CUBE_VERTS}<triangles>{tris}</triangles></mesh>"


def model(inner, unit="millimeter"):
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        f'<model unit="{unit}" xml:lang="en-US" xmlns="{XMLNS}">\n'
        f"{inner}\n</model>\n"
    )


def write_3mf(name, model_xml):
    path = OUT / name
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for member, data in [("[Content_Types].xml", CONTENT_TYPES),
                             ("3D/3dmodel.model", model_xml)]:
            info = zipfile.ZipInfo(member, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)
    print(f"wrote {path.name} ({path.stat().st_size} bytes)")


# colorgroup: per-triangle p1 on the first two triangles; object pid with NO
# pindex, so triangle 2 onward falls through to nothing (uncolored). Green as
# #RRGGBB, red as #RRGGBBAA — both hex forms, alpha dropped.
COLORGROUP = model(
    "  <resources>\n"
    '    <colorgroup id="3">\n'
    '      <color color="#00FF00"/>\n'
    '      <color color="#FF0000FF"/>\n'
    "    </colorgroup>\n"
    '    <object id="1" pid="3">\n'
    f"      {cube_mesh({0: ' p1=\"0\"', 1: ' p1=\"1\"'})}\n"
    "    </object>\n"
    "  </resources>\n"
    "  <build><item objectid=\"1\"/></build>"
)

# units: a 1-unit cube in centimeters -> 10 mm wide.
UNITS = model(
    "  <resources>\n"
    '    <object id="1" name="centi">' + cube_mesh() + "</object>\n"
    "  </resources>\n"
    "  <build><item objectid=\"1\"/></build>",
    unit="centimeter",
)

# nested: two transform-composing component levels around the cube — inner
# +5 x (with a <components> wrapper), outer +7 y (bare <component>, no
# wrapper). The build item's object is unnamed and the middle object has an
# empty name, so the part name falls through to the mesh object "cube".
NESTED = model(
    "  <resources>\n"
    '    <object id="3" name="cube">' + cube_mesh() + "</object>\n"
    '    <object id="2" name="">\n'
    "      <components>\n"
    '        <component objectid="3" transform="1 0 0 0 1 0 0 0 1 5 0 0"/>\n'
    "      </components>\n"
    "    </object>\n"
    '    <object id="1">\n'
    '      <component objectid="2" transform="1 0 0 0 1 0 0 0 1 0 7 0"/>\n'
    "    </object>\n"
    "  </resources>\n"
    "  <build><item objectid=\"1\"/></build>"
)

# nosort: no <build> section, so the parser falls back to one part per
# object id sorted AS STRINGS — "10" sorts before "2".
NOSORT = model(
    "  <resources>\n"
    '    <object id="10" name="ten">' + cube_mesh() + "</object>\n"
    '    <object id="2" name="two">' + cube_mesh() + "</object>\n"
    "  </resources>"
)

if __name__ == "__main__":
    write_3mf("colorgroup.3mf", COLORGROUP)
    write_3mf("units.3mf", UNITS)
    write_3mf("nested.3mf", NESTED)
    write_3mf("nosort.3mf", NOSORT)
