#ifndef _ASM_SET_MEMORY_H
#define _ASM_SET_MEMORY_H

#include <linux/mm_types.h>

/* AerOS keeps its own 1:1 mapping of RAM write-back. Pages a device will
 * read without snooping (write-combined or uncached in Linux) get their
 * cache lines written back here, so no stale line can later land on top of
 * what the device wrote; mappings of them made with vmap are uncached. */
void aeros_flush_address(unsigned long address, unsigned long bytes);
void aeros_flush_pages(struct page **pages, int count);

static inline int set_memory_wc(unsigned long address, int pages) { aeros_flush_address(address, (unsigned long)pages << 12); return 0; }
static inline int set_memory_uc(unsigned long address, int pages) { aeros_flush_address(address, (unsigned long)pages << 12); return 0; }
static inline int set_memory_wb(unsigned long address, int pages) { (void)address; (void)pages; return 0; }
static inline int set_pages_wb(struct page *page, int pages) { (void)page; (void)pages; return 0; }
static inline int set_pages_uc(struct page *page, int pages) { aeros_flush_pages(&page, -pages); return 0; }
static inline int set_pages_array_wc(struct page **pages, int count) { aeros_flush_pages(pages, count); return 0; }
static inline int set_pages_array_uc(struct page **pages, int count) { aeros_flush_pages(pages, count); return 0; }
static inline int set_pages_array_wb(struct page **pages, int count) { (void)pages; (void)count; return 0; }

#endif
