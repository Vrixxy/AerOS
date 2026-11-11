/* The functions AerOS hands the Linux driver module at load time. Matches
 * `LinuxOps` in kernel/src/linux.rs (System V calling convention). */
#ifndef AEROS_OPS_H
#define AEROS_OPS_H

#define AEROS_OPS_VERSION 6

struct aeros_pci_info {
	unsigned char bus, slot, function, revision;
	unsigned short vendor, device;
	unsigned short subsystem_vendor, subsystem_device;
	unsigned int class_code; /* class << 16 | subclass << 8 | interface */
	unsigned int irq_line;
	unsigned long long bars[6];      /* physical base, 0 if unused */
	unsigned long long bar_sizes[6];
	unsigned int bar_flags[6];       /* bit 0 I/O, bit 1 64-bit, bit 2 prefetch */
};

struct aeros_ops {
	unsigned long version;
	void (*log)(const char *text, unsigned long length);
	void *(*alloc)(unsigned long size, unsigned long align);
	void (*free)(void *pointer);
	unsigned long long (*nanoseconds)(void);
	unsigned long (*pci_count)(void);
	int (*pci_get)(unsigned long index, struct aeros_pci_info *info);
	unsigned int (*pci_read)(unsigned int bdf, unsigned int offset, unsigned int bytes);
	void (*pci_write)(unsigned int bdf, unsigned int offset, unsigned int bytes,
			  unsigned int value);
	void *(*map_mmio)(unsigned long long physical, unsigned long long length);
	unsigned long long (*virt_to_phys)(const void *address);
	const void *(*acpi_table)(const char *signature, unsigned long *length);
	int (*framebuffer)(unsigned long long *base, unsigned long long *size);
	void *(*dma_alloc)(unsigned long size, unsigned long long *physical);
	void (*dma_free)(void *cpu);
	int (*dma_allow)(unsigned long long physical, unsigned long size);
	const void *(*firmware)(const char *name, unsigned long *size);
	/* Zeroed pages the GPU may reach; returns a token (0 on failure). */
	unsigned long long (*page_alloc)(unsigned long long pages);
	void (*page_free)(unsigned long long token, unsigned long long pages);
	unsigned long long (*options)(void);
};

/* What the module reports back to AerOS. */
struct aeros_linux_report {
	unsigned int initcalls;
	unsigned int initcall_failures;
	unsigned int drivers;
	unsigned int bound;
	void (*poll)(void);
};

extern const struct aeros_ops *aeros;

#endif
