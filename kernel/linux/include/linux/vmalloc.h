#ifndef _LINUX_VMALLOC_H
#define _LINUX_VMALLOC_H

#include <linux/mm.h>

#define VM_MAP 0x4ul
#define VM_IOREMAP 0x1ul
void *vmap(struct page **pages, unsigned int count, unsigned long flags, pgprot_t prot);
void vunmap(const void *address);

#endif
