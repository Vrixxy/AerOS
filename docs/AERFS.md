# AerFS

AerFS is AerOS's own crash-safe filesystem: copy-on-write, with a checksum on every block pointer. It lives in `kernel/src/aerfs.rs` (device-generic, `core` only) with `kernel/src/volume.rs` presenting it to the VFS next to FAT.

## Why it cannot be left half-written

Nothing is overwritten in place. A change writes new blocks (the data, the tree blocks above it, the inode-table blocks) into free space, then commits by writing **one 512-byte superblock sector**, alternating between two slots (blocks 0 and 1). Each superblock carries a generation number and a CRC-32.

- A power cut before that sector lands leaves the previous tree untouched: none of its blocks were reused.
- A power cut after it leaves the new tree complete.
- A torn superblock write fails its CRC, and mount falls back to the other slot.

There is no journal to replay and no repair step. Every public call is one transaction, so `rename` (including over an existing file) is atomic.

Every block pointer stores the CRC-32 of the block it points to. A flipped bit is an error (`Corrupt`), never wrong data. `fsck` reads every block and checks checksums, blocks referenced twice, directory entries, unreachable inodes, and the allocation map against what is reachable.

## Layout

4 KiB blocks. Files, directories and the inode table are radix trees of 512-way pointer blocks over data blocks (up to 512 GiB per file, sparse). Inodes are 128 bytes. Directory entries are 64 bytes with names up to 58 bytes. Free space is not stored: the allocation map is rebuilt at mount by walking the metadata (not the file data), so it cannot disagree with the tree.

Limits: 256 MiB per volume (65,536 blocks), 65,535 inodes, two volumes mounted at once, no hard links, no symbolic links, no extended attributes, timestamps are modification times only.

## Using it

```
aerfs format <disk> [label]    # a whole disk; mounts at /media/<label>
aerfs mount <disk>             # a disk that already holds one
fsck /media/<label>            # read everything and check it
```

Disk names are the ones `lsblk` shows (`sdu`, `mmcblk0`, `nvme0n1`, `vda`, `sda`). `format` refuses the boot disk and a disk that is already mounted. USB sticks and SD cards that carry an AerFS volume mount by themselves, like FAT ones.

## How it was tested

- `tools/aerfs-host` (`cargo test`): the same source against a RAM device that can lose power in the middle of a block write. A model of the expected file tree is compared with the filesystem after 4,000 random operations and after a remount; a 17-step script is cut at **every** sector write, and after each cut the recovered filesystem must mount, pass `fsck`, and equal the state before or after the step that was running; flipping a bit in each block of a volume must be caught by mount, `fsck` or the read; running out of space must leave everything intact; the allocation map kept at run time must equal the one rebuilt at mount.
- Boot-test gates `AEROS_AERFS_CRASH` (the same cut-at-every-sector test inside the kernel, on the RAM disk, through the volume layer) and `AEROS_AERFS_VFS` (create, write, read back, list, rename, truncate, `fsck`, unmount and remount through `/media`).

## What it does not do

- Data is not encrypted.
- Atomicity is per call: a program that needs several files to change together still needs its own rename-into-place step.
- Sector writes are assumed atomic and ordered by `flush`; the drivers write through, and a disk that reorders or lies about flushes can break any filesystem that depends on write ordering.
- AerFS can be the home volume. An AerFS disk labelled `AEROSHOME` is adopted as `/home` at boot (or as soon as it is attached, `aerfs format <disk> AEROSHOME` does it directly when no home is mounted), gets the usual folders, and carries the persistent system directories. Only one disk should carry that label. A blank-marked disk is still formatted as FAT.
- `/` itself is the read-only boot image on purpose (it is the signed, measured part of the system). What persists and is writable: `/home`, `/etc`, `/var`, `/opt`, `/srv`, `/root` (the last five are bound to `/home/.root/...`; `/etc` is seeded on a new volume with `hostname`, `hosts`, `os-release` and `aeros-release`, and the image's own `/etc` stays hidden underneath), `/data` and the volatile `/tmp`. A union overlay that would let any path of `/` be written (with copy-up and deletion markers) is not done; a change to a file of the image fails with a permission error. A file `/etc` already holds is never overwritten by a newer image.
