// SPDX-License-Identifier: MIT
/* Stage 1 of the Radeon work: look at an AMD GPU without changing it. Logs
 * what amdgpu would start from: the PCI identity and BARs, the video BIOS
 * the firmware left in the ACPI VFCT table, the memory size NBIO reports
 * and where the firmware's framebuffer sits. Only reads. */
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/string.h>
#include <linux/firmware.h>
#include <aeros/ops.h>

/* NBIO 7.0: RCC_CONFIG_MEMSIZE, segment 2 (0xd20) + 0xc3, in dwords. */
#define RCC_CONFIG_MEMSIZE_BYTE ((0xd20 + 0xc3) * 4)

struct vfct_image {
	u32 bus, device, function;
	u16 vendor_id, device_id, ssvid, ssid;
	u32 revision, length;
} __packed;

static void describe_vbios(struct pci_dev *pdev, const u8 *bios, u32 length)
{
	char message[64];
	u16 header, boot;
	size_t used = 0;

	if (length < 0x4a || bios[0] != 0x55 || bios[1] != 0xaa) {
		dev_info(&pdev->dev, "vbios: no ROM signature\n");
		return;
	}
	header = bios[0x48] | bios[0x49] << 8;
	if (header + 0x12 > length) {
		dev_info(&pdev->dev, "vbios: header out of range\n");
		return;
	}
	boot = bios[header + 0x10] | bios[header + 0x11] << 8;
	for (u32 at = boot; at < length && used + 1 < sizeof(message); at++) {
		u8 c = bios[at];
		if (!c)
			break;
		if (c >= 0x20 && c < 0x7f)
			message[used++] = (char)c;
	}
	message[used] = '\0';
	dev_info(&pdev->dev, "vbios: %u bytes, %.4s table, \"%s\"\n", length,
		 (const char *)bios + header + 4, message);
}

static void find_vbios(struct pci_dev *pdev)
{
	unsigned long length = 0;
	const u8 *vfct = aeros->acpi_table("VFCT", &length);
	u32 offset;

	if (!vfct || length < 0x4c) {
		dev_info(&pdev->dev, "vbios: no VFCT table\n");
		return;
	}
	offset = *(const u32 *)(vfct + 0x34);
	while (offset + sizeof(struct vfct_image) <= length) {
		const struct vfct_image *image = (const void *)(vfct + offset);
		const u8 *content = (const u8 *)(image + 1);

		if (!image->length || offset + sizeof(*image) + image->length > length)
			break;
		if (image->vendor_id == pdev->vendor && image->device_id == pdev->device &&
		    image->bus == pdev->bus->number && image->device == PCI_SLOT(pdev->devfn)) {
			describe_vbios(pdev, content, image->length);
			return;
		}
		offset += sizeof(*image) + image->length;
	}
	dev_info(&pdev->dev, "vbios: VFCT has no image for this GPU\n");
}

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

static int amdgpu_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	unsigned long long fb_base = 0, fb_size = 0;

	(void)id;
	dev_info(&pdev->dev, "AMD GPU %04x:%04x rev %02x subsystem %04x:%04x (read-only probe)\n",
		 pdev->vendor, pdev->device, pdev->revision, pdev->subsystem_vendor,
		 pdev->subsystem_device);
	for (int bar = 0; bar < PCI_NUM_RESOURCES; bar++)
		if (pci_resource_len(pdev, bar))
			dev_info(&pdev->dev, "BAR%d %llx, %llu KiB, %s%s%s\n", bar,
				 (unsigned long long)pci_resource_start(pdev, bar),
				 (unsigned long long)pci_resource_len(pdev, bar) / 1024,
				 pci_resource_flags(pdev, bar) & IORESOURCE_IO ? "I/O" : "memory",
				 pci_resource_flags(pdev, bar) & IORESOURCE_MEM_64 ? ", 64-bit" : "",
				 pci_resource_flags(pdev, bar) & IORESOURCE_PREFETCH ? ", prefetchable" : "");
	describe_express(pdev);
	find_vbios(pdev);
	{
		static const char *const needed[] = {
			"amdgpu/renoir_asd.bin", "amdgpu/renoir_ta.bin", "amdgpu/renoir_pfp.bin",
			"amdgpu/renoir_me.bin", "amdgpu/renoir_ce.bin", "amdgpu/renoir_mec.bin",
			"amdgpu/renoir_rlc.bin", "amdgpu/renoir_sdma.bin",
		};
		for (unsigned int i = 0; i < ARRAY_SIZE(needed); i++) {
			const struct firmware *blob;
			if (!request_firmware(&blob, needed[i], &pdev->dev)) {
				dev_info(&pdev->dev, "firmware %s: %zu bytes\n", needed[i], blob->size);
				release_firmware(blob);
			}
		}
	}
	if (pci_resource_len(pdev, 5) >= RCC_CONFIG_MEMSIZE_BYTE + 4 &&
	    !(pci_resource_flags(pdev, 5) & IORESOURCE_IO)) {
		void __iomem *mmio = pci_iomap(pdev, 5, 0);
		if (mmio)
			dev_info(&pdev->dev, "NBIO memsize %u MiB\n",
				 readl((u8 __iomem *)mmio + RCC_CONFIG_MEMSIZE_BYTE));
	}
	if (aeros->framebuffer(&fb_base, &fb_size))
		dev_info(&pdev->dev, "firmware framebuffer %llx, %llu KiB%s\n", fb_base, fb_size / 1024,
			 fb_base >= pci_resource_start(pdev, 0) &&
					 fb_base < pci_resource_start(pdev, 0) + pci_resource_len(pdev, 0) ?
				 " (inside BAR0)" :
				 "");
	return 0;
}

static const struct pci_device_id amdgpu_probe_ids[] = {
	{ .vendor = PCI_VENDOR_ID_ATI, .device = PCI_ANY_ID, .subvendor = PCI_ANY_ID,
	  .subdevice = PCI_ANY_ID, .class = 0x030000, .class_mask = 0xff0000 },
	{ 0 }
};

static struct pci_driver amdgpu_probe_driver = {
	.name = "aeros-amdgpu-probe",
	.id_table = amdgpu_probe_ids,
	.probe = amdgpu_probe,
};

module_pci_driver(amdgpu_probe_driver);
MODULE_LICENSE("Dual MIT/GPL");
