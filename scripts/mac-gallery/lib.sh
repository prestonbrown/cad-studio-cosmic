# Sourced by the gallery scripts. Launches the upstream app with per-launch
# settings passed as argument-domain defaults (-key value), which override
# the user's stored defaults for that run without writing them.
APP=${APP:-$HOME/code/cad-studio-upstream/build/DerivedData/Build/Products/Release/CAD Studio.app}
BUNDLE=ch.filippofinke.CADStudio
D=${MACDRIVE:-/tmp/cadshots/macdrive}
[ -x "$D" ] || swiftc -O "$(dirname "${BASH_SOURCE[0]}")/macdrive.swift" -o "$D" || exit 1
OUT=${OUT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/shots/upstream}
FX=${FX:-/tmp/cadshots/projects}
ts() { date +%H:%M:%S; }
quit_app() {
  pkill -x "CAD Studio" 2>/dev/null
  for _ in $(seq 20); do pgrep -x "CAD Studio" >/dev/null || return 0; sleep 0.2; done
  pkill -9 -x "CAD Studio"
}
# launch <light|dark> [-key value ...]   env vars set by the caller pass through
launch() {
  local mode=$1; shift
  quit_app
  local look=(-NSRequiresAquaSystemAppearance YES)
  [ "$mode" = dark ] && look=(-AppleInterfaceStyle Dark)
  "$APP/Contents/MacOS/CAD Studio" "${look[@]}" -ApplePersistenceIgnoreState YES "$@" >/dev/null 2>&1 &
  $D wait '.' 15 >/dev/null && $D front
}
recents() { printf '(%s)' "$(printf '"%s",' "$@" | sed 's/,$//')"; }
shot() { "$D" "$@"; echo "$(ts) ${2##*/} $(sips -g pixelWidth -g pixelHeight "$2" | awk '/pixel/{printf "%s ", $2}')"; }
