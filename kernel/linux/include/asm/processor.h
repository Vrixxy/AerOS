#ifndef _ASM_PROCESSOR_H
#define _ASM_PROCESSOR_H

#include <linux/types.h>

#define X86_VENDOR_INTEL 0
#define X86_VENDOR_AMD 2
struct cpuinfo_x86 {
	u8 x86;
	u8 x86_vendor;
	u8 x86_model;
	u8 x86_stepping;
	int x86_clflush_size;
};
extern struct cpuinfo_x86 boot_cpu_data;
#define cpu_data(cpu) boot_cpu_data
#include <asm/cpufeature.h>
#define X86_VENDOR_INTEL 0
#define X86_VENDOR_AMD 2
#define cpu_relax_lowlatency() __asm__ volatile("pause" ::: "memory")

static inline unsigned int topology_num_cores_per_package(void) { return 1; }

#endif
