#ifndef _LINUX_SCATTERLIST_H
#define _LINUX_SCATTERLIST_H

#include <linux/mm.h>
#include <linux/dma-mapping.h>

/* A scatter list is one flat array here (no chaining). */
struct scatterlist {
	struct page *page;
	unsigned int offset;
	unsigned int length;
	dma_addr_t dma_address;
	unsigned int dma_length;
	bool last;
};
struct sg_table {
	struct scatterlist *sgl;
	unsigned int nents;
	unsigned int orig_nents;
};

#define sg_dma_address(sg) ((sg)->dma_address)
#define sg_dma_len(sg) ((sg)->dma_length)
static inline struct page *sg_page(struct scatterlist *sg) { return sg->page; }
static inline void sg_set_page(struct scatterlist *sg, struct page *page, unsigned int length, unsigned int offset)
{
	sg->page = page;
	sg->offset = offset;
	sg->length = length;
}
static inline void sg_assign_page(struct scatterlist *sg, struct page *page) { sg->page = page; }
static inline bool sg_is_last(struct scatterlist *sg) { return sg->last; }
static inline void sg_mark_end(struct scatterlist *sg) { sg->last = true; }
static inline struct scatterlist *sg_next(struct scatterlist *sg) { return sg->last ? NULL : sg + 1; }
static inline phys_addr_t sg_phys(struct scatterlist *sg) { return page_to_phys(sg->page) + sg->offset; }
static inline void *sg_virt(struct scatterlist *sg) { return (char *)page_address(sg->page) + sg->offset; }
#define for_each_sg(sglist, sg, count, i) \
	for ((i) = 0, (sg) = (sglist); (i) < (count); (i)++, (sg) = sg_next(sg))
#define for_each_sgtable_sg(table, sg, i) for_each_sg((table)->sgl, sg, (table)->orig_nents, i)
#define for_each_sgtable_dma_sg(table, sg, i) for_each_sg((table)->sgl, sg, (table)->nents, i)

int sg_alloc_table(struct sg_table *table, unsigned int count, gfp_t gfp);
void sg_free_table(struct sg_table *table);
int sg_alloc_table_from_pages_segment(struct sg_table *table, struct page **pages, unsigned int count,
				      unsigned int offset, unsigned long size, unsigned int max_segment,
				      gfp_t gfp);
static inline int sg_alloc_table_from_pages(struct sg_table *table, struct page **pages, unsigned int count,
					    unsigned int offset, unsigned long size, gfp_t gfp)
{
	return sg_alloc_table_from_pages_segment(table, pages, count, offset, size, 0xffffffffu, gfp);
}
static inline void sg_init_table(struct scatterlist *sgl, unsigned int count)
{
	memset(sgl, 0, sizeof(*sgl) * count);
	if (count)
		sgl[count - 1].last = true;
}
static inline void sg_init_one(struct scatterlist *sg, const void *buffer, unsigned int length)
{
	sg_init_table(sg, 1);
	sg_set_page(sg, virt_to_page(buffer), length, offset_in_page(buffer));
}

/* Device addresses equal physical ones (the IOMMU already allows them). */
static inline int dma_map_sgtable(struct device *dev, struct sg_table *table, enum dma_data_direction dir,
				  unsigned long attrs)
{
	struct scatterlist *sg;
	unsigned int i;

	(void)dev; (void)dir; (void)attrs;
	for_each_sgtable_sg(table, sg, i) {
		sg->dma_address = sg_phys(sg);
		sg->dma_length = sg->length;
	}
	table->nents = table->orig_nents;
	return 0;
}
static inline void dma_unmap_sgtable(struct device *dev, struct sg_table *table, enum dma_data_direction dir,
				     unsigned long attrs)
{
	(void)dev; (void)table; (void)dir; (void)attrs;
}
#define dma_map_sg(dev, sgl, count, dir) ({ \
	struct sg_table __sgt = { (sgl), (count), (count) }; \
	dma_map_sgtable((dev), &__sgt, (dir), 0); (int)(count); })
#define dma_unmap_sg(dev, sgl, count, dir) do { } while (0)
#define dma_sync_sgtable_for_cpu(dev, table, dir) do { } while (0)
#define dma_sync_sgtable_for_device(dev, table, dir) do { } while (0)

struct sg_page_iter {
	struct scatterlist *sg;
	unsigned int sg_pgoffset;
	unsigned int nents;
	int pg_advance;
};
struct sg_dma_page_iter {
	struct sg_page_iter base;
};
static inline unsigned int __sg_page_count(struct scatterlist *sg, bool dma)
{
	return (unsigned int)PFN_UP(sg->offset + (dma ? sg->dma_length : sg->length));
}
static inline bool __sg_page_iter_next(struct sg_page_iter *iter, bool dma)
{
	if (!iter->nents || !iter->sg)
		return false;
	iter->sg_pgoffset += iter->pg_advance;
	iter->pg_advance = 1;
	while (iter->sg_pgoffset >= __sg_page_count(iter->sg, dma)) {
		iter->sg_pgoffset -= __sg_page_count(iter->sg, dma);
		iter->sg = sg_next(iter->sg);
		if (!--iter->nents || !iter->sg)
			return false;
	}
	return true;
}
static inline void __sg_page_iter_start(struct sg_page_iter *iter, struct scatterlist *sglist,
					unsigned int nents, unsigned long pgoffset)
{
	iter->pg_advance = 0;
	iter->sg = sglist;
	iter->nents = nents;
	iter->sg_pgoffset = (unsigned int)pgoffset;
}
static inline struct page *sg_page_iter_page(struct sg_page_iter *iter)
{
	return nth_page(sg_page(iter->sg), iter->sg_pgoffset);
}
static inline dma_addr_t sg_page_iter_dma_address(struct sg_dma_page_iter *iter)
{
	return sg_dma_address(iter->base.sg) + ((dma_addr_t)iter->base.sg_pgoffset << PAGE_SHIFT);
}
#define for_each_sg_page(sglist, iter, nents, pgoffset) \
	for (__sg_page_iter_start((iter), (sglist), (nents), (pgoffset)); __sg_page_iter_next((iter), false);)
#define for_each_sg_dma_page(sglist, iter, nents, pgoffset) \
	for (__sg_page_iter_start(&(iter)->base, (sglist), (nents), (pgoffset)); \
	     __sg_page_iter_next(&(iter)->base, true);)
#define for_each_sgtable_page(table, iter, pgoffset) for_each_sg_page((table)->sgl, iter, (table)->orig_nents, pgoffset)
#define for_each_sgtable_dma_page(table, iter, pgoffset) \
	for_each_sg_dma_page((table)->sgl, iter, (table)->nents, pgoffset)

#endif
