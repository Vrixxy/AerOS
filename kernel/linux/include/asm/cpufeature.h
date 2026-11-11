#ifndef _AEROS_ASM_CPUFEATURE_H
#define _AEROS_ASM_CPUFEATURE_H

#include <linux/types.h>
/* Linux numbers feature bits as 32 * word + bit; words 0 and 4 are CPUID 1's EDX and ECX. */
#define X86_FEATURE_FPU (0 * 32 + 0)
#define X86_FEATURE_PAT (0 * 32 + 16)
#define X86_FEATURE_CLFLUSH (0 * 32 + 19)
#define X86_FEATURE_XMM (0 * 32 + 25)
#define X86_FEATURE_XMM2 (0 * 32 + 26)
#define X86_FEATURE_XMM3 (4 * 32 + 0)
#define X86_FEATURE_XMM4_1 (4 * 32 + 19)
#define X86_FEATURE_XMM4_2 (4 * 32 + 20)
#define X86_FEATURE_AVX (4 * 32 + 28)
#define X86_FEATURE_HYPERVISOR (4 * 32 + 31)
static inline bool boot_cpu_has(int feature)
{
	u32 eax = 1, ebx, ecx = 0, edx;

	__asm__ volatile("cpuid" : "+a"(eax), "=b"(ebx), "+c"(ecx), "=d"(edx));
	if (feature / 32 == 0)
		return (edx >> (feature % 32)) & 1;
	if (feature / 32 == 4)
		return (ecx >> (feature % 32)) & 1;
	return false;
}
#define static_cpu_has boot_cpu_has
#define cpu_feature_enabled boot_cpu_has
#endif
