// SPDX-License-Identifier: MIT
/* Boot self-test of the unmodified GPU buddy allocator (amdgpu's VRAM
 * manager): random plain, top-down, contiguous and range allocations and
 * frees on a 256 MiB space, checking alignment, bounds, accounting and
 * overlap. */
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <drm/drm_buddy.h>

#define SPACE (256ull << 20)
#define CHUNK (4ull << 10)
#define SLOTS 48
#define MAX_RANGES 8192

struct allocation {
	struct list_head blocks;
	u64 size;
	bool used;
};

struct range {
	u64 start, end;
};

static struct gpu_buddy mm;
static struct allocation slots[SLOTS];
static struct range ranges[MAX_RANGES];
static u64 seed = 0x853c49e6748fea9bull;

static u64 next_random(void)
{
	seed ^= seed << 13;
	seed ^= seed >> 7;
	seed ^= seed << 17;
	return seed;
}

static void sort_ranges(int count)
{
	int gap, i, j;

	for (gap = count / 2; gap > 0; gap /= 2) {
		for (i = gap; i < count; i++) {
			struct range value = ranges[i];

			for (j = i; j >= gap && ranges[j - gap].start > value.start; j -= gap)
				ranges[j] = ranges[j - gap];
			ranges[j] = value;
		}
	}
}

/* Every allocated block is disjoint from every other one. */
static bool disjoint(void)
{
	struct gpu_buddy_block *block;
	int count = 0, i, slot;

	for (slot = 0; slot < SLOTS; slot++) {
		if (!slots[slot].used)
			continue;
		list_for_each_entry(block, &slots[slot].blocks, link) {
			if (count == MAX_RANGES)
				return true;
			ranges[count].start = gpu_buddy_block_offset(block);
			ranges[count].end = ranges[count].start + gpu_buddy_block_size(&mm, block);
			count++;
		}
	}
	sort_ranges(count);
	for (i = 1; i < count; i++)
		if (ranges[i].start < ranges[i - 1].end)
			return false;
	return true;
}

static bool check(struct allocation *allocation, u64 alignment, u64 start, u64 end)
{
	struct gpu_buddy_block *block;
	u64 total = 0;

	list_for_each_entry(block, &allocation->blocks, link) {
		u64 offset = gpu_buddy_block_offset(block);
		u64 size = gpu_buddy_block_size(&mm, block);

		if (!gpu_buddy_block_is_allocated(block) || offset % alignment || size % alignment ||
		    offset < start || offset + size > end)
			return false;
		total += size;
	}
	return total >= allocation->size;
}

static int __init buddy_selftest(void)
{
	u32 steps, allocations = 0, frees = 0, full = 0;
	u64 in_use = 0;
	int slot, error;

	error = gpu_buddy_init(&mm, SPACE, CHUNK);
	if (error) {
		pr_err("buddy selftest: init failed (%d)\n", error);
		return error;
	}
	for (slot = 0; slot < SLOTS; slot++)
		INIT_LIST_HEAD(&slots[slot].blocks);
	for (steps = 0; steps < 3000; steps++) {
		struct allocation *allocation = &slots[next_random() % SLOTS];
		u64 alignment, size, start = 0, end = SPACE;
		unsigned long flags = 0;

		if (allocation->used) {
			struct gpu_buddy_block *block;

			list_for_each_entry(block, &allocation->blocks, link)
				in_use -= gpu_buddy_block_size(&mm, block);
			gpu_buddy_free_list(&mm, &allocation->blocks, 0);
			allocation->used = false;
			frees++;
			continue;
		}
		alignment = CHUNK << (next_random() % 6);
		size = alignment * (1 + next_random() % 256);
		switch (next_random() % 4) {
		case 1:
			flags = GPU_BUDDY_TOPDOWN_ALLOCATION;
			break;
		case 2:
			flags = GPU_BUDDY_CONTIGUOUS_ALLOCATION;
			break;
		case 3:
			flags = GPU_BUDDY_RANGE_ALLOCATION;
			start = round_down(next_random() % (SPACE / 2), alignment);
			end = start + round_up(size * 2, alignment);
			break;
		}
		allocation->size = size;
		error = gpu_buddy_alloc_blocks(&mm, start, end, size, alignment, &allocation->blocks, flags);
		if (error == -ENOSPC) {
			full++;
			continue;
		}
		if (error || !check(allocation, (flags & GPU_BUDDY_CONTIGUOUS_ALLOCATION) ? CHUNK : alignment,
				    start, end)) {
			pr_err("buddy selftest: step %u: allocation of %llu (align %llu, flags %lx) failed: %d\n",
			       steps, size, alignment, flags, error);
			return -EINVAL;
		}
		{
			struct gpu_buddy_block *block;

			list_for_each_entry(block, &allocation->blocks, link)
				in_use += gpu_buddy_block_size(&mm, block);
		}
		allocation->used = true;
		allocations++;
		if (mm.avail != SPACE - in_use || (steps % 250 == 0 && !disjoint())) {
			pr_err("buddy selftest: step %u: accounting or overlap broken (avail %llu, in use %llu)\n",
			       steps, mm.avail, in_use);
			return -EINVAL;
		}
	}
	if (!disjoint()) {
		pr_err("buddy selftest: blocks overlap at the end\n");
		return -EINVAL;
	}
	pr_info("buddy selftest: %u steps ok (%u allocations, %u frees, %u full), %llu MiB in use\n",
		steps, allocations, frees, full, in_use >> 20);
	for (slot = 0; slot < SLOTS; slot++)
		if (slots[slot].used)
			gpu_buddy_free_list(&mm, &slots[slot].blocks, 0);
	if (mm.avail != SPACE) {
		pr_err("buddy selftest: %llu bytes missing after freeing everything\n", SPACE - mm.avail);
		return -EINVAL;
	}
	gpu_buddy_fini(&mm);
	return 0;
}

module_init(buddy_selftest);
