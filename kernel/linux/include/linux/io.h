#ifndef _LINUX_IO_H
#define _LINUX_IO_H

#include <linux/types.h>

static inline u8 readb(const volatile void __iomem *address) { return *(const volatile u8 *)address; }
static inline u16 readw(const volatile void __iomem *address) { return *(const volatile u16 *)address; }
static inline u32 readl(const volatile void __iomem *address) { return *(const volatile u32 *)address; }
static inline u64 readq(const volatile void __iomem *address) { return *(const volatile u64 *)address; }
static inline void writeb(u8 value, volatile void __iomem *address) { *(volatile u8 *)address = value; }
static inline void writew(u16 value, volatile void __iomem *address) { *(volatile u16 *)address = value; }
static inline void writel(u32 value, volatile void __iomem *address) { *(volatile u32 *)address = value; }
static inline void writeq(u64 value, volatile void __iomem *address) { *(volatile u64 *)address = value; }

#define ioread8 readb
#define ioread16 readw
#define ioread32 readl
#define iowrite8 writeb
#define iowrite16 writew
#define iowrite32 writel
#define readl_relaxed readl
#define writel_relaxed writel

void __iomem *ioremap(phys_addr_t physical, size_t length);
#define ioremap_wc ioremap
#define ioremap_uc ioremap
static inline void iounmap(volatile void __iomem *address) { (void)address; }

phys_addr_t virt_to_phys(const volatile void *address);

static inline void memset_io(volatile void __iomem *address, int value, size_t length)
{
	volatile u8 *bytes = (volatile u8 *)address;

	while (length--)
		*bytes++ = (u8)value;
}
static inline void memcpy_toio(volatile void __iomem *to, const void *from, size_t length)
{
	volatile u8 *out = (volatile u8 *)to;
	const u8 *in = from;

	while (length--)
		*out++ = *in++;
}
static inline void memcpy_fromio(void *to, const volatile void __iomem *from, size_t length)
{
	const volatile u8 *in = (const volatile u8 *)from;
	u8 *out = to;

	while (length--)
		*out++ = *in++;
}
#define ioremap_cache ioremap
#define ioremap_np ioremap
enum {
	MEMREMAP_WB = 1 << 0,
	MEMREMAP_WT = 1 << 1,
	MEMREMAP_WC = 1 << 2,
	MEMREMAP_ENC = 1 << 3,
	MEMREMAP_DEC = 1 << 4,
};
#define memremap(physical, length, flags) ((void *)ioremap((physical), (length)))
#define memunmap(address) iounmap(address)

/* Memory types come from AerOS's page tables; MTRR/PAT hooks report success. */
static inline int arch_phys_wc_add(unsigned long base, unsigned long size) { (void)base; (void)size; return 0; }
static inline void arch_phys_wc_del(int handle) { (void)handle; }
static inline int arch_io_reserve_memtype_wc(resource_size_t start, resource_size_t size) { (void)start; (void)size; return 0; }
static inline void arch_io_free_memtype_wc(resource_size_t start, resource_size_t size) { (void)start; (void)size; }

#endif
