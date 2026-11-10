#ifndef _LINUX_HIGHMEM_H
#define _LINUX_HIGHMEM_H

#include <linux/mm.h>

/* No high memory: every page is always mapped. */
#define kmap(page) page_address(page)
#define kunmap(page) do { (void)(page); } while (0)
#define kmap_local_page(page) page_address(page)
#define kmap_local_page_prot(page, prot) page_address(page)
#define kmap_atomic(page) page_address(page)
#define kmap_local_pfn(pfn) ((void *)((uintptr_t)(pfn) << PAGE_SHIFT))
#define kunmap_local(address) do { (void)(address); } while (0)
#define kunmap_atomic(address) do { (void)(address); } while (0)
static inline void clear_highpage(struct page *page) { clear_page(page_address(page)); }
static inline void copy_highpage(struct page *to, struct page *from) { copy_page(page_address(to), page_address(from)); }
static inline void memcpy_to_page(struct page *page, size_t offset, const char *from, size_t length)
{
	memcpy((char *)page_address(page) + offset, from, length);
}
static inline void memcpy_from_page(char *to, struct page *page, size_t offset, size_t length)
{
	memcpy(to, (char *)page_address(page) + offset, length);
}

#endif
