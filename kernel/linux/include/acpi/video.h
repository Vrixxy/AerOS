#ifndef _AEROS_ACPI_VIDEO_H
#define _AEROS_ACPI_VIDEO_H

#include <linux/types.h>
static inline bool acpi_video_backlight_use_native(void) { return true; }
static inline void acpi_video_register_backlight(void) { }
#endif
