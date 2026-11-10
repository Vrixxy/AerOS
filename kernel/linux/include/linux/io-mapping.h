#ifndef _LINUX_IO_MAPPING_H
#define _LINUX_IO_MAPPING_H

#include <linux/io.h>
#include <linux/mm_types.h>

/* A whole aperture mapped once; local mappings are offsets into it. */
struct io_mapping {
	resource_size_t base;
	unsigned long size;
	pgprot_t prot;
	void __iomem *iomem;
};
static inline struct io_mapping *io_mapping_init_wc(struct io_mapping *mapping, resource_size_t base,
						    unsigned long size)
{
	mapping->base = base;
	mapping->size = size;
	mapping->iomem = ioremap_wc(base, size);
	return mapping->iomem ? mapping : NULL;
}
static inline void io_mapping_fini(struct io_mapping *mapping) { iounmap(mapping->iomem); }
static inline void __iomem *io_mapping_map_local_wc(struct io_mapping *mapping, unsigned long offset)
{
	return (char __iomem *)mapping->iomem + offset;
}
#define io_mapping_map_atomic_wc io_mapping_map_local_wc
#define io_mapping_map_wc(mapping, offset, size) io_mapping_map_local_wc((mapping), (offset))
#define io_mapping_unmap_local(address) do { (void)(address); } while (0)
#define io_mapping_unmap_atomic io_mapping_unmap_local
#define io_mapping_unmap io_mapping_unmap_local

#endif
