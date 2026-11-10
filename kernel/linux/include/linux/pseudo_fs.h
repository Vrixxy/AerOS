#ifndef _LINUX_PSEUDO_FS_H
#define _LINUX_PSEUDO_FS_H

#include <linux/fs.h>
struct pseudo_fs_context {
	const struct super_operations *ops;
	const struct dentry_operations *dops;
	unsigned long magic;
};
static inline struct pseudo_fs_context *init_pseudo(struct fs_context *fc, unsigned long magic)
{
	(void)fc; (void)magic;
	return NULL;
}

#endif
