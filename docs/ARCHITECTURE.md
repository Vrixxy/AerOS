# AerOS architecture

AerOS starts as a monolithic kernel with explicit subsystem boundaries. This minimizes context-switching overhead while retaining a future path for moving failure-prone drivers into isolated user processes.

The UEFI entry layer exists only to acquire the framebuffer, ACPI root pointer, and authoritative physical memory map. After a successful `ExitBootServices`, firmware boot services are never called again.

The physical allocator consumes only UEFI conventional memory above the first MiB. Firmware runtime, ACPI, MMIO, persistent, unusable, and reserved regions remain unavailable. Loader code and data remain protected because UEFI does not report them as conventional memory at handoff.

AerOS installs a kernel-owned PML4 after physical-memory initialization. The first milestone inherits firmware mappings for continuity and creates a private high-half subtree for kernel mappings. The switch is accepted only after a volatile read/write probe through the new virtual address, NX activation, and supervisor write protection.

The 8259 and PIT remain a calibration fallback. Normal scheduler ticks come from a divided Local APIC counter calibrated against a bounded PIT interval, with APIC EOI issued before any scheduler context switch. External IRQ routing is owned by the I/O APIC: MADT source overrides select the GSI, redirection entries target a chosen Local APIC, and both legacy PIC masks remain closed.

UEFI reserves a page below the first MiB for the application-processor trampoline before memory-map handoff. The bootstrap processor sends the architectural INIT/SIPI sequence, and each AP transitions from real mode through protected mode into the shared long-mode page tables. Up to eight processors receive isolated GDT, TSS, ring-0, double-fault, and bootstrap stacks. Bring-up is accepted only after every AP validates its APIC identity, completes a cache-line-isolated workload, enters interrupt-driven idle, and acknowledges a fixed IPI.

General monotonic time comes from the ACPI-discovered HPET rather than accumulated scheduler ticks. AerOS validates the timer period and counter width, starts the main counter from zero, and performs femtosecond-to-nanosecond conversion with a widened intermediate.

Boot entropy is collected from CPUID-gated RDSEED or RDRAND samples and mixed with timing state into a ChaCha20 generator. User address-space slots are selected from the generator, and random data crosses the privilege boundary only through validated writable mappings.

ELF process entry receives a Linux-shaped initial stack. AerOS maps the program-header image, aligns the vector area to 16 bytes, installs `argc`, `argv`, an empty environment, and bounded auxiliary vectors including a fresh 16-byte `AT_RANDOM` value.

Each process image owns an anonymous-memory frame pool that remains inaccessible until a successful mapping operation. Break growth, anonymous mappings, protection changes, and unmapping update leaf permissions with local TLB invalidation, enforce W^X, and zero frames both before exposure and after release.

Process return and fault paths remove the owned PML4 branch before reuse, flush the active translation context, scrub page tables and user data, and return the complete contiguous allocation through one checked allocator transaction. Boot validation compares allocator state before process creation and after all reaping to detect leaks.

Kernel diagnostics use COM1 independently of the framebuffer. Every automated boot emits structured stage records and reaches `AEROS_READY` only after the allocator invariant test and framebuffer render have completed.

Plus Jakarta Sans is the interface face. Roboto Mono is the diagnostic and developer face. Runtime rendering consumes deterministic pre-rasterized atlases derived from the bundled original font files.

The compatibility design keeps directly supported Linux ELF and syscall execution above AerOS primitives and routes applications that need the broader Linux environment into an isolated Debian 13 guest.

Executable dispatch validates ELF class, encoding, ABI, machine, program-header bounds, loadability, and interpreter paths before choosing a runtime. Supported static x86-64 PIE executables remain on the native AerOS Linux ABI. Dynamically linked ELF files, fixed-address executables, and valid interpreter scripts route to the Debian compatibility runtime. Malformed inputs are rejected before either loader receives them.

The compatibility policy requires VT-x or AMD-V in production and forbids software CPU emulation. The Debian base image is verified against its official SHA-512 manifest and remains immutable beneath a writable overlay. Host files, devices, clipboard, and network access are denied unless granted through capability-scoped bridge endpoints. The first implementation stage reports VMX, SVM, nested-paging, and hypervisor exposure without enabling privileged virtualization instructions until the matching backend owns interrupt, memory, and teardown paths.

The pinned Debian image build, guest-agent digest, acceleration policy, and architecture are recorded in a generated runtime manifest. Provisioning uses a deterministic FAT12 CIDATA volume. A static Rust-musl agent is installed as a hardened system service and communicates over a named virtio-serial port. The hardware-accelerated acceptance test starts from a fresh overlay without a network device, validates the agent handshake, launches `/usr/bin/true` as UID and GID 65534, requires exit status zero, terminates the VM, and discards only the test overlay.

The first executable path is a read-only initramfs mounted into a bounded VFS. File descriptors carry generations so a closed numeric handle cannot silently alias a later open file. The ELF loader currently accepts checked x86-64 position-independent images, rejects writable-executable mappings, maps load segments into user-only pages, and installs an unmapped stack guard.

Native user calls enter through the processor `SYSCALL` mechanism using the Linux x86-64 register convention. AerOS swaps to a dedicated supervisor stack before invoking Rust, masks unsafe status flags in hardware, validates the entire requested user range through all four page-table levels, and opens SMAP access only around the exact copy operation.

Every online processor owns a cache-line-separated CPU-local record and a dedicated 32 KiB syscall stack. GS and kernel-GS bases are initialized on the processor that consumes them, entry uses `SWAPGS` before loading supervisor state, and exit restores the user GS context before `SYSRET`.

The current Linux surface covers basic file I/O and metadata, polling, seeking, executable-path discovery, identity and time queries, resource limits, TLS bases, thread registration, randomness, anonymous memory, basic futex outcomes, pathname mutation, and process exit. Structures exposed to applications use Linux x86-64 field sizes and offsets. Separately compiled `no_std` and ordinary Rust `std` static PIE programs exercise the ABI without being linked into the kernel.

Each process owns a complete bounded descriptor table including standard input, output, and error. `dup`, `dup2`, `dup3`, `F_DUPFD`, and `F_DUPFD_CLOEXEC` create descriptor-local entries that reference the same VFS handle or socket, so file position and status flags remain shared while close-on-exec state stays local. Replacement and close release the underlying object only after its final descriptor disappears, and process teardown deduplicates shared handles before returning them to the VFS.

Private file mappings allocate zero-filled process frames, populate them directly from the VFS without changing the descriptor cursor, preserve zero fill beyond end-of-file, and apply the requested final page permissions from the start. Failed population tears the complete mapping down before returning an error. The current implementation is eager and bounded by the per-process anonymous arena; shared mappings and demand paging remain later milestones.

Exceptions are classified by their saved privilege level. A kernel-origin fault remains fatal and emits a structured register record. A user-origin fault records the vector, hardware error, and fault address, converts the exception to a signal-style process status, abandons the compromised user context, and returns through the trusted kernel continuation.

The storage path takes native ownership of PCI AHCI controllers after firmware handoff. It enables memory decoding and bus mastering, validates the implemented port mask against controller capabilities, selects an active SATA link, installs page-aligned command/FIS/table memory, and uses bounded polling for ATA IDENTIFY and DMA reads.

Partition discovery accepts bounded legacy MBR entries or validates GPT header and entry-array CRCs before exposing non-overlapping ranges. The first disk filesystem reader derives FAT geometry from the BPB, rejects inconsistent cluster counts, bounds every chain walk, and resolves the EFI boot image through native sector reads.

The first native network path supports Intel e1000-family PCI devices. It provisions aligned legacy transmit and receive descriptor rings from physical memory, programs MMIO queue state with interrupts masked, validates the permanent MAC and negotiated link, and proves bidirectional DMA by exchanging ARP with the virtual gateway.

Ethernet ring ownership persists behind a synchronized packet interface. The initial network layer emits and parses bounded IPv4 packets, rejects fragmented or length-inconsistent frames, validates Internet checksums, and proves the stack with an ICMP echo exchange after ARP resolution.

Network configuration is acquired through a validated DHCP discover, offer, request, and acknowledgement exchange. The resulting address, gateway, and DNS server feed reusable UDP transmit and receive paths. DNS parsing bounds labels, compression pointers, resource records, and payload sizes. The command shell can issue fresh ICMP probes and A-record queries through the same native stack.

The native TCP client constructs checked IPv4 and TCP headers, validates incoming Internet and transport checksums, verifies the SYN acknowledgement, tracks both sequence spaces, acknowledges received payload and FIN segments, and bounds connection and receive time. Browser issues HTTP/1.1 requests only after DNS succeeds, parses the response status and bounded HTML title and heading, and exposes connection results as structured serial records.

The root VFS remains immutable. `/tmp` is a bounded writable subtree with reusable inode slots and fixed-size data storage. Mutation verifies parent write access and rejects attempts to remove the mount root, mutate immutable nodes, remove non-empty directories, reuse open nodes, or move a directory beneath itself.

Linux pathname compatibility maintains a bounded per-process current working directory. Absolute and `AT_FDCWD`-relative paths are canonicalized without allocation, including repeated separators and `.` or `..` components, before reaching the VFS. `chdir` accepts directories only, `getcwd` returns the canonical path, and relative open, stat, statx, and access requests share the same resolver.

Legacy and directory-relative Linux `mkdir`, `mkdirat`, `rename`, `renameat`, `renameat2`, `unlink`, `unlinkat`, and `rmdir` calls use that resolver and operate on the native VFS. Rename validates both parents, prevents directory cycles and incompatible replacements, rejects replacement of open or non-empty destinations, supports `RENAME_NOREPLACE`, and atomically releases compatible closed targets. The ordinary Rust standard-library validation process exercises both legacy and modern relative calls, including replacement and no-replace behavior. Boot acceptance requires the exact syscall counts and complete restoration of the pre-test tmpfs node, byte, and handle totals.

Mutable files support descriptor-based truncation with zero-filled extension and discarded-byte scrubbing on shrink. Linux `ftruncate`, `fsync`, `fdatasync`, `chmod`, `fchmod`, and `fchmodat` expose resizing, durability boundaries, and permission updates to applications. The standard-library process writes, shrinks, synchronizes, changes mode, reopens, validates, and removes a real tmpfs file during every accepted boot.

The local command environment is a kernel-resident recovery shell until AerOS has persistent user processes, a TTY ABI, and dynamic application loading. `aersh` uses a single declarative registry for discovery, manuals, completion, privilege policy, and dispatch. Its parser bounds command and argument storage and supports single quotes, double quotes, and escaping without allocation.

Each scheduled kernel task owns an aligned extended-processor-state image sized from CPUID leaf `0x0d`. The scheduler captures a clean architectural template, copies it into new tasks, saves and restores XSAVE state at every cooperative and timer-driven switch, and releases the image with the task. The FXSAVE path remains available when XSAVE is absent. Boot validation runs two tasks with conflicting XMM15 values and requires both values to survive interleaved switches.

Keyboard input uses the first i8042 port. IRQ1 is translated through MADT legacy routing, installed as I/O APIC vector 52, and consumed through a bounded single-producer queue. The framebuffer terminal sleeps with interrupts enabled when the queue is empty and redraws only after input events.

`ear` grants effective UID 0 only for one nested command, prevents recursive elevation, restores the caller identity on every return path, and records successful and denied elevation counts. The current local-console session receives the elevation capability during boot; authentication and persistent policy storage remain future security milestones.

AerUI begins with renderer-independent integer geometry, fixed-point logical scaling, independent corner radii, bounded interaction state, and an explicit painter. Button activation semantics live in the shared state machine rather than individual visual components. Pointer capture requires a primary press inside the bounds, release activates only inside, and focused Enter or Space produces the same activation result.

The software frost path captures the backdrop into static bounded storage, performs linear-time separable horizontal and vertical blur, applies saturation, brightness, tint, and deterministic noise, then paints rounded clipping, borders, inner highlights, inner shadows, and layered outer shadows. Surfaces beyond 1,048,576 captured pixels fall back to a tinted rounded surface rather than allocating unpredictably. The interface permits a later GPU compositor to preserve component behavior and styling while replacing the painter implementation.

The desktop uses a 752×458 logical canvas and scales between one-half and two times while preserving aspect ratio. Its wallpaper is embedded as RGB565 and cover-scaled with bilinear sampling. The 702×71 dock, 702×420 app switcher, 314×328 quick-settings panel, and 531×303 application window share the same glass painter and reusable button implementation. The dock exposes seven scalable vector-icon controls and separate RTC-derived UTC time and date regions. Settings and Browser are distinct interactive application states. Runtime frames are composed into a fixed bounded off-screen surface and copied to the device framebuffer only after completion, so keyboard activation never exposes a partially rendered blur pass. Completed frames emit auditable redraw records.
