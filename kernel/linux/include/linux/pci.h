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

struct pci_dev;
struct pci_bus {
	unsigned char number;
	struct pci_bus *parent;
	struct pci_dev *self;
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
	u8 msix_cap;
	u8 msi_cap;
	u8 pcie_cap;
	u16 ltr_path;
	u64 dma_mask;
	phys_addr_t rom;
	size_t romlen;
	u32 saved_config_space[16];
	unsigned int no_64bit_msi:1;
	unsigned int is_thunderbolt:1;
};

struct pci_driver {
	const char *name;
	const struct pci_device_id *id_table;
	int (*probe)(struct pci_dev *dev, const struct pci_device_id *id);
	void (*remove)(struct pci_dev *dev);
	void (*shutdown)(struct pci_dev *dev);
	const struct attribute_group **dev_groups;
	const struct pci_error_handlers *err_handler;
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

/* Error recovery (AER): never triggered on AerOS, but drivers provide handlers. */
typedef unsigned int pci_channel_state_t;
typedef unsigned int pci_ers_result_t;
enum pci_channel_state {
	pci_channel_io_normal = 1,
	pci_channel_io_frozen = 2,
	pci_channel_io_perm_failure = 3,
};
enum pci_ers_result {
	PCI_ERS_RESULT_NONE = 1,
	PCI_ERS_RESULT_CAN_RECOVER = 2,
	PCI_ERS_RESULT_NEED_RESET = 3,
	PCI_ERS_RESULT_DISCONNECT = 4,
	PCI_ERS_RESULT_RECOVERED = 5,
	PCI_ERS_RESULT_NO_AER_DRIVER = 6,
};
struct pci_error_handlers {
	pci_ers_result_t (*error_detected)(struct pci_dev *dev, pci_channel_state_t error);
	pci_ers_result_t (*mmio_enabled)(struct pci_dev *dev);
	pci_ers_result_t (*slot_reset)(struct pci_dev *dev);
	void (*reset_prepare)(struct pci_dev *dev);
	void (*reset_done)(struct pci_dev *dev);
	void (*resume)(struct pci_dev *dev);
	void (*cor_error_detected)(struct pci_dev *dev);
};
struct pci_saved_state;

typedef int pci_power_t;
#define PCI_D0 ((pci_power_t)0)
#define PCI_D1 ((pci_power_t)1)
#define PCI_D2 ((pci_power_t)2)
#define PCI_D3hot ((pci_power_t)3)
#define PCI_D3cold ((pci_power_t)4)

enum pci_bus_speed {
	PCIE_SPEED_2_5GT = 0x14,
	PCIE_SPEED_5_0GT = 0x15,
	PCIE_SPEED_8_0GT = 0x16,
	PCIE_SPEED_16_0GT = 0x17,
	PCIE_SPEED_32_0GT = 0x18,
	PCIE_SPEED_64_0GT = 0x19,
	PCI_SPEED_UNKNOWN = 0xff,
};
enum pcie_link_width {
	PCIE_LNK_WIDTH_RESRV = 0x00,
	PCIE_LNK_X1 = 0x01,
	PCIE_LNK_X2 = 0x02,
	PCIE_LNK_X4 = 0x04,
	PCIE_LNK_X8 = 0x08,
	PCIE_LNK_X12 = 0x0c,
	PCIE_LNK_X16 = 0x10,
	PCIE_LNK_X32 = 0x20,
	PCIE_LNK_WIDTH_UNKNOWN = 0xff,
};

#define PCI_VENDOR_ID_APPLE 0x106b
#define PCI_VENDOR_ID_DELL 0x1028
#define PCI_VENDOR_ID_ATI 0x1002
#define PCI_BASE_CLASS_DISPLAY 0x03
#define PCI_CLASS_DISPLAY_VGA 0x0300
#define PCI_CLASS_DISPLAY_OTHER 0x0380
#define PCI_CLASS_ACCELERATOR_PROCESSING 0x1200
#define PCI_ROM_RESOURCE 6
#define PCI_PRIMARY_BUS 0x18
#define PCI_POSSIBLE_ERROR(val) ((val) == ((__typeof__(val))~0))
#define PCI_IRQ_INTX (1 << 0)
#define PCI_IRQ_LEGACY PCI_IRQ_INTX
#define PCI_IRQ_MSI (1 << 1)
#define PCI_IRQ_MSIX (1 << 2)
#define PCI_IRQ_ALL_TYPES (PCI_IRQ_INTX | PCI_IRQ_MSI | PCI_IRQ_MSIX)
#define PCI_CAP_ID_MSI 0x05
#define PCI_CAP_ID_MSIX 0x11
#define PCI_MSIX_FLAGS 2
#define PCI_MSIX_FLAGS_QSIZE 0x07ff
#define PCI_MSIX_FLAGS_MASKALL 0x4000
#define PCI_MSIX_FLAGS_ENABLE 0x8000
#define PCI_EXPRESS 0x10
#define PCI_EXP_FLAGS 0x02
#define PCI_EXP_FLAGS_TYPE 0x00f0
#define PCI_EXP_TYPE_ENDPOINT 0x0
#define PCI_EXP_TYPE_ROOT_PORT 0x4
#define PCI_EXP_TYPE_UPSTREAM 0x5
#define PCI_EXP_TYPE_DOWNSTREAM 0x6
#define PCI_EXP_DEVCAP 0x04
#define PCI_EXP_DEVCTL 0x08
#define PCI_EXP_DEVCTL_READRQ 0x7000
#define PCI_EXP_DEVCTL_PAYLOAD 0x00e0
#define PCI_EXP_DEVSTA 0x0a
#define PCI_EXP_DEVSTA_TRPND 0x0020
#define PCI_EXP_LNKCAP 0x0c
#define PCI_EXP_LNKCAP_SLS 0x0000000f
#define PCI_EXP_LNKCAP_MLW 0x000003f0
#define PCI_EXP_LNKCAP_CLKPM 0x00040000
#define PCI_EXP_LNKCTL 0x10
#define PCI_EXP_LNKCTL_HAWD 0x0200
#define PCI_EXP_LNKSTA_CLS 0x000f
#define PCI_EXP_LNKSTA_NLW 0x03f0
#define PCI_EXP_DEVCAP2 0x24
#define PCI_EXP_DEVCAP2_ATOMIC_COMP32 0x00000080
#define PCI_EXP_DEVCAP2_ATOMIC_COMP64 0x00000100
#define PCI_EXP_DEVCTL2 0x28
#define PCI_EXP_DEVCTL2_LTR_EN 0x0400
#define PCI_EXP_LNKCAP2 0x2c
#define PCI_EXP_LNKCTL2 0x30
#define PCI_EXP_LNKCTL2_TLS 0x000f
#define PCI_EXP_LNKCTL2_TLS_2_5GT 0x0001
#define PCI_EXP_LNKCTL2_TLS_5_0GT 0x0002
#define PCI_EXP_LNKCTL2_TLS_8_0GT 0x0003
#define PCI_EXP_LNKCTL2_ENTER_COMP 0x0010
#define PCI_EXP_LNKCTL2_TX_MARGIN 0x0380
#define PCI_EXT_CAP_ID_ERR 0x01
#define PCI_EXT_CAP_ID_VNDR 0x0b
#define PCI_EXT_CAP_ID_REBAR 0x15
#define PCI_EXT_CAP_ID_LTR 0x18
#define PCI_ERR_UNCOR_STATUS 0x04
#define PCI_ERR_COR_STATUS 0x10
#define PCI_LTR_MAX_SNOOP_LAT 0x4

int pcie_capability_read_word(struct pci_dev *dev, int pos, u16 *val);
int pcie_capability_read_dword(struct pci_dev *dev, int pos, u32 *val);
int pcie_capability_write_word(struct pci_dev *dev, int pos, u16 val);
int pcie_capability_clear_and_set_word(struct pci_dev *dev, int pos, u16 clear, u16 set);
#define pcie_capability_set_word(dev, pos, set) pcie_capability_clear_and_set_word(dev, pos, 0, set)
#define pcie_capability_clear_word(dev, pos, clear) pcie_capability_clear_and_set_word(dev, pos, clear, 0)
enum pci_bus_speed pcie_get_speed_cap(struct pci_dev *dev);
enum pcie_link_width pcie_get_width_cap(struct pci_dev *dev);
u32 pcie_bandwidth_available(struct pci_dev *dev, struct pci_dev **limiting_dev, enum pci_bus_speed *speed,
			     enum pcie_link_width *width);
static inline int pci_pcie_type(struct pci_dev *dev)
{
	u16 flags = 0;

	pcie_capability_read_word(dev, PCI_EXP_FLAGS, &flags);
	return (flags & PCI_EXP_FLAGS_TYPE) >> 4;
}
static inline int pcie_get_readrq(struct pci_dev *dev)
{
	u16 ctl = 0;

	pcie_capability_read_word(dev, PCI_EXP_DEVCTL, &ctl);
	return 128 << ((ctl & PCI_EXP_DEVCTL_READRQ) >> 12);
}
int pcie_set_readrq(struct pci_dev *dev, int rq);
static inline int pcie_get_mps(struct pci_dev *dev)
{
	u16 ctl = 0;

	pcie_capability_read_word(dev, PCI_EXP_DEVCTL, &ctl);
	return 128 << ((ctl & PCI_EXP_DEVCTL_PAYLOAD) >> 5);
}
static inline bool pcie_aspm_enabled(struct pci_dev *dev) { (void)dev; return false; }
static inline struct pci_dev *pcie_find_root_port(struct pci_dev *dev) { (void)dev; return NULL; }
static inline struct pci_dev *pci_upstream_bridge(struct pci_dev *dev) { (void)dev; return NULL; }
static inline bool pci_is_root_bus(struct pci_bus *bus) { return !bus->parent; }
static inline bool pci_is_thunderbolt_attached(struct pci_dev *dev) { return dev->is_thunderbolt; }
static inline bool pci_pr3_present(struct pci_dev *dev) { (void)dev; return false; }
static inline bool pci_is_enabled(struct pci_dev *dev) { return dev->enabled; }
static inline bool pci_dev_is_disconnected(const struct pci_dev *dev) { (void)dev; return false; }
static inline bool pci_device_is_present(struct pci_dev *dev)
{
	u32 id = 0;

	return pci_read_config_dword(dev, 0, &id) == 0 && id != 0xffffffff;
}
static inline u16 pci_dev_id(struct pci_dev *dev) { return (u16)((dev->bus->number << 8) | dev->devfn); }
static inline void pci_dev_put(struct pci_dev *dev) { (void)dev; }
static inline struct pci_dev *pci_dev_get(struct pci_dev *dev) { return dev; }
static inline struct pci_dev *pci_get_domain_bus_and_slot(int domain, unsigned int bus, unsigned int devfn)
{
	(void)domain; (void)bus; (void)devfn;
	return NULL;
}
static inline struct pci_dev *pci_get_base_class(unsigned int class, struct pci_dev *from)
{
	(void)class; (void)from;
	return NULL;
}
static inline struct pci_dev *pci_get_class(unsigned int class, struct pci_dev *from)
{
	(void)class; (void)from;
	return NULL;
}
void pci_clear_master(struct pci_dev *dev);
static inline int pci_set_power_state(struct pci_dev *dev, pci_power_t state) { (void)dev; (void)state; return 0; }
static inline int pci_save_state(struct pci_dev *dev) { (void)dev; return 0; }
static inline void pci_restore_state(struct pci_dev *dev) { (void)dev; }
static inline void pci_restore_msi_state(struct pci_dev *dev) { (void)dev; }
static inline struct pci_saved_state *pci_store_saved_state(struct pci_dev *dev) { (void)dev; return NULL; }
static inline int pci_load_saved_state(struct pci_dev *dev, struct pci_saved_state *state) { (void)dev; (void)state; return 0; }
static inline int pci_load_and_free_saved_state(struct pci_dev *dev, struct pci_saved_state **state)
{
	(void)dev;
	*state = NULL;
	return 0;
}
static inline int pci_reset_function(struct pci_dev *dev) { (void)dev; return -ENOTTY; }
static inline int pci_wait_for_pending_transaction(struct pci_dev *dev) { (void)dev; return 1; }
static inline void pci_ignore_hotplug(struct pci_dev *dev) { (void)dev; }
static inline int pci_wake_from_d3(struct pci_dev *dev, bool enable) { (void)dev; (void)enable; return 0; }
static inline int pci_enable_atomic_ops_to_root(struct pci_dev *dev, u32 cap_mask) { (void)dev; (void)cap_mask; return -EINVAL; }
static inline void __iomem *pci_map_rom(struct pci_dev *dev, size_t *size) { (void)dev; *size = 0; return NULL; }
static inline void pci_unmap_rom(struct pci_dev *dev, void __iomem *rom) { (void)dev; (void)rom; }
static inline u32 pci_rebar_get_possible_sizes(struct pci_dev *dev, int bar) { (void)dev; (void)bar; return 0; }
static inline int pci_rebar_bytes_to_size(u64 bytes) { return fls64(bytes >> 20) - 1 > 0 ? fls64(bytes >> 20) - 1 : 0; }
static inline int pci_resize_resource(struct pci_dev *dev, int i, int size, ...) { (void)dev; (void)i; (void)size; return -ENOTSUPP; }
static inline void pci_release_resource(struct pci_dev *dev, int resno) { (void)dev; (void)resno; }
static inline void pci_assign_unassigned_bus_resources(struct pci_bus *bus) { (void)bus; }
#define pci_bus_for_each_resource(bus, res, ...) for ((void)(bus), (res) = NULL; (res); )
int pci_alloc_irq_vectors(struct pci_dev *dev, unsigned int min_vecs, unsigned int max_vecs, unsigned int flags);
static inline void pci_free_irq_vectors(struct pci_dev *dev) { (void)dev; }
static inline int pci_irq_vector(struct pci_dev *dev, unsigned int nr) { return nr ? -EINVAL : (int)dev->irq; }
#define pci_info(pdev, fmt, ...) dev_info(&(pdev)->dev, fmt, ##__VA_ARGS__)
#define pci_warn(pdev, fmt, ...) dev_warn(&(pdev)->dev, fmt, ##__VA_ARGS__)
#define pci_err(pdev, fmt, ...) dev_err(&(pdev)->dev, fmt, ##__VA_ARGS__)
#define pci_dbg(pdev, fmt, ...) dev_dbg(&(pdev)->dev, fmt, ##__VA_ARGS__)

#endif
