#!/bin/sh
# Compiles WPE WebKit and the Cog browser shell from Debian 13's source
# packages, so the binaries match the libraries of AerOS's Debian userland.
# Run inside WSL Debian 13 as root:
#   wsl -d Debian -u root -- sh /mnt/c/Users/hkvla/AerOS/tools/build-webkit.sh
# The .deb packages land in build/webkit. JOBS (default 4) bounds the
# parallel compiles; each WebKit unit can take 2 GiB. SWAP=0 skips the
# swap file (containers, such as tools/webkit-ci, cannot add one).
# COG_ONLY=1 rebuilds only Cog against the WebKit packages already in $WORK.
set -eu

OUT=${OUT:-/mnt/c/Users/hkvla/AerOS/build/webkit}
WORK=${WORK:-/root/webkit-build}
JOBS=${JOBS:-4}
SWAP=${SWAP:-8G}
PATCHES=${PATCHES:-$(cd "$(dirname "$0")" && pwd)/webkit-patches}

for sources in /etc/apt/sources.list.d/*.sources; do
    sed -i 's/^Types: deb$/Types: deb deb-src/' "$sources"
done

if [ "$SWAP" != 0 ]; then
    if [ ! -f /swap-webkit ]; then
        fallocate -l "$SWAP" /swap-webkit
        chmod 600 /swap-webkit
        mkswap /swap-webkit >/dev/null
    fi
    swapon --show=NAME --noheadings | grep -q /swap-webkit || swapon /swap-webkit
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q build-essential devscripts dpkg-dev fakeroot
mkdir -p "$WORK" "$OUT"
cd "$WORK"
if [ -z "${COG_ONLY:-}" ]; then
apt-get build-dep -y -q wpewebkit
[ -d wpewebkit-*/ ] || apt-get source -q wpewebkit
cd wpewebkit-*/
DEB_BUILD_OPTIONS="parallel=$JOBS nocheck nodoc noddebs" dpkg-buildpackage -b -uc -us
cd "$WORK"
cp -f ./*.deb "$OUT"/
fi

dpkg -i ./libwpewebkit-2.0-1_*.deb ./libwpewebkit-2.0-dev_*.deb || apt-get install -f -y -q
apt-get build-dep -y -q cog
[ -d cog-*/ ] || apt-get source -q cog
cd cog-*/
# Debian builds the drm, headless and wayland platforms; the AerOS guest runs
# X11, so add Cog's x11 platform, patched to show shared-memory frames (the
# guest has no GPU, so WebKit never exports EGL images there).
apt-get install -y -q libxcb1-dev libx11-xcb-dev libxkbcommon-x11-dev
grep -q platforms= debian/rules ||
    printf '\noverride_dh_auto_configure:\n\tdh_auto_configure -- -Dplatforms=drm,headless,wayland,x11\n' >> debian/rules
for p in "$PATCHES"/cog-*.patch; do
    [ -f "$p" ] || continue
    patch -p1 -R --dry-run -s < "$p" >/dev/null 2>&1 || patch -p1 < "$p"
done
DEB_BUILD_OPTIONS="parallel=$JOBS nocheck nodoc noddebs" dpkg-buildpackage -b -uc -us
cd "$WORK"
cp -f ./cog*.deb ./libcogcore*.deb "$OUT"/ 2>/dev/null || cp -f ./*.deb "$OUT"/

ls -la "$OUT"
echo AEROS_WEBKIT_BUILD done
