#include <linux/module.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/io.h>
#include <linux/delay.h>
#include <aeros/ops.h>

const struct aeros_ops *aeros;

/* Each block carries its size so krealloc and ksize work. */
struct block {
	size_t size;
	size_t pad;
};

void *kmalloc(size_t size, gfp_t flags)
{
	struct block *block = aeros->alloc(sizeof(struct block) + size, 16);

	if (!block)
		return 0;
	block->size = size;
	if (flags & __GFP_ZERO)
		memset(block + 1, 0, size);
	return block + 1;
}

void *kzalloc(size_t size, gfp_t flags)
{
	return kmalloc(size, flags | __GFP_ZERO);
}

void *kcalloc(size_t count, size_t size, gfp_t flags)
{
	if (size && count > (size_t)-1 / size)
		return 0;
	return kzalloc(count * size, flags);
}

size_t ksize(const void *pointer)
{
	return pointer ? ((const struct block *)pointer - 1)->size : 0;
}

void *krealloc(const void *pointer, size_t size, gfp_t flags)
{
	void *grown;

	if (pointer && ksize(pointer) >= size)
		return (void *)pointer;
	grown = kmalloc(size, flags);
	if (grown && pointer) {
		memcpy(grown, pointer, ksize(pointer));
		kfree(pointer);
	}
	return grown;
}

void kfree(const void *pointer)
{
	if (pointer && !IS_ERR(pointer))
		aeros->free((struct block *)pointer - 1);
}

void __iomem *ioremap(phys_addr_t physical, size_t length)
{
	return aeros->map_mmio(physical, length);
}

phys_addr_t virt_to_phys(const volatile void *address)
{
	return aeros->virt_to_phys((const void *)address);
}

u64 ktime_get_ns(void)
{
	return aeros->nanoseconds();
}

void ndelay(unsigned long nanoseconds)
{
	u64 end = aeros->nanoseconds() + nanoseconds;

	while (aeros->nanoseconds() < end)
		__asm__ volatile("pause");
}

void udelay(unsigned long microseconds)
{
	ndelay(microseconds * 1000);
}

void mdelay(unsigned long milliseconds)
{
	ndelay(milliseconds * 1000000);
}

void msleep(unsigned int milliseconds)
{
	mdelay(milliseconds);
}

void usleep_range(unsigned long minimum, unsigned long maximum)
{
	(void)maximum;
	udelay(minimum);
}

extern initcall_t __start_aeros_initcalls[];
extern initcall_t __stop_aeros_initcalls[];

void aeros_pci_report(struct aeros_linux_report *report);
void aeros_linux_poll(void);

/* AerOS calls this once, right after loading the module. */
extern bool aeros_amdgpu_enabled;

int aeros_linux_entry(const struct aeros_ops *ops, struct aeros_linux_report *report)
{
	aeros = ops;
	memset(report, 0, sizeof(*report));
	if (ops->version != AEROS_OPS_VERSION)
		return -EINVAL;
	report->poll = aeros_linux_poll;
	aeros_amdgpu_enabled = ops->options() & 1;
	for (initcall_t *call = __start_aeros_initcalls; call < __stop_aeros_initcalls; call++) {
		report->initcalls++;
		if ((*call)() != 0)
			report->initcall_failures++;
	}
	aeros_pci_report(report);
	return 0;
}
