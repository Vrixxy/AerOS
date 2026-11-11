#ifndef _AEROS_LINUX_EFI_H
#define _AEROS_LINUX_EFI_H

#include <linux/types.h>
#define EFI_BOOT 0
#define EFI_MEMMAP 4
static inline bool efi_enabled(int feature) { (void)feature; return true; }
#endif
