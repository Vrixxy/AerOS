#!/bin/sh
# Tidies the guest's application list (what AerOS shows in its launcher and
# search): hides the window manager, the X terminals and interpreters, and
# shows Firefox as "Firefox", started through aeros-firefox.
# Usage: guest-app-list.sh <root directory of the guest image>
set -e
ROOT=${1:?root directory}
APPS="$ROOT/usr/share/applications"
[ -d "$APPS" ] || exit 0
for name in openbox debian-uxterm debian-xterm netsurf-gtk python3.13; do
  file="$APPS/$name.desktop"
  [ -f "$file" ] || continue
  grep -q '^NoDisplay=true' "$file" || sed -i '0,/^\[Desktop Entry\]/s//[Desktop Entry]\nNoDisplay=true/' "$file"
done
for file in "$APPS"/python3*.desktop; do
  [ -f "$file" ] || continue
  grep -q '^NoDisplay=true' "$file" || sed -i '0,/^\[Desktop Entry\]/s//[Desktop Entry]\nNoDisplay=true/' "$file"
done
if [ -f "$APPS/firefox-esr.desktop" ]; then
  sed -i -e '0,/^Name=.*/s//Name=Firefox/' -e 's#^Exec=.*firefox-esr#Exec=aeros-firefox#' "$APPS/firefox-esr.desktop"
fi
