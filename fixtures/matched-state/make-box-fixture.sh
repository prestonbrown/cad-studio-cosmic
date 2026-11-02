#!/bin/bash
# Assemble the matched-state "box-fixture" project: the content upstream's
# gallery captures (shots/upstream/project-model.png et al.) show, so port and
# upstream captures compare like for like.
# Usage: make-box-fixture.sh <dest-parent-dir>   -> <dest>/box-fixture
set -eu
here=$(cd "$(dirname "$0")" && pwd)
golden="$here/../golden"
dest="$1/box-fixture"
rm -rf "$dest"
mkdir -p "$dest/output" "$dest/.cadstudio"
cp "$golden"/* "$dest/output/"
cp "$here/project.json" "$here/chat.json" "$dest/.cadstudio/"
cp "$here/model.py" "$dest/model.py"
prompts=("Crea una scatola con coperchio, 60×60 mm, con pareti da 2 mm." \
         "Porta l'altezza della base a 8 mm." \
         "Aggiungi lo smusso sugli spigoli del coperchio.")
for n in 1 2 3; do
  v="$dest/.cadstudio/versions/$n"
  mkdir -p "$v/output"
  cp "$golden"/* "$v/output/"
  cp "$here/chat.json" "$v/chat.json"
  cp "$here/model.py" "$v/model.py"
  printf '{"number": %d, "created_at": "2026-10-2%dT08:4%d:00Z", "prompt": "%s", "session_id": null}\n' \
    "$n" "$((6 + n))" "$n" "${prompts[$((n - 1))]}" > "$v/version.json"
done
mkdir -p "$1/empty-project"
echo "$dest"
