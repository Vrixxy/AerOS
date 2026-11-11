#ifndef _LINUX_OF_H
#define _LINUX_OF_H
#include <linux/property.h>
/* No device tree on PCs. */
struct device_node {
	const char *name;
	struct fwnode_handle fwnode;
};
#define of_node_put(node) do { (void)(node); } while (0)
#define of_node_get(node) (node)
#define of_fwnode_handle(node) ((struct fwnode_handle *)NULL)
static inline bool of_device_is_available(const struct device_node *node) { (void)node; return false; }
static inline int of_property_read_u32(const struct device_node *node, const char *name, u32 *value)
{
	(void)node; (void)name; (void)value;
	return -EINVAL;
}
#endif
