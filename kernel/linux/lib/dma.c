#include <linux/dma-mapping.h>
#include <aeros/ops.h>

void *dma_alloc_coherent(struct device *dev, size_t size, dma_addr_t *handle, gfp_t flags)
{
	unsigned long long physical = 0;
	void *cpu = aeros->dma_alloc(size, &physical);

	(void)dev;
	(void)flags;
	*handle = cpu ? physical : 0;
	return cpu;
}

void dma_free_coherent(struct device *dev, size_t size, void *cpu, dma_addr_t handle)
{
	(void)dev;
	(void)size;
	(void)handle;
	if (cpu)
		aeros->dma_free(cpu);
}

dma_addr_t dma_map_single(struct device *dev, void *cpu, size_t size,
			  enum dma_data_direction direction)
{
	dma_addr_t physical = virt_to_phys(cpu);

	(void)dev;
	(void)direction;
	return aeros->dma_allow(physical, size) ? physical : DMA_MAPPING_ERROR;
}
