// SPDX-License-Identifier: MIT
/* Boot self-test of the unmodified DRM core: a GEM driver's device is
 * allocated, registered and torn down, and a GEM object is created and
 * released through its reference count. */
#include <linux/module.h>
#include <linux/slab.h>
#include <drm/drm_drv.h>
#include <drm/drm_device.h>
#include <drm/drm_gem.h>

static int freed;

static void object_free(struct drm_gem_object *obj)
{
	drm_gem_object_release(obj);
	kfree(obj);
	freed++;
}

static const struct drm_gem_object_funcs object_funcs = {
	.free = object_free,
};

static const struct drm_driver driver = {
	.driver_features = DRIVER_GEM,
	.name = "aeros-test",
	.desc = "AerOS DRM self-test",
	.major = 1,
};

static int __init drm_selftest(void)
{
	static struct device parent;
	struct drm_gem_object *obj;
	struct drm_device *dev;
	int error;

	dev = drm_dev_alloc(&driver, &parent);
	if (IS_ERR(dev)) {
		pr_err("drm selftest: drm_dev_alloc failed (%ld)\n", PTR_ERR(dev));
		return (int)PTR_ERR(dev);
	}
	error = drm_dev_register(dev, 0);
	if (error) {
		pr_err("drm selftest: drm_dev_register failed (%d)\n", error);
		return error;
	}
	obj = kzalloc(sizeof(*obj), GFP_KERNEL);
	obj->funcs = &object_funcs;
	drm_gem_private_object_init(dev, obj, 64 << 10);
	drm_gem_object_get(obj);
	drm_gem_object_put(obj);
	if (freed) {
		pr_err("drm selftest: object freed while still referenced\n");
		return -EINVAL;
	}
	drm_gem_object_put(obj);
	drm_dev_unregister(dev);
	drm_dev_put(dev);
	if (freed != 1) {
		pr_err("drm selftest: object not freed on its last reference\n");
		return -EINVAL;
	}
	pr_info("drm selftest: device %s registered as minor 0, GEM object refcounting ok, device released\n",
		driver.name);
	return 0;
}

module_init(drm_selftest);
