#ifndef _LINUX_DEBUGFS_H
#define _LINUX_DEBUGFS_H

#include <linux/types.h>
#include <linux/seq_file.h>

/* No debugfs: creation succeeds with a NULL dentry and nothing is shown. */
struct dentry;
struct file_operations;
struct device;
struct debugfs_reg32 {
	char *name;
	unsigned long offset;
};
struct debugfs_regset32 {
	const struct debugfs_reg32 *regs;
	int nregs;
	void __iomem *base;
	struct device *dev;
};

static inline struct dentry *debugfs_create_dir(const char *name, struct dentry *parent) { return NULL; }
static inline struct dentry *debugfs_create_file(const char *name, umode_t mode, struct dentry *parent,
						 void *data, const struct file_operations *fops) { return NULL; }
#define debugfs_create_file_size(name, mode, parent, data, fops, size) debugfs_create_file(name, mode, parent, data, fops)
static inline void debugfs_create_bool(const char *name, umode_t mode, struct dentry *parent, bool *value) { }
static inline void debugfs_create_u32(const char *name, umode_t mode, struct dentry *parent, u32 *value) { }
static inline void debugfs_create_u64(const char *name, umode_t mode, struct dentry *parent, u64 *value) { }
static inline void debugfs_create_x32(const char *name, umode_t mode, struct dentry *parent, u32 *value) { }
static inline void debugfs_create_ulong(const char *name, umode_t mode, struct dentry *parent, unsigned long *value) { }
static inline void debugfs_remove(struct dentry *dentry) { }
#define debugfs_remove_recursive debugfs_remove
static inline struct dentry *debugfs_lookup(const char *name, struct dentry *parent) { return NULL; }
static inline void debugfs_lookup_and_remove(const char *name, struct dentry *parent) { }
#define DEFINE_DEBUGFS_ATTRIBUTE(fops, get, set, fmt) \
	static __attribute__((unused)) const void *fops = (const void *)(get)

static inline void debugfs_create_atomic_t(const char *name, umode_t mode, struct dentry *parent, atomic_t *value) { }
static inline void debugfs_create_size_t(const char *name, umode_t mode, struct dentry *parent, size_t *value) { }
static inline void debugfs_create_u8(const char *name, umode_t mode, struct dentry *parent, u8 *value) { }
static inline void debugfs_create_x64(const char *name, umode_t mode, struct dentry *parent, u64 *value) { }
static inline struct dentry *debugfs_create_blob(const char *name, umode_t mode, struct dentry *parent, void *blob) { return NULL; }

#endif
