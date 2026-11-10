// SPDX-License-Identifier: MIT
/* Boot self-test of lib/rbtree.c: random inserts and erases on an
 * augmented tree (subtree maximum), checking the red-black rules, the
 * order and every augmented value after each step. */
#include <linux/module.h>
#include <linux/rbtree_augmented.h>
#include <linux/slab.h>

#define NODES 256

struct item {
	struct rb_node rb;
	u32 key, weight, subtree_max;
	bool in;
};

static struct item items[NODES];
static struct rb_root tree;
static u64 seed = 0x9e3779b97f4a7c15ull;

static u32 next_random(void)
{
	seed ^= seed << 13;
	seed ^= seed >> 7;
	seed ^= seed << 17;
	return (u32)seed;
}

#define ITEM_WEIGHT(item) ((item)->weight)
RB_DECLARE_CALLBACKS_MAX(static, item_callbacks, struct item, rb, u32, subtree_max, ITEM_WEIGHT)

static int check(struct rb_node *node, u32 *max, const struct rb_node *parent)
{
	u32 left_max = 0, right_max = 0;
	int left, right;
	struct item *item;

	if (!node) {
		*max = 0;
		return 1;
	}
	item = rb_entry(node, struct item, rb);
	if (rb_parent(node) != parent)
		return -1;
	if ((node->__rb_parent_color & 1) == RB_RED &&
	    ((node->rb_left && !(node->rb_left->__rb_parent_color & 1)) ||
	     (node->rb_right && !(node->rb_right->__rb_parent_color & 1))))
		return -1;
	left = check(node->rb_left, &left_max, node);
	right = check(node->rb_right, &right_max, node);
	if (left < 0 || right < 0 || left != right)
		return -1;
	*max = max(item->weight, max(left_max, right_max));
	if (item->subtree_max != *max)
		return -1;
	return left + (int)(node->__rb_parent_color & 1);
}

static void insert(struct item *item)
{
	struct rb_node **link = &tree.rb_node, *parent = NULL;

	item->subtree_max = item->weight;
	while (*link) {
		struct item *at = rb_entry(*link, struct item, rb);
		parent = *link;
		if (at->subtree_max < item->weight)
			at->subtree_max = item->weight;
		link = item->key < at->key ? &parent->rb_left : &parent->rb_right;
	}
	rb_link_node(&item->rb, parent, link);
	rb_insert_augmented(&item->rb, &tree, &item_callbacks);
}

static int __init rbtree_selftest(void)
{
	unsigned int steps = 0, count = 0;

	tree = RB_ROOT;
	for (unsigned int i = 0; i < NODES; i++) {
		items[i].key = next_random();
		items[i].weight = next_random() % 100000;
	}
	for (; steps < 20000; steps++) {
		struct item *item = &items[next_random() % NODES];
		u32 max_value;

		if (item->in) {
			rb_erase_augmented(&item->rb, &tree, &item_callbacks);
			item->in = false;
			count--;
		} else {
			insert(item);
			item->in = true;
			count++;
		}
		if (check(tree.rb_node, &max_value, NULL) < 0 ||
		    (tree.rb_node && (tree.rb_node->__rb_parent_color & 1) != RB_BLACK)) {
			pr_err("rbtree selftest: broken after %u steps\n", steps);
			return -EINVAL;
		}
	}
	{
		unsigned int seen = 0;
		u32 last = 0;
		for (struct rb_node *node = rb_first(&tree); node; node = rb_next(node)) {
			u32 key = rb_entry(node, struct item, rb)->key;
			if (seen && key < last) {
				pr_err("rbtree selftest: out of order\n");
				return -EINVAL;
			}
			last = key;
			seen++;
		}
		if (seen != count) {
			pr_err("rbtree selftest: walked %u of %u\n", seen, count);
			return -EINVAL;
		}
	}
	pr_info("rbtree selftest: %u random steps ok, %u nodes left\n", steps, count);
	return 0;
}

module_init(rbtree_selftest);
MODULE_LICENSE("MIT");
