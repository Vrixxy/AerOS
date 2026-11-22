#ifndef _AEROS_LINUX_BACKLIGHT_H
#define _AEROS_LINUX_BACKLIGHT_H

#include <linux/device.h>
#include <linux/fb.h>
#include <linux/slab.h>
#include <linux/err.h>
enum backlight_type { BACKLIGHT_RAW = 1, BACKLIGHT_PLATFORM, BACKLIGHT_FIRMWARE, BACKLIGHT_TYPE_MAX };
enum backlight_update_reason { BACKLIGHT_UPDATE_HOTKEY, BACKLIGHT_UPDATE_SYSFS };
#define BACKLIGHT_POWER_ON 0
#define BACKLIGHT_POWER_OFF 4
#define FB_BLANK_UNBLANK 0
#define FB_BLANK_POWERDOWN 4
#define BL_CORE_SUSPENDED (1 << 0)
#define BL_CORE_FBBLANK (1 << 1)
#define BL_CORE_SUSPENDRESUME (1 << 0)
struct backlight_device;
extern struct backlight_device *aeros_backlight;
struct backlight_ops {
	unsigned int options;
	int (*update_status)(struct backlight_device *);
	int (*get_brightness)(struct backlight_device *);
};
struct backlight_properties {
	int brightness;
	int max_brightness;
	int power;
	enum backlight_type type;
	unsigned int state;
	int scale;
};
struct backlight_device {
	struct backlight_properties props;
	const struct backlight_ops *ops;
	struct device dev;
	void *data;
};
/* No sysfs class: a registered device is just its properties and ops, so
 * drivers that only set the level when one exists (amdgpu DC) still do. */
static inline struct backlight_device *backlight_device_register(const char *name, struct device *dev, void *data,
								  const struct backlight_ops *ops,
								  const struct backlight_properties *props)
{
	struct backlight_device *bd = kzalloc(sizeof(*bd), GFP_KERNEL);

	(void)name;
	if (!bd)
		return ERR_PTR(-ENOMEM);
	bd->dev.parent = dev;
	bd->data = data;
	bd->ops = ops;
	if (props)
		bd->props = *props;
	if (!aeros_backlight)
		aeros_backlight = bd;
	return bd;
}
static inline void backlight_device_unregister(struct backlight_device *bd)
{
	if (IS_ERR_OR_NULL(bd))
		return;
	if (aeros_backlight == bd)
		aeros_backlight = NULL;
	kfree(bd);
}
static inline void *bl_get_data(struct backlight_device *bd) { return bd->data; }
static inline int backlight_update_status(struct backlight_device *bd)
{
	return bd->ops && bd->ops->update_status ? bd->ops->update_status(bd) : 0;
}
static inline bool backlight_is_blank(const struct backlight_device *bd)
{
	return bd->props.power != BACKLIGHT_POWER_ON || (bd->props.state & (BL_CORE_SUSPENDED | BL_CORE_FBBLANK));
}
static inline int backlight_get_brightness(const struct backlight_device *bd)
{
	return backlight_is_blank(bd) ? 0 : bd->props.brightness;
}
static inline int backlight_disable(struct backlight_device *bd)
{
	if (!bd)
		return 0;
	bd->props.power = BACKLIGHT_POWER_OFF;
	return backlight_update_status(bd);
}
static inline int backlight_enable(struct backlight_device *bd)
{
	if (!bd)
		return 0;
	bd->props.power = BACKLIGHT_POWER_ON;
	return backlight_update_status(bd);
}
#endif
