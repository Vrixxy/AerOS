#ifndef _VIDEO_VIDEOMODE_H
#define _VIDEO_VIDEOMODE_H

#include <linux/types.h>

enum display_flags {
	DISPLAY_FLAGS_HSYNC_LOW = 1 << 0,
	DISPLAY_FLAGS_HSYNC_HIGH = 1 << 1,
	DISPLAY_FLAGS_VSYNC_LOW = 1 << 2,
	DISPLAY_FLAGS_VSYNC_HIGH = 1 << 3,
	DISPLAY_FLAGS_DE_LOW = 1 << 4,
	DISPLAY_FLAGS_DE_HIGH = 1 << 5,
	DISPLAY_FLAGS_PIXDATA_POSEDGE = 1 << 6,
	DISPLAY_FLAGS_PIXDATA_NEGEDGE = 1 << 7,
	DISPLAY_FLAGS_INTERLACED = 1 << 8,
	DISPLAY_FLAGS_DOUBLESCAN = 1 << 9,
	DISPLAY_FLAGS_DOUBLECLK = 1 << 10,
	DISPLAY_FLAGS_SYNC_POSEDGE = 1 << 11,
	DISPLAY_FLAGS_SYNC_NEGEDGE = 1 << 12,
};

struct videomode {
	unsigned long pixelclock;
	u32 hactive, hfront_porch, hback_porch, hsync_len;
	u32 vactive, vfront_porch, vback_porch, vsync_len;
	enum display_flags flags;
};

#endif
