#include <linux/module.h>
#include <linux/pci.h>
#include <linux/slab.h>
#include <aeros/ops.h>

#define MAX_DRIVERS 16

static struct pci_driver *drivers[MAX_DRIVERS];
static unsigned int driver_count;
static unsigned int bound_count;

static unsigned int bdf(const struct pci_dev *dev)
{
	return (unsigned int)dev->bus->number << 8 | dev->devfn;
}

static bool matches(const struct pci_device_id *id, const struct pci_dev *dev)
{
	return (id->vendor == PCI_ANY_ID || id->vendor == dev->vendor) &&
	       (id->device == PCI_ANY_ID || id->device == dev->device) &&
	       (id->subvendor == PCI_ANY_ID || id->subvendor == dev->subsystem_vendor) &&
	       (id->subdevice == PCI_ANY_ID || id->subdevice == dev->subsystem_device) &&
	       !((id->class ^ dev->class) & id->class_mask);
}

static const struct pci_device_id *match_table(const struct pci_driver *driver,
					       const struct pci_dev *dev)
{
	for (const struct pci_device_id *id = driver->id_table; id && (id->vendor || id->subvendor || id->class_mask); id++)
		if (matches(id, dev))
			return id;
	return 0;
}

/* The firmware's copy of a GPU's video BIOS (ACPI VFCT), which Linux
 * drivers read as the device's platform ROM. */
struct vfct_image {
	u32 bus, device, function;
	u16 vendor_id, device_id, ssvid, ssid;
	u32 revision, length;
} __packed;

static void attach_platform_rom(struct pci_dev *dev)
{
	unsigned long length = 0;
	const u8 *vfct = aeros->acpi_table("VFCT", &length);
	u32 offset;

	if (!vfct || length < 0x4c)
		return;
	offset = *(const u32 *)(vfct + 0x34);
	while (offset + sizeof(struct vfct_image) <= length) {
		const struct vfct_image *image = (const void *)(vfct + offset);

		if (!image->length || offset + sizeof(*image) + image->length > length)
			return;
		if (image->vendor_id == dev->vendor && image->device_id == dev->device &&
		    image->bus == dev->bus->number && image->device == PCI_SLOT(dev->devfn) &&
		    image->function == PCI_FUNC(dev->devfn)) {
			dev->rom = virt_to_phys(image + 1);
			dev->romlen = image->length;
			return;
		}
		offset += sizeof(*image) + image->length;
	}
}

static struct pci_dev *make_device(const struct aeros_pci_info *info)
{
	struct pci_dev *dev = kzalloc(sizeof(*dev), GFP_KERNEL);

	if (!dev)
		return 0;
	dev->bus = &dev->bus_storage;
	dev->bus->number = info->bus;
	dev->devfn = (unsigned int)info->slot << 3 | info->function;
	dev->vendor = info->vendor;
	dev->device = info->device;
	dev->subsystem_vendor = info->subsystem_vendor;
	dev->subsystem_device = info->subsystem_device;
	dev->class = info->class_code;
	dev->revision = info->revision;
	dev->irq = info->irq_line;
	for (int bar = 0; bar < PCI_NUM_RESOURCES; bar++) {
		struct resource *resource = &dev->resource[bar];
		if (!info->bars[bar] || !info->bar_sizes[bar])
			continue;
		resource->start = info->bars[bar];
		resource->end = info->bars[bar] + info->bar_sizes[bar] - 1;
		resource->flags = info->bar_flags[bar] & 1 ? IORESOURCE_IO : IORESOURCE_MEM;
		if (info->bar_flags[bar] & 2)
			resource->flags |= IORESOURCE_MEM_64;
		if (info->bar_flags[bar] & 4)
			resource->flags |= IORESOURCE_PREFETCH;
	}
	dev->dev.is_pci = true;
	if ((dev->class >> 16) == PCI_BASE_CLASS_DISPLAY)
		attach_platform_rom(dev);
	snprintf(dev->dev.name, sizeof(dev->dev.name), "0000:%02x:%02x.%u", info->bus, info->slot,
		 info->function);
	return dev;
}

bool aeros_amdgpu_enabled;

int pci_register_driver(struct pci_driver *driver)
{
	struct aeros_pci_info info;

	if (driver_count == MAX_DRIVERS)
		return -ENOSPC;
	drivers[driver_count++] = driver;
	for (unsigned long index = 0; index < aeros->pci_count(); index++) {
		if (!aeros->pci_get(index, &info))
			continue;
		struct pci_dev *dev = make_device(&info);
		if (!dev)
			return -ENOMEM;
		const struct pci_device_id *id = match_table(driver, dev);
		if (!id) {
			kfree(dev);
			continue;
		}
		/* The real GPU driver drives the hardware: only when switched on. */
		if (!strcmp(driver->name, "amdgpu") && !aeros_amdgpu_enabled) {
			dev_info(&dev->dev, "amdgpu matches this GPU but is switched off; not binding\n");
			kfree(dev);
			continue;
		}
		dev->driver = driver;
		int result = driver->probe(dev, id);
		if (result) {
			dev_info(&dev->dev, "%s probe failed (%d)\n", driver->name, result);
			kfree(dev);
			continue;
		}
		bound_count++;
	}
	return 0;
}

void pci_unregister_driver(struct pci_driver *driver)
{
	(void)driver;
}

void aeros_pci_report(struct aeros_linux_report *report)
{
	report->drivers = driver_count;
	report->bound = bound_count;
}

int pci_read_config_byte(const struct pci_dev *dev, int where, u8 *value)
{
	*value = (u8)aeros->pci_read(bdf(dev), (unsigned int)where, 1);
	return 0;
}

int pci_read_config_word(const struct pci_dev *dev, int where, u16 *value)
{
	*value = (u16)aeros->pci_read(bdf(dev), (unsigned int)where, 2);
	return 0;
}

int pci_read_config_dword(const struct pci_dev *dev, int where, u32 *value)
{
	*value = aeros->pci_read(bdf(dev), (unsigned int)where, 4);
	return 0;
}

int pci_write_config_byte(const struct pci_dev *dev, int where, u8 value)
{
	aeros->pci_write(bdf(dev), (unsigned int)where, 1, value);
	return 0;
}

int pci_write_config_word(const struct pci_dev *dev, int where, u16 value)
{
	aeros->pci_write(bdf(dev), (unsigned int)where, 2, value);
	return 0;
}

int pci_write_config_dword(const struct pci_dev *dev, int where, u32 value)
{
	aeros->pci_write(bdf(dev), (unsigned int)where, 4, value);
	return 0;
}

int pci_enable_device(struct pci_dev *dev)
{
	u16 command;

	pci_read_config_word(dev, PCI_COMMAND, &command);
	command |= PCI_COMMAND_MEMORY | PCI_COMMAND_IO;
	pci_write_config_word(dev, PCI_COMMAND, command);
	dev->enabled = true;
	return 0;
}

void pci_disable_device(struct pci_dev *dev)
{
	dev->enabled = false;
}

void pci_set_master(struct pci_dev *dev)
{
	u16 command;

	pci_read_config_word(dev, PCI_COMMAND, &command);
	pci_write_config_word(dev, PCI_COMMAND, command | PCI_COMMAND_MASTER);
}

void __iomem *pci_iomap(struct pci_dev *dev, int bar, unsigned long maxlen)
{
	resource_size_t length = pci_resource_len(dev, bar);

	if (!length || pci_resource_flags(dev, bar) & IORESOURCE_IO)
		return 0;
	if (maxlen && maxlen < length)
		length = maxlen;
	return ioremap(pci_resource_start(dev, bar), length);
}

int pci_find_next_capability(struct pci_dev *dev, u8 position, int cap)
{
	u8 id;

	pci_read_config_byte(dev, position + 1, &position);
	for (int ttl = 48; position >= 0x40 && ttl; ttl--) {
		position &= ~3;
		pci_read_config_byte(dev, position, &id);
		if (id == 0xff)
			break;
		if (id == cap)
			return position;
		pci_read_config_byte(dev, position + 1, &position);
	}
	return 0;
}

int pci_find_capability(struct pci_dev *dev, int cap)
{
	u16 status;

	pci_read_config_word(dev, 0x06, &status);
	if (!(status & 0x10))
		return 0;
	return pci_find_next_capability(dev, 0x34 - 1, cap);
}

int pci_find_ext_capability(struct pci_dev *dev, int cap)
{
	int position = 0x100;
	u32 header;

	for (int ttl = (4096 - 256) / 8; ttl; ttl--) {
		pci_read_config_dword(dev, position, &header);
		if (header == 0 || header == 0xffffffff)
			return 0;
		if (PCI_EXT_CAP_ID(header) == cap)
			return position;
		position = PCI_EXT_CAP_NEXT(header);
		if (position < 0x100)
			return 0;
	}
	return 0;
}

static int pcie_cap(struct pci_dev *dev)
{
	if (!dev->pcie_cap)
		dev->pcie_cap = (u8)pci_find_capability(dev, PCI_CAP_ID_EXP);
	return dev->pcie_cap;
}

int pcie_capability_read_word(struct pci_dev *dev, int pos, u16 *val)
{
	int cap = pcie_cap(dev);

	*val = 0;
	return cap ? pci_read_config_word(dev, cap + pos, val) : -EINVAL;
}

int pcie_capability_read_dword(struct pci_dev *dev, int pos, u32 *val)
{
	int cap = pcie_cap(dev);

	*val = 0;
	return cap ? pci_read_config_dword(dev, cap + pos, val) : -EINVAL;
}

int pcie_capability_write_word(struct pci_dev *dev, int pos, u16 val)
{
	int cap = pcie_cap(dev);

	return cap ? pci_write_config_word(dev, cap + pos, val) : -EINVAL;
}

int pcie_capability_clear_and_set_word(struct pci_dev *dev, int pos, u16 clear, u16 set)
{
	u16 val;
	int ret = pcie_capability_read_word(dev, pos, &val);

	if (ret)
		return ret;
	return pcie_capability_write_word(dev, pos, (u16)((val & ~clear) | set));
}

int pcie_set_readrq(struct pci_dev *dev, int rq)
{
	u16 v = (u16)((ffs(rq) - 8) << 12);

	if (rq < 128 || rq > 4096 || (rq & (rq - 1)))
		return -EINVAL;
	return pcie_capability_clear_and_set_word(dev, PCI_EXP_DEVCTL, PCI_EXP_DEVCTL_READRQ, v);
}

enum pci_bus_speed pcie_get_speed_cap(struct pci_dev *dev)
{
	static const enum pci_bus_speed speeds[] = { PCI_SPEED_UNKNOWN, PCIE_SPEED_2_5GT, PCIE_SPEED_5_0GT,
						     PCIE_SPEED_8_0GT, PCIE_SPEED_16_0GT, PCIE_SPEED_32_0GT,
						     PCIE_SPEED_64_0GT };
	u32 cap = 0;

	pcie_capability_read_dword(dev, PCI_EXP_LNKCAP, &cap);
	cap &= PCI_EXP_LNKCAP_SLS;
	return cap < ARRAY_SIZE(speeds) ? speeds[cap] : PCI_SPEED_UNKNOWN;
}

enum pcie_link_width pcie_get_width_cap(struct pci_dev *dev)
{
	u32 cap = 0;

	pcie_capability_read_dword(dev, PCI_EXP_LNKCAP, &cap);
	return cap ? (enum pcie_link_width)((cap & PCI_EXP_LNKCAP_MLW) >> 4) : PCIE_LNK_WIDTH_UNKNOWN;
}

u32 pcie_bandwidth_available(struct pci_dev *dev, struct pci_dev **limiting_dev, enum pci_bus_speed *speed,
			     enum pcie_link_width *width)
{
	static const u32 mbps[] = { 0, 2000, 4000, 7877, 15754, 31508, 63015 };
	u16 status = 0;
	unsigned int gen, lanes;

	pcie_capability_read_word(dev, 0x12, &status);
	gen = status & PCI_EXP_LNKSTA_CLS;
	lanes = (status & PCI_EXP_LNKSTA_NLW) >> 4;
	if (limiting_dev)
		*limiting_dev = dev;
	if (speed)
		*speed = gen && gen < 7 ? (enum pci_bus_speed)(PCIE_SPEED_2_5GT + gen - 1) : PCI_SPEED_UNKNOWN;
	if (width)
		*width = lanes ? (enum pcie_link_width)lanes : PCIE_LNK_WIDTH_UNKNOWN;
	return gen < ARRAY_SIZE(mbps) ? mbps[gen] * lanes : 0;
}

void pci_clear_master(struct pci_dev *dev)
{
	u16 command;

	pci_read_config_word(dev, PCI_COMMAND, &command);
	pci_write_config_word(dev, PCI_COMMAND, command & (u16)~PCI_COMMAND_MASTER);
}

int pci_alloc_irq_vectors(struct pci_dev *dev, unsigned int min_vecs, unsigned int max_vecs, unsigned int flags)
{
	(void)max_vecs;
	if (min_vecs > 1 || !(flags & PCI_IRQ_INTX))
		return -ENOSPC;
	return 1;
}
