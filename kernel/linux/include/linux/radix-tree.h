#ifndef _AEROS_LINUX_RADIX_TREE_H
#define _AEROS_LINUX_RADIX_TREE_H

#include <linux/xarray.h>
#include <linux/errno.h>
/* The radix-tree API on AerOS's xarray; tags are kept as one set per tag. */
#define RADIX_TREE_MAX_TAGS 3
struct radix_tree_root {
	struct xarray entries;
	struct xarray tags[RADIX_TREE_MAX_TAGS];
};
struct radix_tree_iter {
	unsigned long index;
	void *entry;
};
static inline void __radix_tree_init(struct radix_tree_root *root)
{
	int i;

	xa_init(&root->entries);
	for (i = 0; i < RADIX_TREE_MAX_TAGS; i++)
		xa_init(&root->tags[i]);
}
#define INIT_RADIX_TREE(root, mask) __radix_tree_init(root)
#define RADIX_TREE_INIT(name, mask) { 0 }
static inline int radix_tree_insert(struct radix_tree_root *root, unsigned long index, void *item)
{
	int ret = xa_insert(&root->entries, index, item, GFP_KERNEL);

	return ret == -EBUSY ? -EEXIST : ret;
}
static inline void *radix_tree_lookup(struct radix_tree_root *root, unsigned long index)
{
	return xa_load(&root->entries, index);
}
static inline void *radix_tree_delete(struct radix_tree_root *root, unsigned long index)
{
	int i;

	for (i = 0; i < RADIX_TREE_MAX_TAGS; i++)
		xa_erase(&root->tags[i], index);
	return xa_erase(&root->entries, index);
}
static inline void *radix_tree_tag_set(struct radix_tree_root *root, unsigned long index, unsigned int tag)
{
	void *item = xa_load(&root->entries, index);

	if (item)
		xa_store(&root->tags[tag], index, xa_mk_value(1), GFP_KERNEL);
	return item;
}
static inline void *radix_tree_tag_clear(struct radix_tree_root *root, unsigned long index, unsigned int tag)
{
	xa_erase(&root->tags[tag], index);
	return xa_load(&root->entries, index);
}
static inline int radix_tree_tag_get(struct radix_tree_root *root, unsigned long index, unsigned int tag)
{
	return xa_load(&root->tags[tag], index) != NULL;
}
static inline int radix_tree_tagged(struct radix_tree_root *root, unsigned int tag)
{
	return !xa_empty(&root->tags[tag]);
}
static inline unsigned int radix_tree_gang_lookup_tag(struct radix_tree_root *root, void **results,
						      unsigned long first_index, unsigned int max_items,
						      unsigned int tag)
{
	unsigned long index = first_index;
	unsigned int found = 0;

	while (found < max_items && xa_find(&root->tags[tag], &index, ~0UL, XA_PRESENT)) {
		results[found++] = xa_load(&root->entries, index);
		if (index == ~0UL)
			break;
		index++;
	}
	return found;
}
#define radix_tree_for_each_slot(slot, root, iter, start) \
	for ((iter)->index = (start); \
	     ((iter)->entry = xa_find(&(root)->entries, &(iter)->index, ~0UL, XA_PRESENT)) && \
	     ((slot) = (void **)&(iter)->entry); \
	     (iter)->index++)
#define radix_tree_deref_slot(slot) (*(slot))
static inline void radix_tree_iter_delete(struct radix_tree_root *root, struct radix_tree_iter *iter, void **slot)
{
	(void)slot;
	radix_tree_delete(root, iter->index);
}
#endif
