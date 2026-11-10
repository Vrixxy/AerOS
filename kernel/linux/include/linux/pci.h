#ifndef _LINUX_PCI_H
#define _LINUX_PCI_H

#include <linux/types.h>
#include <linux/device.h>
#include <linux/errno.h>
#include <linux/io.h>
#include <linux/mod_devicetable.h>

#define PCI_ANY_ID (~0u)
#define PCI_NUM_RESOURCES 6

#define PCI_COMMAND 0x04
#define PCI_COMMAND_IO 0x1
#define PCI_COMMAND_MEMORY 0x2
#define PCI_COMMAND_MASTER 0x4
#define PCI_COMMAND_INTX_DISABLE 0x400
#define PCI_REVISION_ID 0x08
#define PCI_BASE_ADDRESS_0 0x10


#define PCI_DEVICE(v, d) \
	.vendor = (v), .device = (d), .subvendor = PCI_ANY_ID, .subdevice = PCI_ANY_ID
#define PCI_DEVICE_CLASS(c, m) \
	.vendor = PCI_ANY_ID, .device = PCI_ANY_ID, .subvendor = PCI_ANY_ID, \
	.subdevice = PCI_ANY_ID, .class = (c), .class_mask = (m)
#define PCI_VDEVICE(v, d) \
	.vendor = PCI_VENDOR_ID_##v, .device = (d), .subvendor = PCI_ANY_ID, \
	.subdevice = PCI_ANY_ID, .class = 0, .class_mask = 0

#define PCI_VENDOR_ID_ATI 0x1002
#define PCI_VENDOR_ID_AMD 0x1022
#define PCI_VENDOR_ID_REDHAT_QUMRANET 0x1af4

#include <linux/ioport.h>

struct pci_bus {
	unsigned char number;
};

struct pci_dev {
	struct device dev;
	struct pci_bus *bus;
	struct pci_bus bus_storage;
	unsigned int devfn;
	unsigned short vendor;
	unsigned short device;
	unsigned short subsystem_vendor;
	unsigned short subsystem_device;
	unsigned int class;
	u8 revision;
	unsigned int irq;
	struct resource resource[PCI_NUM_RESOURCES];
	struct pci_driver *driver;
	bool enabled;
};

struct pci_driver {
	const char *name;
	const struct pci_device_id *id_table;
	int (*probe)(struct pci_dev *dev, const struct pci_device_id *id);
	void (*remove)(struct pci_dev *dev);
	void (*shutdown)(struct pci_dev *dev);
	struct device_driver driver;
};

#define PCI_SLOT(devfn) (((devfn) >> 3) & 0x1f)
#define PCI_FUNC(devfn) ((devfn) & 0x07)
#define to_pci_dev(d) container_of(d, struct pci_dev, dev)

int pci_register_driver(struct pci_driver *driver);
void pci_unregister_driver(struct pci_driver *driver);

#define module_pci_driver(drv) \
	static int __init drv##_register(void) { return pci_register_driver(&(drv)); } \
	module_init(drv##_register)

#define PCI_CAP_ID_EXP 0x10
#define PCI_EXP_LNKSTA 0x12
#define PCI_EXT_CAP_ID(header) ((header) & 0xffff)
#define PCI_EXT_CAP_NEXT(header) (((header) >> 20) & 0xffc)

int pci_find_capability(struct pci_dev *dev, int cap);
int pci_find_next_capability(struct pci_dev *dev, u8 position, int cap);
int pci_find_ext_capability(struct pci_dev *dev, int cap);
static inline bool pci_is_pcie(struct pci_dev *dev) { return pci_find_capability(dev, PCI_CAP_ID_EXP) != 0; }

int pci_enable_device(struct pci_dev *dev);
static inline int pcim_enable_device(struct pci_dev *dev) { return pci_enable_device(dev); }
void pci_disable_device(struct pci_dev *dev);
void pci_set_master(struct pci_dev *dev);

int pci_read_config_byte(const struct pci_dev *dev, int where, u8 *value);
int pci_read_config_word(const struct pci_dev *dev, int where, u16 *value);
int pci_read_config_dword(const struct pci_dev *dev, int where, u32 *value);
int pci_write_config_byte(const struct pci_dev *dev, int where, u8 value);
int pci_write_config_word(const struct pci_dev *dev, int where, u16 value);
int pci_write_config_dword(const struct pci_dev *dev, int where, u32 value);

#define pci_resource_start(dev, bar) ((dev)->resource[(bar)].start)
#define pci_resource_end(dev, bar) ((dev)->resource[(bar)].end)
#define pci_resource_flags(dev, bar) ((dev)->resource[(bar)].flags)
#define pci_resource_len(dev, bar) \
	((dev)->resource[(bar)].end ? (dev)->resource[(bar)].end - (dev)->resource[(bar)].start + 1 : 0)

void __iomem *pci_iomap(struct pci_dev *dev, int bar, unsigned long maxlen);
static inline void pci_iounmap(struct pci_dev *dev, void __iomem *address) { (void)dev; (void)address; }
static inline int pci_request_regions(struct pci_dev *dev, const char *name) { (void)dev; (void)name; return 0; }
static inline void pci_release_regions(struct pci_dev *dev) { (void)dev; }

static inline void *pci_get_drvdata(struct pci_dev *dev) { return dev_get_drvdata(&dev->dev); }
static inline void pci_set_drvdata(struct pci_dev *dev, void *data) { dev_set_drvdata(&dev->dev, data); }
static inline const char *pci_name(const struct pci_dev *dev) { return dev_name(&dev->dev); }

static inline int pci_domain_nr(struct pci_bus *bus) { (void)bus; return 0; }

#endif
