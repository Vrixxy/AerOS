/* Red-black trees (written for AerOS from the textbook algorithm), with the
 * augment hooks Linux's interface promises: `rotate` on every rotation,
 * and on erase `copy` when the successor takes the erased node's place,
 * then `propagate` up from where the tree changed. */
#include <linux/rbtree_augmented.h>

static inline int color(const struct rb_node *node)
{
	return node ? (int)(node->__rb_parent_color & 1) : RB_BLACK;
}

static inline void set_color(struct rb_node *node, int value)
{
	node->__rb_parent_color = (node->__rb_parent_color & ~1UL) | (unsigned long)value;
}

static inline void set_parent(struct rb_node *node, struct rb_node *parent)
{
	node->__rb_parent_color = (node->__rb_parent_color & 1) | (unsigned long)parent;
}

static void change_child(struct rb_node *old, struct rb_node *new_node, struct rb_node *parent,
			 struct rb_root *root)
{
	if (!parent)
		root->rb_node = new_node;
	else if (parent->rb_left == old)
		parent->rb_left = new_node;
	else
		parent->rb_right = new_node;
}

static void rotate_left(struct rb_node *x, struct rb_root *root, const struct rb_augment_callbacks *augment)
{
	struct rb_node *y = x->rb_right, *parent = rb_parent(x);

	x->rb_right = y->rb_left;
	if (y->rb_left)
		set_parent(y->rb_left, x);
	set_parent(y, parent);
	change_child(x, y, parent, root);
	y->rb_left = x;
	set_parent(x, y);
	if (augment)
		augment->rotate(x, y);
}

static void rotate_right(struct rb_node *x, struct rb_root *root, const struct rb_augment_callbacks *augment)
{
	struct rb_node *y = x->rb_left, *parent = rb_parent(x);

	x->rb_left = y->rb_right;
	if (y->rb_right)
		set_parent(y->rb_right, x);
	set_parent(y, parent);
	change_child(x, y, parent, root);
	y->rb_right = x;
	set_parent(x, y);
	if (augment)
		augment->rotate(x, y);
}

static void insert_fixup(struct rb_node *node, struct rb_root *root,
			 const struct rb_augment_callbacks *augment)
{
	struct rb_node *parent, *grand, *uncle;

	set_color(node, RB_RED);
	while ((parent = rb_parent(node)) && color(parent) == RB_RED) {
		grand = rb_parent(parent);
		if (parent == grand->rb_left) {
			uncle = grand->rb_right;
			if (color(uncle) == RB_RED) {
				set_color(parent, RB_BLACK);
				set_color(uncle, RB_BLACK);
				set_color(grand, RB_RED);
				node = grand;
				continue;
			}
			if (node == parent->rb_right) {
				rotate_left(parent, root, augment);
				node = parent;
				parent = rb_parent(node);
			}
			set_color(parent, RB_BLACK);
			set_color(grand, RB_RED);
			rotate_right(grand, root, augment);
		} else {
			uncle = grand->rb_left;
			if (color(uncle) == RB_RED) {
				set_color(parent, RB_BLACK);
				set_color(uncle, RB_BLACK);
				set_color(grand, RB_RED);
				node = grand;
				continue;
			}
			if (node == parent->rb_left) {
				rotate_right(parent, root, augment);
				node = parent;
				parent = rb_parent(node);
			}
			set_color(parent, RB_BLACK);
			set_color(grand, RB_RED);
			rotate_left(grand, root, augment);
		}
	}
	set_color(root->rb_node, RB_BLACK);
}

static void erase_fixup(struct rb_node *x, struct rb_node *parent, struct rb_root *root,
			const struct rb_augment_callbacks *augment)
{
	struct rb_node *sibling;

	while (x != root->rb_node && color(x) == RB_BLACK) {
		if (x == parent->rb_left) {
			sibling = parent->rb_right;
			if (color(sibling) == RB_RED) {
				set_color(sibling, RB_BLACK);
				set_color(parent, RB_RED);
				rotate_left(parent, root, augment);
				sibling = parent->rb_right;
			}
			if (color(sibling->rb_left) == RB_BLACK && color(sibling->rb_right) == RB_BLACK) {
				set_color(sibling, RB_RED);
				x = parent;
				parent = rb_parent(x);
				continue;
			}
			if (color(sibling->rb_right) == RB_BLACK) {
				set_color(sibling->rb_left, RB_BLACK);
				set_color(sibling, RB_RED);
				rotate_right(sibling, root, augment);
				sibling = parent->rb_right;
			}
			set_color(sibling, color(parent));
			set_color(parent, RB_BLACK);
			set_color(sibling->rb_right, RB_BLACK);
			rotate_left(parent, root, augment);
		} else {
			sibling = parent->rb_left;
			if (color(sibling) == RB_RED) {
				set_color(sibling, RB_BLACK);
				set_color(parent, RB_RED);
				rotate_right(parent, root, augment);
				sibling = parent->rb_left;
			}
			if (color(sibling->rb_left) == RB_BLACK && color(sibling->rb_right) == RB_BLACK) {
				set_color(sibling, RB_RED);
				x = parent;
				parent = rb_parent(x);
				continue;
			}
			if (color(sibling->rb_left) == RB_BLACK) {
				set_color(sibling->rb_right, RB_BLACK);
				set_color(sibling, RB_RED);
				rotate_left(sibling, root, augment);
				sibling = parent->rb_left;
			}
			set_color(sibling, color(parent));
			set_color(parent, RB_BLACK);
			set_color(sibling->rb_left, RB_BLACK);
			rotate_right(parent, root, augment);
		}
		x = root->rb_node;
		break;
	}
	if (x)
		set_color(x, RB_BLACK);
}

static void erase(struct rb_node *node, struct rb_root *root, const struct rb_augment_callbacks *augment)
{
	struct rb_node *parent = rb_parent(node), *x, *x_parent;
	int removed;

	if (!node->rb_left || !node->rb_right) {
		x = node->rb_left ? node->rb_left : node->rb_right;
		x_parent = parent;
		removed = color(node);
		change_child(node, x, parent, root);
		if (x)
			set_parent(x, parent);
		if (augment && parent)
			augment->propagate(parent, NULL);
	} else {
		struct rb_node *successor = node->rb_right;

		while (successor->rb_left)
			successor = successor->rb_left;
		removed = color(successor);
		x = successor->rb_right;
		if (rb_parent(successor) == node) {
			x_parent = successor;
		} else {
			x_parent = rb_parent(successor);
			x_parent->rb_left = x;
			if (x)
				set_parent(x, x_parent);
			successor->rb_right = node->rb_right;
			set_parent(node->rb_right, successor);
		}
		change_child(node, successor, parent, root);
		successor->rb_left = node->rb_left;
		set_parent(node->rb_left, successor);
		successor->__rb_parent_color = node->__rb_parent_color;
		if (augment) {
			augment->copy(node, successor);
			if (x_parent != successor)
				augment->propagate(x_parent, successor);
			augment->propagate(successor, NULL);
		}
	}
	if (removed == RB_BLACK)
		erase_fixup(x, x_parent, root, augment);
}

void rb_insert_color(struct rb_node *node, struct rb_root *root)
{
	insert_fixup(node, root, NULL);
}

void rb_insert_augmented(struct rb_node *node, struct rb_root *root,
			 const struct rb_augment_callbacks *augment)
{
	insert_fixup(node, root, augment);
}

void rb_erase(struct rb_node *node, struct rb_root *root)
{
	erase(node, root, NULL);
}

void rb_erase_augmented(struct rb_node *node, struct rb_root *root,
			const struct rb_augment_callbacks *augment)
{
	erase(node, root, augment);
}

struct rb_node *rb_first(const struct rb_root *root)
{
	struct rb_node *node = root->rb_node;

	while (node && node->rb_left)
		node = node->rb_left;
	return node;
}

struct rb_node *rb_last(const struct rb_root *root)
{
	struct rb_node *node = root->rb_node;

	while (node && node->rb_right)
		node = node->rb_right;
	return node;
}

struct rb_node *rb_next(const struct rb_node *node)
{
	struct rb_node *parent;

	if (RB_EMPTY_NODE(node))
		return NULL;
	if (node->rb_right) {
		node = node->rb_right;
		while (node->rb_left)
			node = node->rb_left;
		return (struct rb_node *)node;
	}
	while ((parent = rb_parent(node)) && node == parent->rb_right)
		node = parent;
	return parent;
}

struct rb_node *rb_prev(const struct rb_node *node)
{
	struct rb_node *parent;

	if (RB_EMPTY_NODE(node))
		return NULL;
	if (node->rb_left) {
		node = node->rb_left;
		while (node->rb_right)
			node = node->rb_right;
		return (struct rb_node *)node;
	}
	while ((parent = rb_parent(node)) && node == parent->rb_left)
		node = parent;
	return parent;
}

void rb_replace_node(struct rb_node *victim, struct rb_node *replacement, struct rb_root *root)
{
	struct rb_node *parent = rb_parent(victim);

	*replacement = *victim;
	if (victim->rb_left)
		set_parent(victim->rb_left, replacement);
	if (victim->rb_right)
		set_parent(victim->rb_right, replacement);
	change_child(victim, replacement, parent, root);
}

static struct rb_node *leftmost_leaf(const struct rb_node *node)
{
	for (;;) {
		if (node->rb_left)
			node = node->rb_left;
		else if (node->rb_right)
			node = node->rb_right;
		else
			return (struct rb_node *)node;
	}
}

struct rb_node *rb_first_postorder(const struct rb_root *root)
{
	return root->rb_node ? leftmost_leaf(root->rb_node) : NULL;
}

struct rb_node *rb_next_postorder(const struct rb_node *node)
{
	const struct rb_node *parent;

	if (!node)
		return NULL;
	parent = rb_parent(node);
	if (parent && node == parent->rb_left && parent->rb_right)
		return leftmost_leaf(parent->rb_right);
	return (struct rb_node *)parent;
}
