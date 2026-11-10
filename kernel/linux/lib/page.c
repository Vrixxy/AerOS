// SPDX-License-Identifier: MIT
#include <linux/mm.h>
#include <linux/vmalloc.h>
#include <linux/scatterlist.h>
#include <linux/xarray.h>
#include <linux/file.h>
#include <linux/dma-mapping.h>
#include <asm/processor.h>
#include <linux/ioport.h>
#include <aeros/ops.h>

/* Every page handed out, by page frame number, for pfn_to_page. */
static DEFINE_XARRAY(pages_by_pfn);

struct page *alloc_pages(gfp_t gfp, unsigned int order)
{
	unsigned long count = 1ul << order, i;
	unsigned long long token;
	struct page *pages;

	(void)gfp;
	pages = kcalloc(count, sizeof(*pages), GFP_KERNEL);
	if (!pages)
		return NULL;
	token = aeros->page_alloc(count);
	if (!token) {
		kfree(pages);
		return NULL;
	}
	for (i = 0; i < count; i++) {
		pages[i].head = pages;
		pages[i].phys = (token & ~0xfffull) + i * PAGE_SIZE;
		INIT_LIST_HEAD(&pages[i].lru);
		if (xa_is_err(xa_store(&pages_by_pfn, page_to_pfn(&pages[i]), &pages[i], GFP_KERNEL))) {
			while (i--)
				xa_erase(&pages_by_pfn, page_to_pfn(&pages[i]));
			aeros->page_free(token, count);
			kfree(pages);
			return NULL;
		}
	}
	pages->order = order;
	pages->token = token;
	atomic_set(&pages->_refcount, 1);
	return pages;
}

void __free_pages(struct page *page, unsigned int order)
{
	unsigned long count, i;

	if (!page)
		return;
	if (page->head != page) {
		pr_err("__free_pages: page %llx is not the first of its allocation\n", page->phys);
		return;
	}
	count = 1ul << page->order;
	if (order != page->order)
		pr_warn("__free_pages: order %u freed as %u\n", page->order, order);
	for (i = 0; i < count; i++)
		xa_erase(&pages_by_pfn, page_to_pfn(&page[i]));
	aeros->page_free(page->token, count);
	kfree(page);
}

struct page *pfn_to_page(unsigned long pfn)
{
	return xa_load(&pages_by_pfn, pfn);
}

/* Several pages become one virtual range only when they already are one
 * physically; anything else needs a page-table mapping AerOS doesn't hand
 * out yet. */
void *vmap(struct page **pages, unsigned int count, unsigned long flags, pgprot_t prot)
{
	unsigned int i;

	(void)flags;
	(void)prot;
	for (i = 1; i < count; i++) {
		if (pages[i]->phys != pages[0]->phys + i * PAGE_SIZE) {
			pr_warn("vmap: %u scattered pages can't be mapped yet\n", count);
			return NULL;
		}
	}
	return count ? page_address(pages[0]) : NULL;
}

void vunmap(const void *address)
{
	(void)address;
}

/* TTM sizes its page pool from this; AerOS doesn't report RAM to the
 * module yet, so it assumes 4 GiB. */
#define ASSUMED_RAM_PAGES ((4ul << 30) >> PAGE_SHIFT)

unsigned long totalram_pages(void)
{
	return ASSUMED_RAM_PAGES;
}

void si_meminfo(struct sysinfo *info)
{
	info->totalram = ASSUMED_RAM_PAGES;
	info->freeram = ASSUMED_RAM_PAGES / 2;
	info->totalhigh = 0;
	info->mem_unit = PAGE_SIZE;
}

struct shrinker *shrinker_alloc(unsigned int flags, const char *fmt, ...)
{
	struct shrinker *shrinker = kzalloc(sizeof(*shrinker), GFP_KERNEL);

	(void)fmt;
	if (shrinker)
		shrinker->flags = flags;
	return shrinker;
}

void shrinker_register(struct shrinker *shrinker)
{
	(void)shrinker;
}

void shrinker_free(struct shrinker *shrinker)
{
	kfree(shrinker);
}

int sg_alloc_table(struct sg_table *table, unsigned int count, gfp_t gfp)
{
	table->sgl = kcalloc(count ? count : 1, sizeof(*table->sgl), gfp);
	if (!table->sgl)
		return -ENOMEM;
	sg_init_table(table->sgl, count);
	table->nents = table->orig_nents = count;
	return 0;
}

void sg_free_table(struct sg_table *table)
{
	kfree(table->sgl);
	table->sgl = NULL;
	table->nents = table->orig_nents = 0;
}

/* Physically adjacent pages share an entry, up to max_segment bytes. */
int sg_alloc_table_from_pages_segment(struct sg_table *table, struct page **pages, unsigned int count,
				      unsigned int offset, unsigned long size, unsigned int max_segment,
				      gfp_t gfp)
{
	unsigned int segments = 0, i, start;
	struct scatterlist *sg;
	int error;

	max_segment = ALIGN_DOWN(max_segment, PAGE_SIZE);
	if (!max_segment)
		max_segment = PAGE_SIZE;
	for (i = 0; i < count; segments++) {
		unsigned long length = PAGE_SIZE;

		for (i++; i < count && pages[i]->phys == pages[i - 1]->phys + PAGE_SIZE &&
			  length + PAGE_SIZE <= max_segment; i++)
			length += PAGE_SIZE;
	}
	error = sg_alloc_table(table, segments, gfp);
	if (error)
		return error;
	sg = table->sgl;
	for (i = 0; i < count; sg = sg_next(sg)) {
		unsigned long length = PAGE_SIZE;
		unsigned int first_offset = i == 0 ? offset : 0;

		start = i;
		for (i++; i < count && pages[i]->phys == pages[i - 1]->phys + PAGE_SIZE &&
			  length + PAGE_SIZE <= max_segment; i++)
			length += PAGE_SIZE;
		length -= first_offset;
		if (length > size)
			length = size;
		size -= length;
		sg_set_page(sg, pages[start], (unsigned int)length, first_offset);
	}
	return 0;
}

void fput(struct file *file)
{
	if (file && atomic_long_dec_return(&file->f_count) == 0)
		kfree(file);
}

void *dma_alloc_attrs(struct device *dev, size_t size, dma_addr_t *handle, gfp_t gfp, unsigned long attrs)
{
	struct page *page = alloc_pages(gfp, (unsigned int)get_order(size));

	(void)dev;
	(void)attrs;
	if (!page)
		return NULL;
	*handle = page_to_phys(page);
	return page_address(page);
}

void dma_free_attrs(struct device *dev, size_t size, void *cpu, dma_addr_t handle, unsigned long attrs)
{
	(void)dev;
	(void)handle;
	(void)attrs;
	if (cpu)
		__free_pages(virt_to_page(cpu), (unsigned int)get_order(size));
}

dma_addr_t dma_map_page_attrs(struct device *dev, struct page *page, size_t offset, size_t size,
			      enum dma_data_direction dir, unsigned long attrs)
{
	(void)dev;
	(void)size;
	(void)dir;
	(void)attrs;
	return page_to_phys(page) + offset;
}

struct cpuinfo_x86 boot_cpu_data = { .x86 = 0x17, .x86_vendor = X86_VENDOR_AMD, .x86_clflush_size = 64 };
struct resource iomem_resource = { .name = "PCI mem", .end = ~0ull };
