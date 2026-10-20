#!/bin/bash
# Builds userspace/dyntest/dyntest.c with Debian's gcc (dynamically linked,
# against glibc) into assets/userspace/dyntest. Run from WSL:
#   wsl -d Debian -- bash tools/build-dyntest.sh /mnt/c/Users/<you>/AerOS
set -e
root="${1:-.}"
gcc -O1 -pthread -o "$root/assets/userspace/dyntest" "$root/userspace/dyntest/dyntest.c" -lm -ldl
ls -la "$root/assets/userspace/dyntest"
