# Software volumes (linear, stripe, mirror)

`kernel/src/raid.rs` joins several disks into one block device. The result is a `block::Disk::Volume(n)` (`md0`, `md1`), so everything that takes a disk name works on it: the block layer's cache and scheduler, `aerfs format md0`, `lsblk`-style listings.

| Level | Layout | Capacity | Survives a lost disk |
| --- | --- | --- | --- |
| `linear` | members end to end | sum | no |
| `stripe` | chunks (128 sectors from the shell) round-robin over the members | members x the smallest, whole chunks | no |
| `mirror` | every member holds everything | the smallest | yes, down to one working member |

## The `raid` command (privileged)

```
raid                                   list volumes and their members
raid create linear|stripe|mirror <disk> <disk>... --yes
raid stop md0                          the volume must not be mounted
raid fail md0 1                        mark a mirror member failed (administration, tests)
raid replace md0 1 sdc                 put a disk in that slot and resynchronise it
```

A mirror notices a member that errors on its own: reads move to the next working member, writes carry on to the rest, and the volume is shown as DEGRADED. The last working member is never taken out of service. `replace` copies the whole volume from a healthy member onto the new disk before the slot is in service again.

Disks that are the boot disk, mounted, or already in another volume are refused.

## Headers and assembly

The last sector of every member holds a header: a magic number and checksum, the volume's UUID, its level and chunk size, the member count, this member's slot, an event counter and a bitmap of failed slots. Creating a volume writes it, so a member gives up its last sector (a GPT backup header there is lost; `raid create` asks for `--yes` for that reason).

At boot, `raid::assemble` reads the last sector of every free disk (not the boot disk, nothing mounted) and puts volumes back together from the headers, whatever the order the disks are found in. The event counter goes up on every membership change (a member failing, a replacement), and only members still in service are rewritten, so a member that was away during a change comes back with a lower count and is left out as failed. A linear or stripe volume with a member missing is not assembled; a mirror runs on what is there.

## Limits

- At most two volumes of up to four members each.
- No write-intent bitmap: a resync always copies everything, and a mirror member that comes back after missing writes must be replaced (resynchronised) before it is trusted again.
- Volumes are not nested and a volume cannot be a boot disk.

## Test

`AEROS_RAID` in the boot test builds each level over in-memory members (`Member::Memory`) and checks: a write across the join of a linear volume lands on both members; a stripe puts chunk *n* on member *n mod count*, row *n / count*, and an unaligned read spans chunks; a mirror holds both copies, keeps reading and writing with a member broken, refuses to lose its last copy, and `replace` leaves the new member identical; the block layer reads and writes a volume; refusals (member in two volumes, wrong counts, duplicate disk) leave nothing behind; headers: a stopped volume is assembled again from its members in a different order with the same UUID and data, a member that missed a change comes back failed, and blank disks assemble to nothing.
