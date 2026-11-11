#ifndef _LINUX_DMA_MAPPING_H
#define _LINUX_DMA_MAPPING_H

#include <linux/types.h>
#include <linux/device.h>
#include <linux/io.h>
#include <linux/slab.h>

#define DMA_BIT_MASK(n) (((n) == 64) ? ~0ULL : ((1ULL << (n)) - 1))
#define DMA_MAPPING_ERROR (~(dma_addr_t)0)

enum dma_data_direction {
	DMA_BIDIRECTIONAL = 0,
	DMA_TO_DEVICE = 1,
	DMA_FROM_DEVICE = 2,
	DMA_NONE = 3,
};

/* Coherent memory: whole zeroed pages the device may reach (AerOS maps
 * memory identically for the CPU and, through the IOMMU, for devices). */
void *dma_alloc_coherent(struct device *dev, size_t size, dma_addr_t *handle, gfp_t flags);
void dma_free_coherent(struct device *dev, size_t size, void *cpu, dma_addr_t handle);
#define dmam_alloc_coherent dma_alloc_coherent

dma_addr_t dma_map_single(struct device *dev, void *cpu, size_t size,
			  enum dma_data_direction direction);
static inline void dma_unmap_single(struct device *dev, dma_addr_t handle, size_t size,
				    enum dma_data_direction direction)
{
	(void)dev; (void)handle; (void)size; (void)direction;
}
static inline int dma_mapping_error(struct device *dev, dma_addr_t handle)
{
	(void)dev;
	return handle == DMA_MAPPING_ERROR;
}
static inline int dma_set_mask(struct device *dev, u64 mask) { (void)dev; (void)mask; return 0; }
static inline int dma_set_coherent_mask(struct device *dev, u64 mask) { (void)dev; (void)mask; return 0; }
static inline int dma_set_mask_and_coherent(struct device *dev, u64 mask) { (void)dev; (void)mask; return 0; }

#define DMA_ATTR_WEAK_ORDERING (1UL << 1)
#define DMA_ATTR_WRITE_COMBINE (1UL << 2)
#define DMA_ATTR_NO_KERNEL_MAPPING (1UL << 4)
#define DMA_ATTR_SKIP_CPU_SYNC (1UL << 5)
#define DMA_ATTR_FORCE_CONTIGUOUS (1UL << 6)
#define DMA_ATTR_ALLOC_SINGLE_PAGES (1UL << 7)
#define DMA_ATTR_NO_WARN (1UL << 8)
struct page;
/* Backed by alloc_pages, so virt_to_page works on the result. */
void *dma_alloc_attrs(struct device *dev, size_t size, dma_addr_t *handle, gfp_t gfp, unsigned long attrs);
void dma_free_attrs(struct device *dev, size_t size, void *cpu, dma_addr_t handle, unsigned long attrs);
dma_addr_t dma_map_page_attrs(struct device *dev, struct page *page, size_t offset, size_t size,
			      enum dma_data_direction dir, unsigned long attrs);
#define dma_map_page(dev, page, offset, size, dir) dma_map_page_attrs((dev), (page), (offset), (size), (dir), 0)
#define dma_unmap_page(dev, handle, size, dir) do { (void)(handle); } while (0)
#define dma_unmap_page_attrs(dev, handle, size, dir, attrs) do { (void)(handle); } while (0)
#define dma_sync_single_for_cpu(dev, handle, size, dir) do { } while (0)
#define dma_sync_single_for_device(dev, handle, size, dir) do { } while (0)
static inline bool dma_addressing_limited(struct device *dev) { (void)dev; return false; }
static inline size_t dma_max_mapping_size(struct device *dev) { (void)dev; return ~(size_t)0; }

static inline dma_addr_t dma_map_resource(struct device *dev, phys_addr_t phys_addr, size_t size,
					  enum dma_data_direction dir, unsigned long attrs)
{
	(void)dev; (void)size; (void)dir; (void)attrs;
	return (dma_addr_t)phys_addr;
}
static inline void dma_unmap_resource(struct device *dev, dma_addr_t addr, size_t size, enum dma_data_direction dir,
				      unsigned long attrs)
{
	(void)dev; (void)addr; (void)size; (void)dir; (void)attrs;
}
static inline int dma_set_max_seg_size(struct device *dev, unsigned int size) { (void)dev; (void)size; return 0; }

#endif
