/* Red-black trees with the Linux interface (written for AerOS). The node's
 * parent pointer and colour share one word, as in Linux, since drivers
 * embed rb_node and some read the layout. */
#ifndef _LINUX_RBTREE_H
#define _LINUX_RBTREE_H

#include <linux/kernel.h>

#define RB_RED 0
#define RB_BLACK 1

struct rb_node {
	unsigned long __rb_parent_color;
	struct rb_node *rb_right;
	struct rb_node *rb_left;
} __attribute__((aligned(sizeof(long))));

struct rb_root {
	struct rb_node *rb_node;
};

struct rb_root_cached {
	struct rb_root rb_root;
	struct rb_node *rb_leftmost;
};

#define RB_ROOT (struct rb_root) { NULL }
#define RB_ROOT_CACHED (struct rb_root_cached) { { NULL }, NULL }
#define rb_parent(r) ((struct rb_node *)((r)->__rb_parent_color & ~3UL))
#define rb_entry(ptr, type, member) container_of(ptr, type, member)
#define rb_entry_safe(ptr, type, member) \
	({ __typeof__(ptr) ____ptr = (ptr); ____ptr ? rb_entry(____ptr, type, member) : NULL; })
#define RB_EMPTY_ROOT(root) (READ_ONCE((root)->rb_node) == NULL)
#define RB_EMPTY_NODE(node) ((node)->__rb_parent_color == (unsigned long)(node))
#define RB_CLEAR_NODE(node) ((node)->__rb_parent_color = (unsigned long)(node))

#ifndef READ_ONCE
#define READ_ONCE(x) (*(const volatile __typeof__(x) *)&(x))
#endif

static inline void rb_link_node(struct rb_node *node, struct rb_node *parent, struct rb_node **link)
{
	node->__rb_parent_color = (unsigned long)parent;
	node->rb_left = node->rb_right = NULL;
	*link = node;
}

void rb_insert_color(struct rb_node *node, struct rb_root *root);
void rb_erase(struct rb_node *node, struct rb_root *root);
struct rb_node *rb_first(const struct rb_root *root);
struct rb_node *rb_last(const struct rb_root *root);
struct rb_node *rb_next(const struct rb_node *node);
struct rb_node *rb_prev(const struct rb_node *node);
void rb_replace_node(struct rb_node *victim, struct rb_node *replacement, struct rb_root *root);
struct rb_node *rb_first_postorder(const struct rb_root *root);
struct rb_node *rb_next_postorder(const struct rb_node *node);

#define rb_first_cached(root) (root)->rb_leftmost

static inline void rb_insert_color_cached(struct rb_node *node, struct rb_root_cached *root, bool leftmost)
{
	if (leftmost)
		root->rb_leftmost = node;
	rb_insert_color(node, &root->rb_root);
}

static inline struct rb_node *rb_erase_cached(struct rb_node *node, struct rb_root_cached *root)
{
	struct rb_node *leftmost = NULL;

	if (root->rb_leftmost == node)
		leftmost = root->rb_leftmost = rb_next(node);
	rb_erase(node, &root->rb_root);
	return leftmost;
}

static inline void rb_add_cached(struct rb_node *node, struct rb_root_cached *tree,
				 bool (*less)(struct rb_node *, const struct rb_node *))
{
	struct rb_node **link = &tree->rb_root.rb_node, *parent = NULL;
	bool leftmost = true;

	while (*link) {
		parent = *link;
		if (less(node, parent)) {
			link = &parent->rb_left;
		} else {
			link = &parent->rb_right;
			leftmost = false;
		}
	}
	rb_link_node(node, parent, link);
	rb_insert_color_cached(node, tree, leftmost);
}

static inline void rb_add(struct rb_node *node, struct rb_root *tree,
			  bool (*less)(struct rb_node *, const struct rb_node *))
{
	struct rb_node **link = &tree->rb_node, *parent = NULL;

	while (*link) {
		parent = *link;
		link = less(node, parent) ? &parent->rb_left : &parent->rb_right;
	}
	rb_link_node(node, parent, link);
	rb_insert_color(node, tree);
}

static inline struct rb_node *rb_find(const void *key, const struct rb_root *tree,
				      int (*cmp)(const void *key, const struct rb_node *))
{
	struct rb_node *node = tree->rb_node;

	while (node) {
		int c = cmp(key, node);
		if (c < 0)
			node = node->rb_left;
		else if (c > 0)
			node = node->rb_right;
		else
			return node;
	}
	return NULL;
}

#define rbtree_postorder_for_each_entry_safe(pos, n, root, field) \
	for (pos = rb_entry_safe(rb_first_postorder(root), __typeof__(*pos), field); \
	     pos && ({ n = rb_entry_safe(rb_next_postorder(&pos->field), __typeof__(*pos), field); 1; }); \
	     pos = n)

#endif
