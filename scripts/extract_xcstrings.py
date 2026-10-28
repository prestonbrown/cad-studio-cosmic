#!/usr/bin/env python3
"""Convert an Xcode String Catalog to fluent .ftl files.

Usage: python3 scripts/extract_xcstrings.py fixtures/upstream/Localizable.xcstrings app/i18n
Writes <out>/<lang>/main.ftl for every localization present. Keys are sanitized
to [a-z0-9-]; collisions get a -2 suffix. Port-added strings (no upstream key)
merge in from <out>/port-strings.json so a re-vendor keeps them. A mapping
report goes to stdout.

Format migration — the generated catalogs carry no Swift-era markers:
- %@ / %lld / %d become fluent placeables {$name}. PLACEHOLDER_NAMES names
  them per key from the upstream interpolation sites (the Swift sources the
  xcstrings was vendored from); keys it doesn't know get positional names
  arg, arg_2, … (underscores, so `fl!("k", arg_2 = v)` parses as Rust too).
- The five format-only raw keys (pure punctuation — every one sanitizes to
  "key") are renamed in RENAMES, the same name in every language.
- A value's real newlines stay real: a newline opens an indented
  continuation line. Fluent text has no backslash escape, so \\ or \\n in
  the .ftl would render as those two literal characters; braces are
  rejected loudly instead (they would open a placeable mid-text).
"""
import json
import re
import sys
from pathlib import Path

# Generated ftl key -> final name; the five raw keys whose sanitized forms
# all collided on "key" (pure-punctuation xcstrings keys from interpolated
# Swift strings). Upstream sources: motion-value-row MotionControls.swift:58,
# version-timestamps VersionMenu.swift:21, activity-accessibility-label
# ActivityViewer.swift:38 + ToolActivityRow.swift:66, speed-option
# MotionControls.swift:38/44, settings-none SettingsView.swift:137/149.
RENAMES = {
    "key": "motion-value-row",
    "key-2": "version-timestamps",
    "key-3": "activity-accessibility-label",
    "key-4": "speed-option",
    "key-5": "settings-none",
}

# Final ftl key -> placeable names, positional. The arg meanings come from
# the Swift interpolation each xcstrings entry was extracted from. Keys
# absent here (and any marker past the listed names) fall back to
# arg / arg_2 / … positionally.
PLACEHOLDER_NAMES = {
    "motion-value-row": ["name", "value"],
    "version-timestamps": ["relative", "absolute"],
    "activity-accessibility-label": ["text", "detail"],
    "speed-option": ["speed"],
    "s": ["time", "duration"],
    "ha-terminato-con-un-errore-codice-d": ["agent", "code"],
    "ha-terminato-con-un-errore": ["agent"],
    "non-trovato": ["agent"],
    "non-autenticato-apri-il-terminale-ed-esegui-per-effettuare-l-accesso": ["agent", "command"],
    "non-installato-oppure-non-stato-trovato-installalo-o-indica-il-percorso-dell-eseguibile": ["agent"],
    "scrive-lo-script-cad-genera-il-modello-3d-e-la-tavola-tecnica": ["agent"],
    "sta-lavorando": ["agent"],
    "sta-lavorando-2": ["agent", "title"],
    "sta-ragionando": ["agent"],
    "sta-scrivendo": ["agent"],
    "lld-righe": ["file", "lines"],
    "x-y-z": ["total", "dx", "dy", "dz"],
    "lld-colori": ["count"],
    "lld-immagini": ["count"],
    "lld": ["number"],
    "lld-2": ["page", "title"],
    "lld-lld": ["page", "total", "title"],
    "accesso-a-richiesto": ["agent"],
    "errore-di": ["agent"],
    "esiste-gi-una-cartella-chiamata-in-questa-posizione": ["name"],
    "file-di-output-mancanti": ["files"],
    "ha-ragionato-per-lld-s": ["seconds"],
    "il-comando-non-riuscito-controlla-il-log-per-i-dettagli": ["command"],
    "impossibile-avviare": ["agent"],
    "impossibile-ripristinare-la-versione-lld": ["version"],
    "l-animazione-ha-collisioni-tra-parti-in-lld-fotogrammi": ["frames"],
    "l-esecuzione-di-model-py-non-riesce-con-questo-errore-correggi-lo-script": ["error"],
    "mostra-2": ["name"],
    "nascondi-2": ["name"],
    "pagina-lld": ["page"],
    "parametro-non-trovato-in-model-py": ["name"],
    "predefinito-di": ["agent"],
    "scegli-dove-creare-la-cartella": ["name"],
    "separa": ["name"],
    "tieni-unita": ["name"],
    "sta-ragionando-circa-token": ["tokens"],
    "tavola-tecnica": ["page"],
    "trovato-in": ["version", "path"],
    "versione": ["version"],
    "versione-lld": ["version"],
    "versione-lld-ripristinata": ["version"],
    "versione-lld-2": ["version", "summary"],
    "animation-json-non-valido": ["error"],
    "v-lld": ["version"],
    "local-bin": ["command"],
    "lld-3": ["count"],
    "non-una-cartella": ["path"],
    "version-not-in-history": ["version"],
}

MARKER = re.compile(r"%(lld|@|d)")
# %.Nf floats (%.2f etc) — none exist in the corpus today, but a future
# re-vendor must convert rather than silently render them literally
FLOAT_MARKER = re.compile(r"%\.\d+f")


def ftl_key(raw: str, used: set[str]) -> str:
    k = re.sub(r"[^a-zA-Z0-9]+", "-", raw).strip("-").lower() or "key"
    base, n = k, 2
    while k in used:
        k = f"{base}-{n}"
        n += 1
    used.add(k)
    return RENAMES.get(k, k)


def positional(index: int) -> str:
    return "arg" if index == 0 else f"arg_{index + 1}"


def to_placeables(key: str, value: str) -> str:
    """Swap %@/%lld/%d/%.Nf for {$name} placeables, named per key."""
    names = PLACEHOLDER_NAMES.get(key, [])

    def sub(match: re.Match) -> str:
        index = sub.count  # type: ignore[attr-defined]
        sub.count += 1  # type: ignore[attr-defined]
        name = names[index] if index < len(names) else positional(index)
        return "{$" + name + "}"

    sub.count = 0  # type: ignore[attr-defined]
    # one shared counter: %.Nf occurrences consume names in the same
    # positional stream as %@/%lld/%d
    return FLOAT_MARKER.sub(sub, MARKER.sub(sub, value))


def render(key: str, raw_value: str, comments: list[str]) -> list[str]:
    """One entry as .ftl lines: optional comments, then `key = value` with
    Swift markers converted to placeables and a real newline turned into an
    indented continuation line."""
    if "{" in raw_value or "}" in raw_value:
        raise SystemExit(
            f"{key}: value carries a brace fluent would read as a placeable — "
            f"needs a manual quoted-string form: {raw_value!r}"
        )
    value = to_placeables(key, raw_value)
    lines = [f"# {comment}" for comment in comments]
    if "\n" in value:
        first, *rest = value.split("\n")
        lines.append(f"{key} = {first}")
        for continuation in rest:
            if continuation == "":
                raise SystemExit(
                    f"{key}: a blank line inside a value would end the fluent "
                    f"entry — needs a manual form: {raw_value!r}"
                )
            lines.append(f"    {continuation}")
    else:
        lines.append(f"{key} = {value}")
    return lines


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
    additions = {}
    additions_path = outp / "port-strings.json"
    if additions_path.exists():
        additions = json.loads(additions_path.read_text())
    outp.mkdir(parents=True, exist_ok=True)
    for lang, pairs in sorted(langs.items()):
        used: set[str] = set()
        d = outp / lang
        d.mkdir(parents=True, exist_ok=True)
        lines = [f"# Generated from {src} — do not edit by hand; source of record is en."]
        renames = []
        for raw, value in pairs:
            key = ftl_key(raw, used)
            if key in RENAMES.values():
                renames.append(f"{raw!r} -> {key}")
            lines.extend(render(key, value, []))
        for raw, spec in additions.items():
            key = ftl_key(raw, used)
            value = spec["localizations"].get(lang)
            if value is None:
                raise SystemExit(f"port addition {raw!r} has no {lang} value")
            comments = ["Port addition (no upstream key)."]
            if lang in spec.get("todo_review", []):
                comments.append(
                    f"TODO(review): English carry-over — needs a real {lang} translation."
                )
            lines.extend(render(key, value, comments))
        (d / "main.ftl").write_text("\n".join(lines) + "\n")
        print(f"{lang}: {len(pairs)} extracted + {len(additions)} port additions")
        for note in renames:
            print(f"  renamed {note}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
