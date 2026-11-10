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
	snprintf(dev->dev.name, sizeof(dev->dev.name), "0000:%02x:%02x.%u", info->bus, info->slot,
		 info->function);
	return dev;
}

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
