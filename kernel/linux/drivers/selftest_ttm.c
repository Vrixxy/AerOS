// SPDX-License-Identifier: MIT
/* Boot self-test of the unmodified TTM memory manager (amdgpu's buffer
 * objects): a device with a pretend 4 MiB "VRAM" carved from RAM, a buffer
 * filled in system memory, moved to VRAM and back with TTM's own memcpy
 * move, and checked byte for byte after each move. */
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/device.h>
#include <linux/mm.h>
#include <drm/ttm/ttm_bo.h>
#include <drm/ttm/ttm_device.h>
#include <drm/ttm/ttm_placement.h>
#include <drm/ttm/ttm_range_manager.h>
#include <drm/ttm/ttm_tt.h>
#include <drm/drm_vma_manager.h>

#define VRAM_ORDER 10
#define VRAM_PAGES (1ul << VRAM_ORDER)
#define BO_PAGES 37

static struct ttm_device bdev;
static struct device device;
static struct page *vram;
static struct drm_vma_offset_manager offsets;

static struct ttm_tt *tt_create(struct ttm_buffer_object *bo, uint32_t page_flags)
{
	struct ttm_tt *tt = kzalloc(sizeof(*tt), GFP_KERNEL);

	if (tt && ttm_tt_init(tt, bo, page_flags, ttm_cached, 0)) {
		kfree(tt);
		return NULL;
	}
	return tt;
}

static void tt_destroy(struct ttm_device *device, struct ttm_tt *tt)
{
	ttm_tt_fini(tt);
	kfree(tt);
}

static const struct ttm_place system_place = { .mem_type = TTM_PL_SYSTEM };
static const struct ttm_place vram_place = { .mem_type = TTM_PL_VRAM };
static struct ttm_placement system_placement = { 1, &system_place };
static struct ttm_placement vram_placement = { 1, &vram_place };

static void evict_flags(struct ttm_buffer_object *bo, struct ttm_placement *placement)
{
	*placement = system_placement;
}

static int move(struct ttm_buffer_object *bo, bool evict, struct ttm_operation_ctx *ctx,
		struct ttm_resource *new_mem, struct ttm_place *hop)
{
	if (!bo->resource || (bo->resource->mem_type == TTM_PL_SYSTEM && !bo->ttm)) {
		ttm_bo_move_null(bo, new_mem);
		return 0;
	}
	return ttm_bo_move_memcpy(bo, ctx, new_mem);
}

static int io_mem_reserve(struct ttm_device *device, struct ttm_resource *mem)
{
	if (mem->mem_type != TTM_PL_VRAM)
		return mem->mem_type == TTM_PL_SYSTEM ? 0 : -EINVAL;
	mem->bus.addr = (char *)page_address(vram) + (mem->start << PAGE_SHIFT);
	mem->bus.offset = page_to_phys(vram) + (mem->start << PAGE_SHIFT);
	mem->bus.is_iomem = true;
	mem->bus.caching = ttm_cached;
	return 0;
}

static const struct ttm_device_funcs funcs = {
	.ttm_tt_create = tt_create,
	.ttm_tt_destroy = tt_destroy,
	.eviction_valuable = ttm_bo_eviction_valuable,
	.evict_flags = evict_flags,
	.move = move,
	.io_mem_reserve = io_mem_reserve,
};

static void destroy(struct ttm_buffer_object *bo)
{
	kfree(bo);
}

static u32 pattern(unsigned long word) { return (u32)(word * 2654435761u) ^ 0xa5e05u; }

static bool check(struct ttm_buffer_object *bo, const char *where)
{
	struct ttm_bo_kmap_obj map;
	bool is_iomem;
	u32 *words;
	unsigned long i;
	int error = ttm_bo_kmap(bo, 0, BO_PAGES, &map);

	if (error) {
		pr_err("ttm selftest: kmap in %s failed (%d)\n", where, error);
		return false;
	}
	words = ttm_kmap_obj_virtual(&map, &is_iomem);
	for (i = 0; i < BO_PAGES * PAGE_SIZE / 4; i++) {
		if (words[i] != pattern(i)) {
			pr_err("ttm selftest: word %lu wrong in %s\n", i, where);
			ttm_bo_kunmap(&map);
			return false;
		}
	}
	ttm_bo_kunmap(&map);
	return true;
}

static int __init ttm_selftest(void)
{
	struct ttm_operation_ctx ctx = { .interruptible = false };
	struct ttm_buffer_object *bo;
	struct ttm_bo_kmap_obj map;
	unsigned long i, vram_offset;
	bool is_iomem;
	u32 *words;
	int error;

	vram = alloc_pages(GFP_KERNEL, VRAM_ORDER);
	if (!vram)
		return -ENOMEM;
	drm_vma_offset_manager_init(&offsets, 1, 1ul << 20);
	error = ttm_device_init(&bdev, &funcs, &device, NULL, &offsets, false, false);
	if (!error)
		error = ttm_range_man_init(&bdev, TTM_PL_VRAM, false, VRAM_PAGES);
	if (error) {
		pr_err("ttm selftest: device setup failed (%d)\n", error);
		return error;
	}

	bo = kzalloc(sizeof(*bo), GFP_KERNEL);
	bo->base.size = BO_PAGES * PAGE_SIZE;
	error = ttm_bo_init_validate(&bdev, bo, ttm_bo_type_kernel, &system_placement, 0, false, NULL, NULL,
				     destroy);
	if (error) {
		pr_err("ttm selftest: buffer creation failed (%d)\n", error);
		return error;
	}
	ttm_bo_reserve(bo, false, false, NULL);
	error = ttm_bo_kmap(bo, 0, BO_PAGES, &map);
	if (error) {
		pr_err("ttm selftest: first kmap failed (%d)\n", error);
		return error;
	}
	words = ttm_kmap_obj_virtual(&map, &is_iomem);
	for (i = 0; i < BO_PAGES * PAGE_SIZE / 4; i++)
		words[i] = pattern(i);
	ttm_bo_kunmap(&map);

	error = ttm_bo_validate(bo, &vram_placement, &ctx);
	if (error || bo->resource->mem_type != TTM_PL_VRAM) {
		pr_err("ttm selftest: move to VRAM failed (%d)\n", error);
		return error ? error : -EINVAL;
	}
	vram_offset = bo->resource->start << PAGE_SHIFT;
	words = (u32 *)((char *)page_address(vram) + vram_offset);
	for (i = 0; i < BO_PAGES * PAGE_SIZE / 4; i++) {
		if (words[i] != pattern(i)) {
			pr_err("ttm selftest: VRAM word %lu wrong after the move\n", i);
			return -EINVAL;
		}
	}
	if (!check(bo, "VRAM"))
		return -EINVAL;

	error = ttm_bo_validate(bo, &system_placement, &ctx);
	if (error || bo->resource->mem_type != TTM_PL_SYSTEM || !check(bo, "system memory after VRAM")) {
		pr_err("ttm selftest: move back to system memory failed (%d)\n", error);
		return error ? error : -EINVAL;
	}
	ttm_bo_unreserve(bo);
	ttm_bo_put(bo);
	ttm_range_man_fini(&bdev, TTM_PL_VRAM);
	ttm_device_fini(&bdev);
	drm_vma_offset_manager_destroy(&offsets);
	__free_pages(vram, VRAM_ORDER);
	pr_info("ttm selftest: %d-page buffer filled in system memory, moved to VRAM at +%lu KiB and back, data intact\n",
		BO_PAGES, vram_offset >> 10);
	return 0;
}

module_init(ttm_selftest);
