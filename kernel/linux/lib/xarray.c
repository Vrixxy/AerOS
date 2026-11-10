// SPDX-License-Identifier: MIT
#include <linux/xarray.h>
#include <linux/slab.h>
#include <linux/errno.h>

/* A reserved slot (xa_alloc with no entry): taken, but reads as NULL. */
#define ZERO_ENTRY ((void *)0x406)

static void *visible(void *entry)
{
	return entry == ZERO_ENTRY ? NULL : entry;
}

struct xa_node {
	struct rb_node rb;
	unsigned long index;
	void *entry;
};

static struct xa_node *find(struct xarray *xa, unsigned long index)
{
	struct rb_node *node = xa->root.rb_node;

	while (node) {
		struct xa_node *item = rb_entry(node, struct xa_node, rb);

		if (index < item->index)
			node = node->rb_left;
		else if (index > item->index)
			node = node->rb_right;
		else
			return item;
	}
	return NULL;
}

/* The first entry with an index >= index, or NULL. */
static struct xa_node *first_from(struct xarray *xa, unsigned long index)
{
	struct rb_node *node = xa->root.rb_node;
	struct xa_node *best = NULL;

	while (node) {
		struct xa_node *item = rb_entry(node, struct xa_node, rb);

		if (item->index >= index) {
			best = item;
			node = node->rb_left;
		} else {
			node = node->rb_right;
		}
	}
	return best;
}

static int insert(struct xarray *xa, unsigned long index, void *entry, gfp_t gfp)
{
	struct rb_node **link = &xa->root.rb_node, *parent = NULL;
	struct xa_node *item = kmalloc(sizeof(*item), gfp);

	if (!item)
		return -ENOMEM;
	item->index = index;
	item->entry = entry;
	while (*link) {
		parent = *link;
		link = index < rb_entry(parent, struct xa_node, rb)->index ? &parent->rb_left : &parent->rb_right;
	}
	rb_link_node(&item->rb, parent, link);
	rb_insert_color(&item->rb, &xa->root);
	xa->count++;
	return 0;
}

static void *remove(struct xarray *xa, struct xa_node *item)
{
	void *entry = item->entry;

	rb_erase(&item->rb, &xa->root);
	kfree(item);
	xa->count--;
	return entry;
}

void *xa_load(struct xarray *xa, unsigned long index)
{
	struct xa_node *item = find(xa, index);

	return item ? visible(item->entry) : NULL;
}

void *__xa_store(struct xarray *xa, unsigned long index, void *entry, gfp_t gfp)
{
	struct xa_node *item = find(xa, index);
	void *old;

	if (!entry)
		return item ? visible(remove(xa, item)) : NULL;
	if (item) {
		old = item->entry;
		item->entry = entry;
		return visible(old);
	}
	return insert(xa, index, entry, gfp) ? xa_mk_err(-ENOMEM) : NULL;
}

void *__xa_erase(struct xarray *xa, unsigned long index)
{
	struct xa_node *item = find(xa, index);

	return item ? visible(remove(xa, item)) : NULL;
}

int __xa_insert(struct xarray *xa, unsigned long index, void *entry, gfp_t gfp)
{
	if (find(xa, index))
		return -EBUSY;
	if (!entry)
		return 0;
	return insert(xa, index, entry, gfp);
}

void *__xa_cmpxchg(struct xarray *xa, unsigned long index, void *old, void *entry, gfp_t gfp)
{
	void *current_entry = xa_load(xa, index);

	if (current_entry == old) {
		void *result = __xa_store(xa, index, entry, gfp);

		if (xa_is_err(result))
			return result;
	}
	return current_entry;
}

/* The lowest free index in [min, max], or -EBUSY. */
static int free_index(struct xarray *xa, u32 min, u32 max, u32 *id)
{
	unsigned long candidate = min;
	struct xa_node *item = first_from(xa, min);

	while (item && item->index == candidate) {
		struct rb_node *next = rb_next(&item->rb);

		candidate++;
		item = next ? rb_entry(next, struct xa_node, rb) : NULL;
	}
	if (candidate > max)
		return -EBUSY;
	*id = (u32)candidate;
	return 0;
}

int __xa_alloc(struct xarray *xa, u32 *id, void *entry, struct xa_limit limit, gfp_t gfp)
{
	u32 min = limit.min;
	int error;

	if ((xa->xa_flags & XA_FLAGS_ZERO_BUSY) && min == 0)
		min = 1;
	error = free_index(xa, min, limit.max, id);
	if (error)
		return error;
	return insert(xa, *id, entry ? entry : ZERO_ENTRY, gfp);
}

int __xa_alloc_cyclic(struct xarray *xa, u32 *id, void *entry, struct xa_limit limit, u32 *next, gfp_t gfp)
{
	u32 min = limit.min;
	bool wrapped = false;
	int error;

	if (*next > min && *next <= limit.max)
		min = *next;
	error = free_index(xa, min, limit.max, id);
	if (error == -EBUSY && min > limit.min) {
		error = free_index(xa, limit.min, limit.max, id);
		wrapped = true;
	}
	if (error)
		return error;
	error = insert(xa, *id, entry ? entry : ZERO_ENTRY, gfp);
	if (error)
		return error;
	*next = *id + 1;
	return wrapped ? 1 : 0;
}

void *xa_find(struct xarray *xa, unsigned long *index, unsigned long max, unsigned long filter)
{
	struct xa_node *item = first_from(xa, *index);

	(void)filter;
	while (item && item->entry == ZERO_ENTRY) {
		struct rb_node *next = rb_next(&item->rb);

		item = next ? rb_entry(next, struct xa_node, rb) : NULL;
	}
	if (!item || item->index > max)
		return NULL;
	*index = item->index;
	return item->entry;
}

void *xa_find_after(struct xarray *xa, unsigned long *index, unsigned long max, unsigned long filter)
{
	unsigned long next = *index + 1;

	if (next == 0)
		return NULL;
	*index = next;
	return xa_find(xa, index, max, filter);
}

void xa_destroy(struct xarray *xa)
{
	while (xa->root.rb_node)
		remove(xa, rb_entry(xa->root.rb_node, struct xa_node, rb));
}
