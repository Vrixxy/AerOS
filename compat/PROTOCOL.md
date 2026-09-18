# AerOS Linux application bridge

The bridge uses a reliable ordered virtio-vsock or virtio-serial byte stream. Every frame starts with a 16-byte little-endian header.

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | ASCII `AERL` |
| 4 | 2 | Protocol version, currently `1` |
| 6 | 2 | Opcode |
| 8 | 4 | Nonzero request identifier |
| 12 | 4 | Payload length |

Payloads are limited to 65,536 bytes. The receiver rejects unknown versions, opcodes, zero request identifiers, truncated frames, trailing bytes, invalid UTF-8 paths, relative paths, traversal components, unrecognized permission bits, and paths longer than 1,024 bytes.

| Opcode | Value | Direction | Purpose |
|---|---:|---|---|
| Hello | 1 | Both | Negotiate features and establish guest identity |
| Launch | 2 | Host to guest | Start one validated executable |
| Exit | 3 | Guest to host | Report process termination |
| Window | 4 | Guest to host | Create or update window metadata |
| Damage | 5 | Guest to host | Publish changed buffer regions |
| Input | 6 | Host to guest | Deliver focused keyboard or pointer input |

A launch payload contains a 32-bit permission mask, a 16-bit path length, and the absolute UTF-8 executable path. Permission bits represent network, shared files, clipboard, microphone, camera, and accelerated graphics. An empty mask grants no external capability. Shared host files require separately minted handles and are never represented by guest-provided host paths.

The guest agent listens on `/dev/virtio-ports/org.aeros.compat`, clears the inherited environment, and launches one application as UID and GID 65534. The per-application guest receives only the virtual devices authorized by the launch permission mask. After sending the child PID, the agent waits for termination, reports the exit status, and completes the session without leaving a zombie process.
