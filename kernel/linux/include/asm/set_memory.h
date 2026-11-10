#ifndef _ASM_SET_MEMORY_H
#define _ASM_SET_MEMORY_H

#include <linux/mm_types.h>

/* AerOS keeps RAM write-back. The GPU snoops CPU caches for system memory,
 * so this is coherent, just slower than write-combining for some uses. */
static inline int set_memory_wc(unsigned long address, int pages) { (void)address; (void)pages; return 0; }
static inline int set_memory_uc(unsigned long address, int pages) { (void)address; (void)pages; return 0; }
static inline int set_memory_wb(unsigned long address, int pages) { (void)address; (void)pages; return 0; }
static inline int set_pages_wb(struct page *page, int pages) { (void)page; (void)pages; return 0; }
static inline int set_pages_uc(struct page *page, int pages) { (void)page; (void)pages; return 0; }
static inline int set_pages_array_wc(struct page **pages, int count) { (void)pages; (void)count; return 0; }
static inline int set_pages_array_uc(struct page **pages, int count) { (void)pages; (void)count; return 0; }
static inline int set_pages_array_wb(struct page **pages, int count) { (void)pages; (void)count; return 0; }

#endif
