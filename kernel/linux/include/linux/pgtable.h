#ifndef _LINUX_PGTABLE_H
#define _LINUX_PGTABLE_H

#include <linux/mm_types.h>
#include <asm/processor.h>

#define _PAGE_CACHE_WC 0x8ul
#define _PAGE_CACHE_UC 0x18ul
#define PAGE_KERNEL __pgprot(0x163ul)
#define PAGE_KERNEL_IO PAGE_KERNEL
#define PAGE_KERNEL_NOCACHE __pgprot(0x163ul | _PAGE_CACHE_UC)
static inline pgprot_t pgprot_writecombine(pgprot_t prot) { return __pgprot(pgprot_val(prot) | _PAGE_CACHE_WC); }
static inline pgprot_t pgprot_noncached(pgprot_t prot) { return __pgprot(pgprot_val(prot) | _PAGE_CACHE_UC); }
#define pgprot_decrypted(prot) (prot)
#define pgprot_encrypted(prot) (prot)
#define pgprot_device pgprot_noncached

#endif
