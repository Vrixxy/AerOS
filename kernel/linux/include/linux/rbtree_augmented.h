/* Augmented red-black trees (each node keeps a value computed from its
 * subtree, kept up to date through rotations), Linux interface. */
#ifndef _LINUX_RBTREE_AUGMENTED_H
#define _LINUX_RBTREE_AUGMENTED_H

#include <linux/rbtree.h>

struct rb_augment_callbacks {
	void (*propagate)(struct rb_node *node, struct rb_node *stop);
	void (*copy)(struct rb_node *old, struct rb_node *new_node);
	void (*rotate)(struct rb_node *old, struct rb_node *new_node);
};

void rb_insert_augmented(struct rb_node *node, struct rb_root *root,
			 const struct rb_augment_callbacks *augment);
void rb_erase_augmented(struct rb_node *node, struct rb_root *root,
			const struct rb_augment_callbacks *augment);

static inline void rb_insert_augmented_cached(struct rb_node *node, struct rb_root_cached *root,
					      bool leftmost, const struct rb_augment_callbacks *augment)
{
	if (leftmost)
		root->rb_leftmost = node;
	rb_insert_augmented(node, &root->rb_root, augment);
}

static inline void rb_erase_augmented_cached(struct rb_node *node, struct rb_root_cached *root,
					     const struct rb_augment_callbacks *augment)
{
	if (root->rb_leftmost == node)
		root->rb_leftmost = rb_next(node);
	rb_erase_augmented(node, &root->rb_root, augment);
}

/* Callbacks for an augmented value that is the maximum of `compute` over
 * a node's subtree (what interval trees and drm_mm keep). */
#define RB_DECLARE_CALLBACKS_MAX(rbstatic, rbname, rbstruct, rbfield, rbtype, rbaugmented, rbcompute) \
static inline bool rbname##_compute_max(rbstruct *node, bool exit) \
{ \
	rbstruct *child; \
	rbtype max = rbcompute(node); \
	if (node->rbfield.rb_left) { \
		child = rb_entry(node->rbfield.rb_left, rbstruct, rbfield); \
		if (child->rbaugmented > max) max = child->rbaugmented; \
	} \
	if (node->rbfield.rb_right) { \
		child = rb_entry(node->rbfield.rb_right, rbstruct, rbfield); \
		if (child->rbaugmented > max) max = child->rbaugmented; \
	} \
	if (exit && node->rbaugmented == max) return true; \
	node->rbaugmented = max; \
	return false; \
} \
static void rbname##_propagate(struct rb_node *rb, struct rb_node *stop) \
{ \
	while (rb != stop) { \
		rbstruct *node = rb_entry(rb, rbstruct, rbfield); \
		if (rbname##_compute_max(node, true)) break; \
		rb = rb_parent(&node->rbfield); \
	} \
} \
static void rbname##_copy(struct rb_node *rb_old, struct rb_node *rb_new) \
{ \
	rb_entry(rb_new, rbstruct, rbfield)->rbaugmented = rb_entry(rb_old, rbstruct, rbfield)->rbaugmented; \
} \
static void rbname##_rotate(struct rb_node *rb_old, struct rb_node *rb_new) \
{ \
	rbstruct *old = rb_entry(rb_old, rbstruct, rbfield); \
	rb_entry(rb_new, rbstruct, rbfield)->rbaugmented = old->rbaugmented; \
	rbname##_compute_max(old, false); \
} \
rbstatic const struct rb_augment_callbacks rbname = { \
	.propagate = rbname##_propagate, .copy = rbname##_copy, .rotate = rbname##_rotate, \
};

#endif
