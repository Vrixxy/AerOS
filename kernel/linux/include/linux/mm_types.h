#ifndef _LINUX_MM_TYPES_H
#define _LINUX_MM_TYPES_H

#include <linux/types.h>
#include <linux/atomic.h>
#include <linux/kernel.h>
#include <linux/list.h>
#define PAGE_SHIFT 12
#define PAGE_SIZE (1UL << PAGE_SHIFT)
#define PAGE_MASK (~(PAGE_SIZE - 1))
#define PAGE_ALIGN(x) ALIGN(x, PAGE_SIZE)
/* A page of AerOS memory. alloc_pages hands out arrays, so page + i is the
 * next physical page of the same allocation, as in Linux. Memory is
 * identity-mapped, so a page's address is its physical address. */
struct page {
	unsigned long flags;
	unsigned long private;
	struct list_head lru;
	atomic_t _refcount;
	unsigned int order;
	struct page *head;
	phys_addr_t phys;
	unsigned long long token;
	void *mapping;
	pgoff_t index;
};
typedef struct {
	unsigned long pgprot;
} pgprot_t;
#define __pgprot(x) ((pgprot_t){ (x) })
#define pgprot_val(x) ((x).pgprot)
typedef unsigned int vm_fault_t;
struct mm_struct;
struct vm_area_struct;

#endif
