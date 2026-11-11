#ifndef _AEROS_LINUX_VGAARB_H
#define _AEROS_LINUX_VGAARB_H

#include <linux/pci.h>
/* AerOS owns the legacy VGA ranges; drivers are told arbitration succeeded. */
#define VGA_RSRC_NONE 0x00
#define VGA_RSRC_LEGACY_IO 0x01
#define VGA_RSRC_LEGACY_MEM 0x02
#define VGA_RSRC_LEGACY_MASK (VGA_RSRC_LEGACY_IO | VGA_RSRC_LEGACY_MEM)
#define VGA_RSRC_NORMAL_IO 0x04
#define VGA_RSRC_NORMAL_MEM 0x08
static inline int vga_client_register(struct pci_dev *pdev, unsigned int (*set_decode)(struct pci_dev *pdev, bool state))
{
	(void)pdev; (void)set_decode;
	return 0;
}
static inline void vga_client_unregister(struct pci_dev *pdev) { (void)pdev; }
static inline int vga_get_uninterruptible(struct pci_dev *pdev, unsigned int rsrc) { (void)pdev; (void)rsrc; return 0; }
static inline int vga_get_interruptible(struct pci_dev *pdev, unsigned int rsrc) { (void)pdev; (void)rsrc; return 0; }
static inline void vga_put(struct pci_dev *pdev, unsigned int rsrc) { (void)pdev; (void)rsrc; }
static inline void vga_set_legacy_decoding(struct pci_dev *pdev, unsigned int decodes) { (void)pdev; (void)decodes; }
static inline struct pci_dev *vga_default_device(void) { return NULL; }
#endif
