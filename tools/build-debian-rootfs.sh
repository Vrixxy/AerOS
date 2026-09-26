#!/bin/bash
# Builds build/linux-extract/rootfs-debian.img: a Debian 13 root filesystem
# (debootstrap minbase + Xorg/fbdev + openbox + xterm + a browser) for the
# AerOS Linux window. The disk is exposed read-only through virtio-blk, so
# everything writable (/tmp, /run, /var/log, ...) is a tmpfs set up by
# /sbin/init, and the interface configs are generated at boot.
#
# Run inside WSL Debian as root:
#   wsl -d Debian -u root -- bash /mnt/c/Users/<you>/AerOS/tools/build-debian-rootfs.sh
# then copy the result over build/esp/ROOTFS.
#
# Needs: network access, ~2 GB of scratch space under /var/tmp, and the
# Debian cloud image's root partition (build/linux-extract/0.img, produced by
# tools/extract-debian-kernel.ps1) to borrow the kernel modules the initrd
# doesn't carry (evdev).
set -euo pipefail

WIN=${WIN:-/mnt/c/Users/hkvla/AerOS}
WORK=/var/tmp/aeros-rootfs
KVER=6.12.107+deb13-amd64
SIZE_MB=${SIZE_MB:-420}
BROWSER=${BROWSER:-netsurf-gtk}
BROWSER_CMD=${BROWSER_CMD:-netsurf-gtk}
OUT_NAME=linux-extract/rootfs-debian.img
# FULL=1: a bigger image that also has Firefox (launched with F in seamless
# mode). It doesn't fit the FAT boot volume, so it is written to
# build/rootfs-disk.img for attaching as a second disk (the hypervisor picks up
# any extra disk labelled aeros-root); see tools/live-linux.ps1.
if [ -n "${FULL:-}" ]; then
  WORK=/var/tmp/aeros-rootfs-full
  SIZE_MB=${FULL_SIZE_MB:-1200}
  BROWSER="netsurf-gtk firefox-esr"
  OUT_NAME=rootfs-disk.img
fi

export DEBIAN_FRONTEND=noninteractive
if ! command -v debootstrap >/dev/null; then
  apt-get update -qq
  apt-get install -y -qq debootstrap
fi

# RESUME=1 skips the download/install phase and redoes only the configuration
# and image steps on an existing $WORK.
if [ -z "${RESUME:-}" ]; then
umount -R "$WORK/proc" "$WORK/sys" "$WORK/dev" 2>/dev/null || true
rm -rf "$WORK"
mkdir -p "$WORK"

BASE=busybox-static,kmod,xserver-xorg-core,xserver-xorg-video-fbdev,xserver-xorg-input-libinput,xinit,x11-xserver-utils,x11-xkb-utils,xkb-data,xauth,fontconfig,xfonts-base,fonts-dejavu-core,openbox,xterm,dbus-x11
debootstrap --variant=minbase --arch=amd64 --include="$BASE" trixie "$WORK" http://deb.debian.org/debian

mount -t proc proc "$WORK/proc"
mount --bind /dev "$WORK/dev"
chroot "$WORK" apt-get install -y --no-install-recommends $BROWSER

# ---- kernel modules the initrd lacks (evdev), plus what depmod needs ------
CLOUD=/mnt/aeros-cloud
mkdir -p "$CLOUD"
mount -o loop,ro "$WIN/build/linux-extract/0.img" "$CLOUD"
SRC=$(ls -d "$CLOUD"/usr/lib/modules/$KVER "$CLOUD"/lib/modules/$KVER 2>/dev/null | head -n 1)
DST="$WORK/usr/lib/modules/$KVER"
mkdir -p "$DST/kernel/drivers/input/mouse"
cp "$SRC/kernel/drivers/input/evdev.ko.xz" "$DST/kernel/drivers/input/"
cp "$SRC/kernel/drivers/input/mouse/psmouse.ko.xz" "$DST/kernel/drivers/input/mouse/"
cp "$SRC"/modules.builtin "$SRC"/modules.builtin.modinfo "$SRC"/modules.order "$DST/" 2>/dev/null || true
umount "$CLOUD"
chroot "$WORK" depmod -a "$KVER"
fi

# ---- extras added after the first full build (safe to re-run) ---------------
# w3m fetches and renders web pages for AerOS's browser (real TLS through
# libssl + the CA bundle); the overlay kernel module lets the store's apt
# installs write into a RAM layer over the read-only root.
EXTRAS="w3m ca-certificates gpgv sqv debian-archive-keyring"
NEED=""
for pkg in $EXTRAS; do chroot "$WORK" dpkg -s $pkg >/dev/null 2>&1 || NEED="$NEED $pkg"; done
if [ -n "$NEED" ]; then
  mountpoint -q "$WORK/proc" || mount -t proc proc "$WORK/proc"
  mountpoint -q "$WORK/dev" || mount --bind /dev "$WORK/dev"
  mkdir -p "$WORK/run"
  cp /etc/resolv.conf "$WORK/run/resolv.conf"
  chroot "$WORK" apt-get update -qq
  chroot "$WORK" apt-get install -y -qq --no-install-recommends $NEED
  chroot "$WORK" apt-get clean
  rm -rf "$WORK"/var/lib/apt/lists/* "$WORK/run/resolv.conf"
  umount "$WORK/dev" "$WORK/proc" 2>/dev/null || true
fi
if [ ! -e "$WORK/usr/lib/modules/$KVER/kernel/fs/overlayfs/overlay.ko.xz" ] \
   || [ ! -e "$WORK/usr/lib/modules/$KVER/kernel/drivers/block/loop.ko.xz" ]; then
  CLOUD=/mnt/aeros-cloud
  mkdir -p "$CLOUD"
  mount -o loop,ro "$WIN/build/linux-extract/0.img" "$CLOUD"
  SRC=$(ls -d "$CLOUD"/usr/lib/modules/$KVER "$CLOUD"/lib/modules/$KVER 2>/dev/null | head -n 1)
  mkdir -p "$WORK/usr/lib/modules/$KVER/kernel/fs/overlayfs"
  cp "$SRC/kernel/fs/overlayfs/overlay.ko.xz" "$WORK/usr/lib/modules/$KVER/kernel/fs/overlayfs/"
  mkdir -p "$WORK/usr/lib/modules/$KVER/kernel/drivers/block"
  cp "$SRC/kernel/drivers/block/loop.ko.xz" "$WORK/usr/lib/modules/$KVER/kernel/drivers/block/"
  umount "$CLOUD"
  chroot "$WORK" depmod -a "$KVER"
fi

# ---- static configuration -------------------------------------------------
mkdir -p "$WORK/etc/aeros" "$WORK/usr/share/udhcpc"
ln -sf /run/resolv.conf "$WORK/etc/resolv.conf"
echo aeros > "$WORK/etc/hostname"

cat > "$WORK/usr/share/udhcpc/default.script" <<'DHCP'
#!/bin/sh
case "$1" in
  bound|renew)
    /bin/busybox ifconfig $interface $ip netmask ${subnet:-255.255.255.0} up
    [ -n "$router" ] && /bin/busybox route add default gw ${router%% *} dev $interface
    : > /run/resolv.conf
    for d in $dns; do echo "nameserver $d" >> /run/resolv.conf; done ;;
esac
exit 0
DHCP
chmod 755 "$WORK/usr/share/udhcpc/default.script"

# The seamless-window agent (guest half of the host's per-app windows): built
# here against the host's libX11 headers, run against the guest's libX11.so.6
# (same Debian release).
dpkg -s libx11-dev libxfixes-dev >/dev/null 2>&1 || apt-get install -y -qq libx11-dev libxfixes-dev
mkdir -p "$WORK/usr/local/bin"
cat > "$WORK/usr/local/bin/aeros-firefox" <<'FFOX'
#!/bin/sh
# Firefox tuned for this environment: no GPU, no sandbox (user namespaces are
# not available), one content process, a throwaway profile in RAM.
export GDK_BACKEND=x11 NO_AT_BRIDGE=1 MOZ_DBUS_REMOTE=0 MOZ_DISABLE_CONTENT_SANDBOX=1 MOZ_DISABLE_GMP_SANDBOX=1 MOZ_DISABLE_RDD_SANDBOX=1 MOZ_DISABLE_SOCKET_PROCESS_SANDBOX=1 MOZ_DISABLE_GPU_SANDBOX=1
P=/tmp/home/ffprofile
mkdir -p $P
cat > $P/user.js <<'PREFS'
user_pref("layers.acceleration.disabled", true);
user_pref("gfx.webrender.software", true);
user_pref("security.sandbox.content.level", 0);
user_pref("dom.ipc.processCount", 1);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.startup.homepage", "about:blank");
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("app.update.enabled", false);
user_pref("media.hardware-video-decoding.enabled", false);
// The guest network is IPv4-only (QEMU user-mode NAT): don't try AAAA
// addresses first, and skip DNS-over-HTTPS so the guest's resolver is used.
user_pref("network.dns.disableIPv6", true);
user_pref("network.trr.mode", 5);
user_pref("network.proxy.type", 0);
// Skip the first-run onboarding pages.
user_pref("browser.aboutwelcome.enabled", false);
user_pref("trailhead.firstrun.didSeeAboutWelcome", true);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("startup.homepage_welcome_url", "");
user_pref("startup.homepage_welcome_url.additional", "");
user_pref("browser.newtabpage.enabled", false);
// Performance on a software-rendered, emulated machine: no animations or
// smooth scrolling, a small frame rate, no background network chatter.
user_pref("general.smoothScroll", false);
user_pref("toolkit.cosmeticAnimations.enabled", false);
user_pref("layout.frame_rate", 30);
user_pref("ui.prefersReducedMotion", 1);
user_pref("browser.sessionstore.interval", 600000);
user_pref("browser.cache.disk.enable", false);
user_pref("browser.cache.memory.capacity", 65536);
user_pref("browser.safebrowsing.malware.enabled", false);
user_pref("browser.safebrowsing.phishing.enabled", false);
user_pref("network.captive-portal-service.enabled", false);
user_pref("network.connectivity-service.enabled", false);
user_pref("extensions.pocket.enabled", false);
user_pref("browser.discovery.enabled", false);
user_pref("toolkit.telemetry.enabled", false);
user_pref("browser.tabs.animate", false);
user_pref("image.mem.decode_bytes_at_a_time", 65536);
user_pref("dom.ipc.keepProcessesAlive.web", 1);
user_pref("gfx.canvas.accelerated", false);
PREFS
# Read the big libraries sequentially first (1 MiB readahead) instead of
# letting Firefox demand-page them in thousands of small reads.
for f in /usr/lib/firefox-esr/libxul.so /usr/lib/firefox-esr/firefox-esr; do
  [ -f $f ] && cat $f >/dev/null 2>&1
done
exec firefox-esr --no-remote --profile $P "$@"
FFOX
chmod 755 "$WORK/usr/local/bin/aeros-firefox"
cat > "$WORK/usr/local/bin/aeros-install" <<'INSTALL'
#!/bin/sh
# Installs Debian packages for the AerOS store (into the RAM overlay).
# The agent reads the last line of the log: DONE / FAILED / anything = running.
LOG=/tmp/aeros-install.log
echo "starting $*" > $LOG
export DEBIAN_FRONTEND=noninteractive
# Run apt as root (no _apt sandbox user in this tiny system) and verify
# signatures with gpgv.
# (The guest clock can be a few days off the mirror's: skip the date checks.)
APT="-o APT::Sandbox::User=root -o APT::Key::GPGVCommand=/usr/bin/gpgv -o Acquire::Check-Date=false -o Acquire::Check-Valid-Until=false"
if [ ! -f /var/lib/apt/aeros-updated ]; then
  echo "updating package lists" >> $LOG
  apt-get $APT update -o Acquire::Languages=none >> $LOG 2>&1 || { echo FAILED >> $LOG; exit 1; }
  : > /var/lib/apt/aeros-updated
fi
case "$1" in
flathub:*)
  # A Flathub app: needs flatpak itself first (installed on demand).
  ID=${1#flathub:}
  if ! command -v flatpak >/dev/null 2>&1; then
    echo "setting up flatpak" >> $LOG
    apt-get $APT install -y --no-install-recommends flatpak >> $LOG 2>&1 || { echo FAILED >> $LOG; exit 1; }
  fi
  echo "adding Flathub" >> $LOG
  flatpak remote-add --system --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo >> $LOG 2>&1 || { echo FAILED >> $LOG; exit 1; }
  echo "installing $ID" >> $LOG
  if flatpak install --system -y --noninteractive flathub "$ID" >> $LOG 2>&1; then echo DONE >> $LOG; else echo FAILED >> $LOG; fi
  ;;
*)
  echo "installing $*" >> $LOG
  if apt-get $APT install -y --no-install-recommends "$@" >> $LOG 2>&1; then echo DONE >> $LOG; else echo FAILED >> $LOG; fi
  ;;
esac
INSTALL
chmod 755 "$WORK/usr/local/bin/aeros-install"
gcc -O2 -Wall -o "$WORK/usr/local/bin/aeros-agent" "$WIN/tools/aeros-agent.c" -lX11 -lXfixes
gcc -O2 -Wall -o "$WORK/usr/local/bin/aeros-mmap-test" "$WIN/tools/mmap-test.c"

# Openbox without window decorations: in seamless mode AerOS draws each
# window's frame itself, and the agent tiles the windows.
sed '0,/<\/applications>/s##<application class="*"><decor>no</decor></application></applications>#' \
  "$WORK/etc/xdg/openbox/rc.xml" > "$WORK/etc/aeros/openbox-rc.xml"

# xterm: selecting text also fills the CLIPBOARD (so it reaches the AerOS
# clipboard), and Ctrl+V / Ctrl+Shift+V paste it (Ctrl+Shift+C copies).
cat > "$WORK/etc/aeros/Xresources" <<'XRES'
XTerm*selectToClipboard: true
XTerm*VT100.translations: #override \n\
  Ctrl Shift <Key>C: copy-selection(CLIPBOARD) \n\
  Ctrl Shift <Key>V: insert-selection(CLIPBOARD) \n\
  Ctrl <Key>V: insert-selection(CLIPBOARD)
XRES

cat > "$WORK/etc/aeros/xinitrc" <<XINIT
#!/bin/sh
xrdb -merge /etc/aeros/Xresources
xsetroot -solid '#1c2b36'
openbox --config-file /etc/aeros/openbox-rc.xml &
aeros-agent &
xterm -geometry 58x20+0+0 -fa 'DejaVu Sans Mono' -fs 10 -bg '#0d1117' -fg '#d7dee6' &
$BROWSER_CMD &
wait
XINIT
chmod 755 "$WORK/etc/aeros/xinitrc"

cat > "$WORK/sbin/init" <<'INIT'
#!/bin/busybox sh
BB=/bin/busybox
$BB mount -t proc proc /proc
$BB mount -t sysfs sysfs /sys
$BB mount -t devtmpfs devtmpfs /dev 2>/dev/null
$BB mkdir -p /dev/pts /dev/shm
$BB mount -t devpts devpts /dev/pts
$BB mount -t tmpfs tmpfs /dev/shm
# The root disk is read-only: every directory something writes to is a tmpfs.
for d in /tmp /run /var/log /var/tmp; do
  $BB mkdir -p $d 2>/dev/null
  $BB mount -t tmpfs tmpfs $d
done
export PATH=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin HOME=/tmp/home TERM=linux
export XDG_RUNTIME_DIR=/tmp/xdg XDG_CACHE_HOME=/tmp/cache DISPLAY=:0 LANG=C
mkdir -p $HOME $XDG_RUNTIME_DIR /tmp/.X11-unix
chmod 700 $XDG_RUNTIME_DIR; chmod 1777 /tmp/.X11-unix
echo "AEROS_LINUX_INIT_OK pid=$$"
echo "AEROS_LINUX_UNAME $(uname -a)"
echo "AEROS_LINUX_CPUS $($BB grep -c '^processor' /proc/cpuinfo)"
/usr/local/bin/aeros-mmap-test
# A RAM layer over the read-only root for the store's installs: apt writes
# into overlays on the directories it touches (gone after a reboot).
# The writable data area behind the root on the same disk (a loop device at
# an offset): the layers and Flatpak live there and so persist across boots.
DATA=""
if [ -f /etc/aeros-data-offset ] && modprobe loop 2>/dev/null; then
  read DATA_OFF < /etc/aeros-data-offset
  $BB mkdir -p /run/data
  if $BB losetup -o $DATA_OFF /dev/loop0 /dev/vda 2>/dev/null; then
    /sbin/e2fsck -p /dev/loop0 >/dev/null 2>&1
    if $BB mount -t ext4 /dev/loop0 /run/data 2>/dev/null; then
      DATA=1
      echo "AEROS_LINUX_DATA_OK $($BB df -k /run/data | $BB tail -n 1 | $BB tr -s ' ')"
      # Push dirty data to the disk regularly (there is no clean shutdown).
      ( while $BB sleep 10; do $BB sync; done ) &
    fi
  fi
fi
if modprobe overlay 2>/dev/null; then
  # (The root is read-only, so the layers live under /run.)
  $BB mkdir -p /run/ov
  if [ -n "$DATA" ]; then
    $BB mkdir -p /run/data/ov
    $BB mount --bind /run/data/ov /run/ov
  else
    $BB mount -t tmpfs -o size=420m tmpfs /run/ov
  fi
  for d in /usr /etc /var/lib /var/cache /opt; do
    [ -d $d ] || continue
    n=$(echo $d | $BB tr '/' '_')
    $BB mkdir -p /run/ov/$n /run/ov/$n.w
    $BB mount -t overlay overlay -o lowerdir=$d,upperdir=/run/ov/$n,workdir=/run/ov/$n.w $d 2>/dev/null
  done
  echo "AEROS_LINUX_OVERLAY_OK $($BB mount | $BB grep -c '^overlay')"
fi
# Directories X needs writable inside /var/lib (mounted after the overlay,
# which would otherwise hide them).
for d in /var/lib/xkb /var/lib/dbus; do
  $BB mkdir -p $d 2>/dev/null
  $BB mount -t tmpfs tmpfs $d
done
# Flatpak's store (Flathub runtimes are big): its own roomy RAM disk.
$BB mkdir -p /var/lib/flatpak 2>/dev/null
if [ -n "$DATA" ]; then
  $BB mkdir -p /run/data/flatpak
  $BB mount --bind /run/data/flatpak /var/lib/flatpak
else
  $BB mount -t tmpfs -o size=1200m tmpfs /var/lib/flatpak
fi
# Big readahead on the virtio disk: demand-paging a large program (Firefox)
# otherwise turns into thousands of small requests through the emulated disk.
echo 1024 > /sys/block/vda/queue/read_ahead_kb 2>/dev/null
# SMP self-test of NMI IPIs: ask every CPU for a backtrace (the guest sends
# an NMI to the others; each prints "NMI backtrace for cpu N").
if [ "$($BB grep -c '^processor' /proc/cpuinfo)" -gt 1 ]; then
  ( $BB sleep 12; echo l > /proc/sysrq-trigger ) &
fi

# Networking: virtio-net, DHCP from the host's user-mode network.
$BB ifconfig lo up
NIC=$(ls /sys/class/net | $BB grep -v '^lo$' | $BB head -n 1)
if [ -n "$NIC" ] && $BB ifconfig $NIC up; then
  ( $BB udhcpc -i $NIC -n -q -t 10 -T 3 -s /usr/share/udhcpc/default.script >/dev/null 2>&1 \
      && echo "AEROS_LINUX_NET_OK $NIC $($BB ifconfig $NIC | $BB grep 'inet addr' | $BB tr -s ' ')" \
      && $BB ping -c 2 -W 3 10.0.2.2 >/dev/null 2>&1 && echo "AEROS_LINUX_PING_OK gateway=10.0.2.2" ) &
fi

# Input devices: the modules Debian's initrd doesn't carry, then find the
# evdev node for each device by name (numbering depends on probe order).
modprobe evdev 2>/dev/null
modprobe psmouse 2>/dev/null
KBD=; MOUSE=
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
  for e in /sys/class/input/event*; do
    [ -e "$e" ] || continue
    n=$(cat $e/device/name 2>/dev/null)
    case "$n" in
      *[Kk]eyboard*) KBD=/dev/input/$(basename $e) ;;
      *[Mm]ouse*) MOUSE=/dev/input/$(basename $e) ;;
    esac
  done
  [ -n "$KBD" ] && [ -n "$MOUSE" ] && break
  $BB sleep 1
done
echo "AEROS_LINUX_INPUT kbd=$KBD mouse=$MOUSE"
# libinput refuses devices udev hasn't "initialized" - i.e. that have no entry
# in the udev database. There is no udevd here, so write the two entries it
# looks for (an "I:" line marks the device initialized, "E:" are properties).
mkdir -p /run/udev/data
udev_entry() {
  [ -n "$1" ] || return
  devnum=$(cat /sys/class/input/$(basename $1)/dev)
  { echo "I:1"; echo "E:ID_INPUT=1"; echo "E:$2=1"; } > /run/udev/data/c$devnum
}
udev_entry "$KBD" ID_INPUT_KEYBOARD
udev_entry "$MOUSE" ID_INPUT_MOUSE
cat > /tmp/xorg.conf <<XCONF
Section "ServerFlags"
  Option "AutoAddDevices" "false"
  Option "DontVTSwitch" "true"
EndSection
Section "InputDevice"
  Identifier "Keyboard0"
  Driver "libinput"
  Option "Device" "$KBD"
EndSection
Section "InputDevice"
  Identifier "Mouse0"
  Driver "libinput"
  Option "Device" "$MOUSE"
  Option "AccelProfile" "flat"
  Option "AccelSpeed" "0"
EndSection
Section "Device"
  Identifier "Framebuffer"
  Driver "fbdev"
  Option "fbdev" "/dev/fb0"
EndSection
Section "Screen"
  Identifier "Screen0"
  Device "Framebuffer"
EndSection
Section "ServerLayout"
  Identifier "Layout0"
  Screen "Screen0"
  InputDevice "Keyboard0" "CoreKeyboard"
  InputDevice "Mouse0" "CorePointer"
EndSection
XCONF

# Default window size for NetSurf so several apps fit on the small guest screen.
mkdir -p $HOME/.netsurf $HOME/.config/netsurf
printf 'window_width:480\nwindow_height:430\nwindow_x:0\nwindow_y:0\n' > $HOME/.netsurf/Choices
cp $HOME/.netsurf/Choices $HOME/.config/netsurf/Choices

# The graphical session goes on the framebuffer console (VT 1).
setsid -c xinit /etc/aeros/xinitrc -- /usr/bin/Xorg :0 vt1 -config /tmp/xorg.conf \
  -logfile /tmp/Xorg.log -nolisten tcp -noreset -novtswitch -keeptty \
  </dev/tty1 >/tmp/xinit.log 2>&1 &
( for i in $(seq 1 90); do
    [ -S /tmp/.X11-unix/X0 ] && { echo "AEROS_LINUX_X_OK"; break; }
    $BB sleep 1
  done
  # Each session program is waited for (up to 40 s) instead of sampled once:
  # on a busy or slow start they come up seconds apart.
  for app in openbox xterm netsurf-gtk aeros-agent; do
    for i in $(seq 1 40); do
      $BB pidof $app >/dev/null 2>&1 && { echo "AEROS_LINUX_APP_OK $app"; break; }
      $BB sleep 1
    done
  done
  echo "AEROS_LINUX_XINPUT_LOG:"
  $BB grep -a -i -E 'Mouse0|Keyboard0|libinput|/dev/input|event[0-9]' /tmp/Xorg.log | $BB tail -n 14 | $BB cut -c1-200 ) &

# Warm the page cache with the launcher apps' binaries and libraries once the
# session is up, so starting them (or several at once) from the host doesn't
# wait on the emulated disk.
( $BB sleep 25
  for p in /usr/bin/xterm /usr/bin/netsurf-gtk; do
    [ -x $p ] || continue
    $BB cat $p >/dev/null 2>&1
    for l in $(ldd $p 2>/dev/null | $BB awk '/=>/ {print $3}'); do
      $BB cat $l >/dev/null 2>&1
    done
  done
  echo "AEROS_LINUX_PREFETCH_OK"
  true ) &

# Status heartbeat on the serial console: which session processes are alive
# and the newest X errors (debugging aid; the desktop has no serial shell).
( while :; do
    $BB sleep 20
    echo "AEROS_LINUX_STATUS pids: $($BB pidof Xorg openbox xterm netsurf-gtk aeros-agent firefox-esr firefox-bin | $BB tr '\n' ' ') mem: $($BB grep -E 'MemAvailable' /proc/meminfo | $BB tr -s ' ')"
    $BB tail -n 4 /tmp/aeros-spawn.log 2>/dev/null | $BB cut -c1-160
    $BB tail -n 3 /tmp/aeros-install.log 2>/dev/null | $BB cut -c1-160
    $BB grep -a '(EE)' /tmp/Xorg.log 2>/dev/null | $BB tail -n 2
    $BB tail -n 2 /tmp/xinit.log 2>/dev/null
    $BB grep -E 'i8042' /proc/interrupts 2>/dev/null | $BB tr -s ' '
  done ) &

echo "Welcome to Linux inside AerOS"
# A root shell stays on the serial console for debugging.
exec /bin/sh </dev/console >/dev/console 2>&1
INIT
chmod 755 "$WORK/sbin/init"

# ---- slim down -------------------------------------------------------------
# Drop Mesa's software-GL stack (LLVM, gallium, z3 - hundreds of MB pulled in
# by Xorg's GL dependencies) and Ghostscript: X runs on plain fbdev and
# nothing here uses OpenGL. Then make sure the programs we start still
# resolve every library.
LIBDIR="$WORK/usr/lib/x86_64-linux-gnu"
rm -f "$LIBDIR"/libLLVM* "$LIBDIR"/libgallium* "$LIBDIR"/libz3* "$LIBDIR"/libgs.so*
rm -rf "$LIBDIR/dri"
for b in usr/lib/xorg/Xorg usr/bin/xterm usr/bin/openbox usr/bin/netsurf-gtk usr/bin/xinit $( [ -n "${FULL:-}" ] && echo usr/lib/firefox-esr/firefox-esr ); do
  chroot "$WORK" ldd "/$b" 2>&1 | grep 'not found' | sed "s|^|MISSING for $b: |" || true
done

chroot "$WORK" fc-cache -f
chroot "$WORK" apt-get clean
rm -rf "$WORK"/var/lib/apt/lists/* "$WORK"/usr/share/doc/* "$WORK"/usr/share/man/* \
       "$WORK"/usr/share/info/* "$WORK"/var/cache/apt/* "$WORK"/var/log/*
find "$WORK"/usr/share/locale -mindepth 1 -maxdepth 1 ! -name 'en*' -exec rm -rf {} + 2>/dev/null || true

umount "$WORK/dev" "$WORK/proc" 2>/dev/null || true
# Mount points the read-only root needs, and the console nodes the kernel opens
# for PID 1 before devtmpfs exists.
mkdir -p "$WORK"/{tmp,run,var/log,var/tmp,var/lib/xkb,var/lib/dbus,dev/pts,dev/shm,proc,sys}
for node in "console c 5 1 600" "null c 1 3 666" "tty1 c 4 1 620"; do
  set -- $node
  [ -e "$WORK/dev/$1" ] || mknod -m "$5" "$WORK/dev/$1" $2 $3 $4
done
du -sm "$WORK"

OUT="$WIN/build/$OUT_NAME"
IMG="${WORK}.img"
rm -f "$IMG"
truncate -s "${SIZE_MB}M" "$IMG"
echo $((SIZE_MB * 1048576)) > "$WORK/etc/aeros-data-offset"
mkfs.ext4 -q -F -m 0 -O ^has_journal -L aeros-root -d "$WORK" "$IMG"
if [ -n "${FULL:-}" ]; then
  # A writable data area after the read-only root (ext4, mounted by the guest
  # through a loop device): the store's installs live there and persist.
  DATA_MB=${DATA_MB:-15360}
  DATA="${WORK}.data"
  rm -f "$DATA"
  truncate -s "${DATA_MB}M" "$DATA"
  mkfs.ext4 -q -F -m 0 -O ^has_journal -E lazy_itable_init=1 -L aeros-data "$DATA"
  dd if="$DATA" of="$IMG" bs=1M seek="$SIZE_MB" conv=notrunc,sparse status=none
  rm -f "$DATA"
fi
cp --sparse=always -f "$IMG" "$OUT"
ls -la "$OUT"
