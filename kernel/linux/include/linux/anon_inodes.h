#ifndef _LINUX_ANON_INODES_H
#define _LINUX_ANON_INODES_H

#include <linux/fs.h>
struct file *anon_inode_getfile(const char *name, const struct file_operations *fops, void *priv, int flags);

#endif
