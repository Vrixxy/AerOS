# Builds build/linux-extract/rootfs-new.img: a 64 MiB ext4 root with static busybox and
# a /sbin/init that mounts /proc,/sys,/dev,/tmp and execs a shell on the console.
# Run inside WSL Debian as root (needs mknod + mkfs.ext4 -d):
#   wsl -d Debian -u root -- bash /mnt/c/Users/<you>/AerOS/tools/build-rootfs.sh
# then copy the result to build/esp/ROOTFS.
set -e
rm -rf /tmp/bb && mkdir -p /tmp/bb && cd /tmp/bb
apt-get download busybox-static >/dev/null 2>&1
dpkg-deb -x busybox-static*.deb out
R=/tmp/bb/root
rm -rf $R; mkdir -p $R/bin $R/sbin $R/proc $R/sys $R/dev $R/tmp $R/etc $R/root $R/usr/bin $R/usr/sbin $R/mnt
cp out/usr/bin/busybox $R/bin/busybox
chmod 755 $R/bin/busybox
for a in $($R/bin/busybox --list); do
  case "$a" in busybox) continue;; esac
  ln -sf /bin/busybox $R/bin/$a
done
# virtio_net is a module in Debian's kernel: bring the initrd's copies (and
# their two dependencies) along, decompressed, for busybox insmod.
MODS=/mnt/c/Users/hkvla/AerOS/build/linux-extract/rootfs-tree/usr/lib/modules/6.12.107+deb13-amd64/kernel
mkdir -p $R/lib/modules $R/usr/share/udhcpc
for m in net/core/failover drivers/net/net_failover drivers/net/virtio_net; do
  xz -dc $MODS/$m.ko.xz > $R/lib/modules/$(basename $m).ko
done
cat > $R/usr/share/udhcpc/default.script <<'DHCP'
#!/bin/sh
case "$1" in
  bound|renew)
    ifconfig $interface $ip netmask ${subnet:-255.255.255.0} up
    [ -n "$router" ] && route add default gw ${router%% *} dev $interface
    : > /etc/resolv.conf
    for d in $dns; do echo "nameserver $d" >> /etc/resolv.conf; done ;;
esac
exit 0
DHCP
chmod 755 $R/usr/share/udhcpc/default.script
mknod -m 600 $R/dev/console c 5 1
mknod -m 666 $R/dev/null c 1 3
mknod -m 666 $R/dev/ttyS0 c 4 64
cat > $R/sbin/init <<'INIT'
#!/bin/busybox sh
/bin/busybox mount -t proc proc /proc
/bin/busybox mount -t sysfs sysfs /sys
/bin/busybox mount -t devtmpfs devtmpfs /dev 2>/dev/null
/bin/busybox mount -t tmpfs tmpfs /tmp
export PATH=/bin:/sbin:/usr/bin:/usr/sbin HOME=/root TERM=vt100
echo "AEROS_LINUX_INIT_OK pid=$$"
echo "AEROS_LINUX_UNAME $(uname -a)"
echo "AEROS_LINUX_ROOT $(ls / | tr '\n' ' ')"
# The initrd usually loaded virtio_net already (and udev may have renamed the
# interface), so tolerate "File exists" and look the NIC up by name.
insmod /lib/modules/failover.ko 2>/dev/null
insmod /lib/modules/net_failover.ko 2>/dev/null
insmod /lib/modules/virtio_net.ko 2>/dev/null
ifconfig lo up
NIC=$(ls /sys/class/net | grep -v '^lo$' | head -n 1)
if [ -n "$NIC" ] && ifconfig $NIC up; then
  ( udhcpc -i $NIC -n -q -t 10 -T 3 -s /usr/share/udhcpc/default.script >/dev/null 2>&1 \
      && echo "AEROS_LINUX_NET_OK $NIC $(ifconfig $NIC | grep 'inet addr' | tr -s ' ')" \
      && ping -c 2 -W 3 10.0.2.2 >/dev/null 2>&1 && echo "AEROS_LINUX_PING_OK gateway=10.0.2.2" ) &
fi
echo "Welcome to Linux inside AerOS"
export PS1='\[\e[1;36m\]aeros\[\e[0m\]:\w# '
# The framebuffer console (tty1) is what the AerOS "Linux" window shows.
setsid -c sh </dev/tty1 >/dev/tty1 2>&1 &
exec /bin/sh </dev/console >/dev/console 2>&1
INIT
chmod 755 $R/sbin/init
echo "root:x:0:0:root:/root:/bin/sh" > $R/etc/passwd
echo "root:x:0:" > $R/etc/group
rm -f rootfs.img
truncate -s 64M rootfs.img
mkfs.ext4 -q -F -L aeros-root -d $R rootfs.img
ls -la rootfs.img

cp -f /tmp/bb/rootfs.img /mnt/c/Users/hkvla/AerOS/build/linux-extract/rootfs-new.img
