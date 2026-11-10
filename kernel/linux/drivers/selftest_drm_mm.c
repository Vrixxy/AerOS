// SPDX-License-Identifier: MIT
/* Boot self-test of the unmodified drm_mm.c range allocator (what amdgpu
 * uses to carve up VRAM and GTT): random reserves, inserts in every mode
 * and removals, checking alignment, range and overlap on each step. */
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <drm/drm_mm.h>
#include <drm/drm_print.h>
#include <linux/seq_file.h>

#define NODES 384
#define SPACE (1ull << 18)

static struct drm_mm mm;
static struct drm_mm_node nodes[NODES];
static u64 seed = 0x2545f4914f6cdd1dull;

static u32 next_random(void)
{
	seed ^= seed << 13;
	seed ^= seed >> 7;
	seed ^= seed << 17;
	return (u32)seed;
}

/* Prints a tiny allocator through drm_print's seq_file printer (into the log). */
static void print_sample(void)
{
	struct seq_file log = { 0 };
	struct drm_printer printer = drm_seq_file_printer(&log);
	struct drm_mm sample;
	struct drm_mm_node first = { 0 }, second = { 0 };

	drm_mm_init(&sample, 0x1000, 0x10000);
	drm_mm_insert_node(&sample, &first, 0x2000);
	drm_mm_insert_node_generic(&sample, &second, 0x800, 0x1000, 0, DRM_MM_INSERT_HIGH);
	drm_mm_print(&sample, &printer);
	drm_mm_remove_node(&second);
	drm_mm_remove_node(&first);
	drm_mm_takedown(&sample);
}

static bool overlaps(int index)
{
	const struct drm_mm_node *node = &nodes[index];
	int other;

	for (other = 0; other < NODES; other++) {
		const struct drm_mm_node *peer = &nodes[other];

		if (other == index || !drm_mm_node_allocated(peer))
			continue;
		if (node->start < peer->start + peer->size && peer->start < node->start + node->size)
			return true;
	}
	return false;
}

static int __init drm_mm_selftest(void)
{
	static const enum drm_mm_insert_mode modes[] = {
		DRM_MM_INSERT_BEST, DRM_MM_INSERT_LOW, DRM_MM_INSERT_HIGH,
	};
	u32 steps, inserted = 0, full = 0, reserved = 0;
	struct drm_mm_node *node;
	u64 previous_end = 0, used = 0;
	int index;

	drm_mm_init(&mm, 0, SPACE);
	for (steps = 0; steps < 6000; steps++) {
		index = (int)(next_random() % NODES);
		node = &nodes[index];
		if (drm_mm_node_allocated(node)) {
			drm_mm_remove_node(node);
			memset(node, 0, sizeof(*node));
			continue;
		}
		if (next_random() % 8 == 0) {
			node->start = next_random() % SPACE;
			node->size = 1 + next_random() % 64;
			if (node->start + node->size > SPACE)
				node->size = SPACE - node->start;
			if (drm_mm_reserve_node(&mm, node) == 0) {
				reserved++;
				if (overlaps(index))
					goto bad;
			} else {
				memset(node, 0, sizeof(*node));
			}
			continue;
		}
		{
			u64 size = 1 + next_random() % 2048;
			u64 alignment = 1ull << (next_random() % 9);
			enum drm_mm_insert_mode mode = modes[next_random() % 3];
			int error = drm_mm_insert_node_generic(&mm, node, size, alignment, 0, mode);

			if (error == -ENOSPC) {
				full++;
				memset(node, 0, sizeof(*node));
				continue;
			}
			if (error || node->size != size || node->start % alignment ||
			    node->start + node->size > SPACE || overlaps(index))
				goto bad;
			inserted++;
		}
	}
	drm_mm_for_each_node(node, &mm) {
		if (node->start < previous_end)
			goto bad;
		previous_end = node->start + node->size;
		used += node->size;
	}
	for (index = 0; index < NODES; index++) {
		if (drm_mm_node_allocated(&nodes[index]))
			drm_mm_remove_node(&nodes[index]);
	}
	if (!drm_mm_clean(&mm))
		goto bad;
	drm_mm_takedown(&mm);
	print_sample();
	pr_info("drm_mm selftest: %u steps ok (%u inserts, %u reserves, %u full), %llu units were in use\n",
		steps, inserted, reserved, full, used);
	return 0;
bad:
	pr_err("drm_mm selftest: failed at step %u (node %d)\n", steps, index);
	return -EINVAL;
}

module_init(drm_mm_selftest);
