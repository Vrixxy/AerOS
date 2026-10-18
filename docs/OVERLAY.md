# The union root

The boot image is read-only: its files are slices of the kernel binary. With a home volume mounted, `/` is a union of that image (the lower layer) and a directory on the home volume, `/home/.root/upper` (the upper layer), so the whole tree can be changed and the changes survive a reboot.

## What goes where

| Operation on a path in the image | Result |
|---|---|
| read, list, `stat` | the upper copy if there is one, else the image's |
| open for writing, `truncate`, `chmod` | the file is copied up (its bytes, then its mode) and the upper copy changed |
| create a file or directory | made in the upper layer, with the parent directories it needs |
| remove | the upper copy is deleted and, if the image has one, a whiteout hides it |
| rename a file | copy to the new name, then remove the old (so the old one is whited out) |
| rename a directory | only a directory the image does not have; for an image directory `rename` fails with `EBUSY` |
| list a directory | the image's entries minus whiteouts and minus names the upper layer has, then the upper layer's |

`/tmp`, `/data`, `/proc`, `/sys`, `/home` and `/media` are not part of the union, and neither is anything under a bind mount (`/var`, `/opt`, `/srv`, `/root`, `/etc` are bound to the home volume already).

## Marks

Removing an image file cannot delete it, so the upper layer records a whiteout. A directory (or file) created over a whited-out name is recorded as opaque: the name itself shows the new thing, and nothing below it shows through from the image. Marks live in `/home/.root/overlay`, one line each (`w/path` whiteout, `o/path` opaque), at most 64. Whiting out a directory drops the marks below it.

## Running programs

A program whose upper copy exists (or whose image copy is whited out) is read through the buffered path used for programs on the home volume (`vfs::needs_buffer`, `vfs::with_home_file`), not as a slice of the image. `vfs::file` refuses it, so a caller that only uses `vfs::file` sees the image's own copy as gone.

## Without a home volume

Nothing changes: the image stays read-only and writes to it are refused. If the home volume is removed the union stops until it is mounted again.

## Getting the image back

`overlay` (privileged) shows the state; `overlay reset` deletes the whole upper layer and the marks, which returns every path to what the boot image holds. It is a kernel shell command, so it works even if the upper layer has hidden a program the shell would otherwise start.

## Limits

- Directory handles that mix the layers use one of six slots; a seventh concurrent merged listing fails with `EMFILE`.
- A directory the image has cannot be renamed (the same as `EXDEV` on Linux overlays).
- Names the home volume cannot hold (longer than 58 bytes on AerFS, or what FAT refuses) cannot be created in the union.
- Copy-up reads and writes the whole file, so opening a large image file for writing takes time and space on the home volume.
- Case: FAT is case-insensitive, so two names that differ only in case are one name in the upper layer.
- Marks are rewritten as a whole on each change; a power cut during that write can drop the last change.

Tested by the boot-test gate `AEROS_OVERLAY`: a new file in `/bin`, listings, copy-up with the image proven untouched, a whiteout and re-creation over it, renames, a remount that must bring everything back, and a reset that must bring the image back.
