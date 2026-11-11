// SPDX-License-Identifier: MIT
/* Small pieces amdgpu calls whose upstream files are GPL-only or not built. */
#include <linux/kernel.h>
#include <linux/pci.h>
#include <drm/drm_gem.h>
#include <drm/drm_drv.h>
#include <drm/drm_aperture.h>
#include <drm/ttm/ttm_bo.h>
#include <drm/drm_gem_ttm_helper.h>

struct amdgpu_device;

/* drm_gem_ttm_helper.c is GPL: TTM-backed GEM objects map through TTM. */
int drm_gem_ttm_vmap(struct drm_gem_object *gem, struct iosys_map *map)
{
	return ttm_bo_vmap(container_of(gem, struct ttm_buffer_object, base), map);
}

void drm_gem_ttm_vunmap(struct drm_gem_object *gem, struct iosys_map *map)
{
	ttm_bo_vunmap(container_of(gem, struct ttm_buffer_object, base), map);
}

int drm_gem_ttm_mmap(struct drm_gem_object *gem, struct vm_area_struct *vma)
{
	int ret = ttm_bo_mmap_obj(vma, container_of(gem, struct ttm_buffer_object, base));

	if (ret < 0)
		return ret;
	drm_gem_object_put(gem);
	return 0;
}

/* AerOS keeps drawing to its own framebuffer: there is nothing to evict. */
int drm_aperture_remove_conflicting_pci_framebuffers(struct pci_dev *pdev, const struct drm_driver *req_driver)
{
	(void)pdev;
	(void)req_driver;
	return 0;
}

/* Cyan Skillfish's register table (a console APU) is GPL-tagged upstream. */
int cyan_skillfish_reg_base_init(struct amdgpu_device *adev)
{
	(void)adev;
	pr_warn("amdgpu: Cyan Skillfish register offsets are not included in AerOS\n");
	return -ENODEV;
}

/* Compute partitions (MI300 XCP) register extra DRM devices on platform
 * devices, which AerOS does not have. */
int amdgpu_xcp_drm_dev_alloc(struct drm_device **ddev)
{
	*ddev = NULL;
	return -ENODEV;
}

void amdgpu_xcp_drv_release(void)
{
}

void amdgpu_xcp_drm_dev_free(struct drm_device *ddev)
{
	(void)ddev;
}
