#ifndef _AEROS_LINUX_IO_64_NONATOMIC_LO_HI_H
#define _AEROS_LINUX_IO_64_NONATOMIC_LO_HI_H

#include <linux/io.h>
static inline u64 lo_hi_readq(const volatile void __iomem *addr)
{
	const volatile u32 __iomem *p = addr;
	u32 low = readl(p), high = readl(p + 1);

	return low | ((u64)high << 32);
}
static inline void lo_hi_writeq(u64 val, volatile void __iomem *addr)
{
	writel((u32)val, addr);
	writel((u32)(val >> 32), (volatile u8 __iomem *)addr + 4);
}
#endif
