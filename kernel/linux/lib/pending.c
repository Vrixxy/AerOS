// SPDX-License-Identifier: MIT
/* Symbols whose upstream file is not compiled in. Each does the harmless
 * minimum; delete it here once its real source joins DRM_SOURCES.
 * drm_sysfs.c and drm_writeback.c are GPL-only, so those stay AerOS's own. */
#include <linux/kernel.h>
#include <linux/sync_file.h>
#include <linux/dma-buf.h>
#include <drm/drm_device.h>
#include <drm/drm_file.h>
#include <drm/drm_prime.h>
#include <drm/drm_internal.h>
#include <drm/drm_crtc_internal.h>
#include <drm/drm_writeback.h>
#include <drm/drm_utils.h>

struct sync_file *sync_file_create(struct dma_fence *fence)
{
	pr_warn("sync_file_create: fences can't become file descriptors yet\n");
	return NULL;
}

struct dma_fence *sync_file_get_fence(int fd)
{
	return NULL;
}

/* drm_sysfs.c (GPL; AerOS has no sysfs) */
int drm_sysfs_init(void)
{
	return 0;
}

void drm_sysfs_destroy(void)
{
}

struct device *drm_sysfs_minor_alloc(struct drm_minor *minor)
{
	struct device *kdev = kzalloc(sizeof(*kdev), GFP_KERNEL);

	return kdev ? kdev : ERR_PTR(-ENOMEM);
}

void drm_sysfs_hotplug_event(struct drm_device *dev)
{
}

void drm_sysfs_connector_hotplug_event(struct drm_connector *connector)
{
}

void drm_sysfs_connector_property_event(struct drm_connector *connector, struct drm_property *property)
{
}

int drm_sysfs_connector_add(struct drm_connector *connector)
{
	return 0;
}

int drm_sysfs_connector_add_late(struct drm_connector *connector)
{
	return 0;
}

void drm_sysfs_connector_remove_early(struct drm_connector *connector)
{
}

void drm_sysfs_connector_remove(struct drm_connector *connector)
{
}

/* drm_writeback.c is GPL-only; AerOS exposes no writeback connectors. */
int drm_writeback_set_fb(struct drm_connector_state *conn_state, struct drm_framebuffer *fb)
{
	return fb ? -EINVAL : 0;
}

void drm_writeback_cleanup_job(struct drm_writeback_job *job)
{
}

struct dma_fence *drm_writeback_get_out_fence(struct drm_writeback_connector *wb_connector)
{
	return NULL;
}

/* drm_panel_orientation_quirks.c needs DMI tables AerOS does not read yet. */
int drm_get_panel_orientation_quirk(int width, int height)
{
	return -ENODATA;
}

int drm_writeback_prepare_job(struct drm_writeback_job *job)
{
	return 0;
}
