#ifndef _AEROS_LINUX_ACPI_AMD_WBRF_H
#define _AEROS_LINUX_ACPI_AMD_WBRF_H

#include <linux/device.h>
#include <linux/notifier.h>
/* No ACPI wifi band RF-interference mitigation. */
#define MAX_NUM_OF_WBRF_RANGES 11
struct freq_band_range {
	u64 start;
	u64 end;
};
struct wbrf_ranges_in_out {
	u64 num_of_ranges;
	struct freq_band_range band_list[MAX_NUM_OF_WBRF_RANGES];
};
enum wbrf_notifier_actions { WBRF_CHANGED };
static inline bool acpi_amd_wbrf_supported_consumer(struct device *dev) { (void)dev; return false; }
static inline int amd_wbrf_retrieve_freq_band(struct device *dev, struct wbrf_ranges_in_out *out)
{
	(void)dev; (void)out;
	return -ENODEV;
}
static inline int amd_wbrf_register_notifier(struct notifier_block *nb) { (void)nb; return -ENODEV; }
static inline int amd_wbrf_unregister_notifier(struct notifier_block *nb) { (void)nb; return -ENODEV; }
#endif
