#!/usr/bin/env bash
set -euo pipefail

app=io.github.novakr12.SoundscapeDesktop
data="${XDG_DATA_HOME:-$HOME/.local/share}"
icon_dir="$data/icons/soundscape"
launcher="$data/applications/$app.desktop"

usage() {
  echo "Usage: $(basename "$0") <color>|reset"
  echo "Colors: green teal blue navy purple pink red orange yellow, or any #rrggbb"
  exit 1
}

refresh() {
  if command -v update-desktop-database >/dev/null; then
    update-desktop-database "$data/applications" 2>/dev/null || true
  fi
}

[ $# -eq 1 ] || usage

if [ "${1,,}" = reset ] || [ "${1,,}" = green ]; then
  rm -f "$launcher"
  rm -rf "$icon_dir"
  refresh
  echo "Soundscape's launcher icon is green again."
  exit 0
fi

case "${1,,}" in
  teal) color=14b8a6 ;;
  blue) color=3b82f6 ;;
  navy) color=1e3a8a ;;
  purple) color=8b5cf6 ;;
  pink) color=ec4899 ;;
  red) color=ef4444 ;;
  orange) color=f97316 ;;
  yellow) color=eab308 ;;
  *)
    color="${1#\#}"
    [[ "$color" =~ ^[0-9a-fA-F]{6}$ ]] || usage
    ;;
esac

find_export() {
  for root in "$data/flatpak/exports/share" /var/lib/flatpak/exports/share; do
    if [ -e "$root/$1" ]; then
      echo "$root/$1"
      return
    fi
  done
  echo "Soundscape Desktop isn't installed with Flatpak here." >&2
  exit 1
}

svg=$(find_export "icons/hicolor/scalable/apps/$app.svg")
desktop=$(find_export "applications/$app.desktop")

shade() {
  local out="" at value
  for at in 0 2 4; do
    value=$((16#${color:at:2}))
    if [ "$1" = darker ]; then
      value=$(((value * 8 + 5) / 10))
    else
      value=$(((value * 7 + 255 * 3 + 5) / 10))
    fi
    out+=$(printf '%02x' "$value")
  done
  echo "#$out"
}

mkdir -p "$icon_dir" "$data/applications"
icon="$icon_dir/soundscape-${color,,}.svg"
rm -f "$icon_dir"/soundscape-*.svg
sed -e "s/#05B851/$(shade darker)/gI" -e "s/#00FF66/$(shade lighter)/gI" "$svg" >"$icon"
sed "s|^Icon=.*|Icon=$icon|" "$desktop" >"$launcher"
refresh
echo "Soundscape's launcher icon is now #${color,,}."
echo "A pinned dock icon may only change after you log out and back in."
echo "Run '$(basename "$0") reset' to go back to green."
