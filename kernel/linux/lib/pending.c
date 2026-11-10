// SPDX-License-Identifier: MIT
/* Symbols whose upstream file is not compiled in yet: mostly the KMS
 * (display) half of DRM. Each does the harmless minimum; delete it here
 * once its real source joins DRM_SOURCES. drm_sysfs.c is GPL-only, so the
 * sysfs ones stay AerOS's own (there is no sysfs). */
#include <linux/kernel.h>
#include <linux/sync_file.h>
#include <linux/dma-buf.h>
#include <drm/drm_device.h>
#include <drm/drm_file.h>
#include <drm/drm_prime.h>
#include <drm/drm_internal.h>
#include <drm/drm_crtc_internal.h>

struct sync_file *sync_file_create(struct dma_fence *fence)
{
	pr_warn("sync_file_create: fences can't become file descriptors yet\n");
	return NULL;
}

struct dma_fence *sync_file_get_fence(int fd)
{
	return NULL;
}

void dma_buf_put(struct dma_buf *dmabuf)
{
	pr_warn("dma_buf_put: buffer sharing is not built yet\n");
}

/* drm_prime.c */
void drm_prime_init_file_private(struct drm_prime_file_private *prime_fpriv)
{
	mutex_init(&prime_fpriv->lock);
	prime_fpriv->dmabufs = RB_ROOT;
	prime_fpriv->handles = RB_ROOT;
}

void drm_prime_destroy_file_private(struct drm_prime_file_private *prime_fpriv)
{
}

void drm_prime_remove_buf_handle(struct drm_prime_file_private *prime_fpriv, uint32_t handle)
{
}

/* drm_client_event.c */
void drm_client_dev_restore(struct drm_device *dev)
{
}

void drm_client_dev_unregister(struct drm_device *dev)
{
}

/* drm_connector.c, drm_framebuffer.c, drm_mode_config.c, drm_property.c */
void drm_connector_ida_init(void)
{
}

void drm_connector_ida_destroy(void)
{
}

void drm_fb_release(struct drm_file *file_priv)
{
}

void drm_mode_config_validate(struct drm_device *dev)
{
}

int drm_modeset_register_all(struct drm_device *dev)
{
	return 0;
}

void drm_modeset_unregister_all(struct drm_device *dev)
{
}

void drm_property_destroy_user_blobs(struct drm_device *dev, struct drm_file *file_priv)
{
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
