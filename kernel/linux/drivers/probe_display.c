// SPDX-License-Identifier: MIT
/* A first driver written the Linux way (pci_driver, probe, pci_iomap,
 * readl), to show a Linux driver binding through AerOS. */
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/workqueue.h>
#include <linux/dma-mapping.h>
#include <linux/firmware.h>
#include <linux/completion.h>

#define PCI_CAPABILITY_LIST 0x34
#define VIRTIO_PCI_CAP_VENDOR 0x09
#define VIRTIO_PCI_CAP_COMMON_CFG 1

static int probe_virtio(struct pci_dev *pdev)
{
	u8 position;

	pci_read_config_byte(pdev, PCI_CAPABILITY_LIST, &position);
	while (position) {
		u8 id, type, bar;
		u32 offset;

		pci_read_config_byte(pdev, position, &id);
		pci_read_config_byte(pdev, position + 3, &type);
		pci_read_config_byte(pdev, position + 4, &bar);
		pci_read_config_dword(pdev, position + 8, &offset);
		if (id == VIRTIO_PCI_CAP_VENDOR && type == VIRTIO_PCI_CAP_COMMON_CFG) {
			void __iomem *base = pci_iomap(pdev, bar, 0);
			if (!base)
				return -ENOMEM;
			void __iomem *common = (u8 __iomem *)base + offset;
			writel(0, common);
			u32 low = readl((u8 __iomem *)common + 4);
			writel(1, common);
			u32 high = readl((u8 __iomem *)common + 4);
			u16 queues = readw((u8 __iomem *)common + 18);
			dev_info(&pdev->dev, "virtio features %08x%08x, %u queues\n", high, low, queues);
			return 0;
		}
		pci_read_config_byte(pdev, position + 1, &position);
	}
	return -ENODEV;
}

static int probe_bochs(struct pci_dev *pdev)
{
	void __iomem *mmio = pci_iomap(pdev, 2, 0);

	if (!mmio)
		return -ENOMEM;
	writew(0, (u8 __iomem *)mmio + 0x500);
	dev_info(&pdev->dev, "bochs dispi id %04x\n", readw((u8 __iomem *)mmio + 0x500 + 2));
	return 0;
}

static unsigned long probed_at;
static struct completion ticked;
static struct timer_list ticker;

static void tick(struct timer_list *timer)
{
	(void)timer;
	complete(&ticked);
}

static void settled(struct work_struct *work)
{
	(void)work;
	pr_info("aeros-probe-display: delayed work ran %lu ms after probe\n", jiffies - probed_at);
}

static DECLARE_DELAYED_WORK_AEROS(settle_work, settled);

static void describe_express(struct pci_dev *pdev)
{
	int express = pci_find_capability(pdev, PCI_CAP_ID_EXP);
	int used = 0, position = 0x100;
	char list[96];
	u32 header;

	if (express) {
		u16 link;
		pci_read_config_word(pdev, express + PCI_EXP_LNKSTA, &link);
		dev_info(&pdev->dev, "PCIe link gen %u x%u\n", link & 0xf, (link >> 4) & 0x3f);
	}
	list[0] = '\0';
	for (int ttl = 32; ttl; ttl--) {
		pci_read_config_dword(pdev, position, &header);
		if (!header || header == 0xffffffff)
			break;
		used += scnprintf(list + used, sizeof(list) - used, " %x", PCI_EXT_CAP_ID(header));
		position = PCI_EXT_CAP_NEXT(header);
		if (position < 0x100)
			break;
	}
	dev_info(&pdev->dev, "extended capabilities:%s\n", used ? list : " none");
}

static int display_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	int ret;

	(void)id;
	/* AMD GPUs are left to the read-only amdgpu probe. */
	if (pdev->vendor == PCI_VENDOR_ID_ATI)
		return -ENODEV;
	ret = pci_enable_device(pdev);
	if (ret)
		return ret;
	dev_info(&pdev->dev, "display %04x:%04x rev %u, BAR0 %llx (%llu bytes)\n", pdev->vendor,
		 pdev->device, pdev->revision, (unsigned long long)pci_resource_start(pdev, 0),
		 (unsigned long long)pci_resource_len(pdev, 0));
	describe_express(pdev);
	{
		dma_addr_t handle;
		u32 *ring = dma_alloc_coherent(&pdev->dev, 8192, &handle, GFP_KERNEL);
		if (ring) {
			ring[2047] = 0xa3e05c71;
			dev_info(&pdev->dev, "dma buffer at %llx, readback %s\n",
				 (unsigned long long)handle,
				 *(volatile u32 *)(unsigned long)(handle + 2047 * 4) == 0xa3e05c71 ? "ok" : "BAD");
			dma_free_coherent(&pdev->dev, 8192, ring, handle);
		}
	}
	{
		const struct firmware *blob;
		if (!request_firmware(&blob, "aeros/test.bin", &pdev->dev)) {
			dev_info(&pdev->dev, "firmware aeros/test.bin: %zu bytes, \"%.*s\"\n", blob->size,
				 (int)blob->size, (const char *)blob->data);
			release_firmware(blob);
		}
	}
	probed_at = jiffies;
	init_completion(&ticked);
	timer_setup(&ticker, tick, 0);
	mod_timer(&ticker, jiffies + msecs_to_jiffies(40));
	if (wait_for_completion_timeout(&ticked, msecs_to_jiffies(500)))
		dev_info(&pdev->dev, "completion from a timer after %lu ms\n", jiffies - probed_at);
	else
		dev_info(&pdev->dev, "completion timed out\n");
	schedule_delayed_work(&settle_work, msecs_to_jiffies(250));
	if (pdev->vendor == PCI_VENDOR_ID_REDHAT_QUMRANET)
		return probe_virtio(pdev);
	if (pdev->vendor == 0x1234 && pdev->device == 0x1111)
		return probe_bochs(pdev);
	return 0;
}

static const struct pci_device_id display_ids[] = {
	{ PCI_DEVICE_CLASS(0x030000, 0xff0000) },
	{ 0 }
};

static struct pci_driver display_driver = {
	.name = "aeros-probe-display",
	.id_table = display_ids,
	.probe = display_probe,
};

module_pci_driver(display_driver);
MODULE_LICENSE("Dual MIT/GPL");
