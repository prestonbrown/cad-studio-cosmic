#!/usr/bin/env python3
"""Convert an Xcode String Catalog to fluent .ftl files.

Usage: python3 scripts/extract_xcstrings.py fixtures/upstream/Localizable.xcstrings app/i18n
Writes <out>/<lang>/main.ftl for every localization present. Keys are sanitized to
[a-z0-9-]; collisions get a -2 suffix. A mapping report goes to stdout.
"""
import json
import re
import sys
from pathlib import Path


def ftl_key(raw: str, used: set[str]) -> str:
    k = re.sub(r"[^a-zA-Z0-9]+", "-", raw).strip("-").lower() or "key"
    base, n = k, 2
    while k in used:
        k = f"{base}-{n}"
        n += 1
    used.add(k)
    return k


def esc(v: str) -> str:
    return v.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def main(src: str, out: str) -> None:
    data = json.loads(Path(src).read_text())
    strings = data["strings"]
    langs: dict[str, list[tuple[str, str]]] = {}
    for raw, entry in sorted(strings.items()):
        for lang, loc in entry.get("localizations", {}).items():
            unit = loc.get("stringUnit") or {}
            value = unit.get("value")
            if value is None:
                continue
            langs.setdefault(lang, []).append((raw, value))
    outp = Path(out)
    for lang, pairs in sorted(langs.items()):
        used: set[str] = set()
        d = outp / lang
        d.mkdir(parents=True, exist_ok=True)
        lines = [f"# Generated from {src} — do not edit by hand; source of record is en."]
        for raw, value in pairs:
            lines.append(f"{ftl_key(raw, used)} = {esc(value)}")
        (d / "main.ftl").write_text("\n".join(lines) + "\n")
        print(f"{lang}: {len(pairs)} strings")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
