#ifndef _LINUX_MM_H
#define _LINUX_MM_H

#include <linux/kernel.h>
#include <linux/mm_types.h>
#include <linux/slab.h>
#include <linux/overflow.h>
#include <linux/pgtable.h>
#include <linux/shrinker.h>

struct address_space;
static inline void unmap_mapping_range(struct address_space *mapping, loff_t start, loff_t length, int even_cows)
{
	(void)mapping; (void)start; (void)length; (void)even_cows;
}
struct page *alloc_pages(gfp_t gfp, unsigned int order);
void __free_pages(struct page *page, unsigned int order);
struct page *pfn_to_page(unsigned long pfn);
#define alloc_pages_node(node, gfp, order) alloc_pages((gfp), (order))
#define alloc_page(gfp) alloc_pages((gfp), 0)
#define __free_page(page) __free_pages((page), 0)
static inline unsigned long page_to_pfn(const struct page *page) { return (unsigned long)(page->phys >> PAGE_SHIFT); }
static inline phys_addr_t page_to_phys(const struct page *page) { return page->phys; }
static inline void *page_address(const struct page *page) { return (void *)(uintptr_t)page->phys; }
static inline void *lowmem_page_address(const struct page *page) { return page_address(page); }
#define virt_to_page(address) pfn_to_page((unsigned long)((uintptr_t)(address) >> PAGE_SHIFT))
#define vmalloc_to_page virt_to_page
#define nth_page(page, n) ((page) + (n))
#define page_private(page) ((page)->private)
#define set_page_private(page, v) ((page)->private = (unsigned long)(v))
static inline struct page *compound_head(struct page *page) { return page->head ? page->head : page; }
static inline unsigned int compound_order(struct page *page) { return page->head == page ? page->order : 0; }
static inline int page_count(struct page *page) { return atomic_read(&compound_head(page)->_refcount); }
static inline void get_page(struct page *page) { atomic_inc(&compound_head(page)->_refcount); }
static inline void put_page(struct page *page)
{
	struct page *head = compound_head(page);

	if (atomic_dec_and_test(&head->_refcount))
		__free_pages(head, head->order);
}
#define PageHighMem(page) 0
#define PageReserved(page) 0
static inline void free_pages(unsigned long address, unsigned int order)
{
	if (address)
		__free_pages(virt_to_page((void *)address), order);
}
#define free_page(address) free_pages((address), 0)
static inline unsigned long __get_free_pages(gfp_t gfp, unsigned int order)
{
	struct page *page = alloc_pages(gfp, order);

	return page ? (unsigned long)page_address(page) : 0;
}
#define __get_free_page(gfp) __get_free_pages((gfp), 0)
#define get_zeroed_page(gfp) __get_free_pages((gfp) | __GFP_ZERO, 0)
static inline void clear_page(void *page) { memset(page, 0, PAGE_SIZE); }
static inline void copy_page(void *to, void *from) { memcpy(to, from, PAGE_SIZE); }

struct sysinfo {
	unsigned long totalram;
	unsigned long freeram;
	unsigned long totalhigh;
	unsigned int mem_unit;
};
void si_meminfo(struct sysinfo *info);
unsigned long totalram_pages(void);

#define VM_FAULT_OOM 0x1u
#define VM_FAULT_SIGBUS 0x2u
#define VM_FAULT_NOPAGE 0x100u
#define VM_FAULT_RETRY 0x400u
struct vm_area_struct {
	unsigned long vm_start;
	unsigned long vm_end;
	unsigned long vm_pgoff;
	unsigned long vm_flags;
	pgprot_t vm_page_prot;
	void *vm_private_data;
	const struct vm_operations_struct *vm_ops;
	struct file *vm_file;
};
struct vm_fault {
	struct vm_area_struct *vma;
	unsigned long address;
	unsigned int flags;
	pgoff_t pgoff;
};
struct vm_operations_struct {
	void (*open)(struct vm_area_struct *area);
	void (*close)(struct vm_area_struct *area);
	vm_fault_t (*fault)(struct vm_fault *vmf);
	int (*access)(struct vm_area_struct *vma, unsigned long addr, void *buf, int len, int write);
};

#define offset_in_page(p) ((unsigned long)(p) & ~PAGE_MASK)
#define PFN_UP(x) (((x) + PAGE_SIZE - 1) >> PAGE_SHIFT)
#define PFN_DOWN(x) ((x) >> PAGE_SHIFT)
#define PFN_PHYS(x) ((phys_addr_t)(x) << PAGE_SHIFT)
#define PHYS_PFN(x) ((unsigned long)((x) >> PAGE_SHIFT))

/* A folio is a single page here. */
struct folio {
	struct page page;
};
#define page_folio(p) ((struct folio *)compound_head(p))
#define folio_page(folio, n) (&(folio)->page + (n))
#define folio_file_page(folio, index) (&(folio)->page)
static inline unsigned long folio_nr_pages(struct folio *folio) { (void)folio; return 1; }
static inline unsigned long folio_pfn(struct folio *folio) { return page_to_pfn(&folio->page); }
static inline void folio_put(struct folio *folio) { put_page(&folio->page); }
static inline void folio_get(struct folio *folio) { get_page(&folio->page); }
#define folio_mark_dirty(folio) ((void)(folio), true)
#define folio_mark_accessed(folio) do { (void)(folio); } while (0)
#define folio_address(folio) page_address(&(folio)->page)
static inline unsigned long vma_pages(struct vm_area_struct *vma) { return (vma->vm_end - vma->vm_start) >> PAGE_SHIFT; }
#define vm_flags_set(vma, flags) ((vma)->vm_flags |= (flags))
#define vm_flags_clear(vma, flags) ((vma)->vm_flags &= ~(flags))
#define vm_get_page_prot(flags) PAGE_KERNEL
#define VM_READ 0x1ul
#define VM_WRITE 0x2ul
#define VM_EXEC 0x4ul
#define VM_SHARED 0x8ul
#define VM_MAYWRITE 0x20ul
#define VM_IO 0x4000ul
#define VM_DONTCOPY 0x20000ul
#define VM_DONTEXPAND 0x40000ul
#define VM_DONTDUMP 0x4000000ul
#define VM_PFNMAP 0x400ul
#define VM_MIXEDMAP 0x10000000ul
#define VM_NORESERVE 0x200000ul
#define VM_LOCKED 0x2000ul
static inline int get_order(unsigned long size)
{
	return size <= PAGE_SIZE ? 0 : fls64((size - 1) >> PAGE_SHIFT);
}
#define set_page_dirty(page) ((void)(page), 0)
#define set_page_dirty_lock set_page_dirty
#define mark_page_accessed(page) do { (void)(page); } while (0)
#define unpin_user_pages(pages, count) do { (void)(pages); (void)(count); } while (0)
static inline bool want_init_on_free(void) { return false; }
static inline bool want_init_on_alloc(gfp_t gfp) { (void)gfp; return false; }
static inline bool is_vmalloc_addr(const void *address) { (void)address; return false; }
#include <linux/highmem.h>

#endif
