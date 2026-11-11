#ifndef _LINUX_FS_H
#define _LINUX_FS_H

#include <linux/types.h>
#include <linux/stat.h>
#include <linux/pagemap.h>
#include <linux/file.h>
#include <linux/mount.h>
#include <linux/seq_file.h>
#include <linux/capability.h>
#include <linux/uaccess.h>
struct poll_table_struct;
struct fs_context;
struct vm_area_struct;
struct super_block;
struct kiocb;

struct inode {
	loff_t i_size;
	unsigned long i_ino;
	dev_t i_rdev;
	struct address_space *i_mapping;
	struct address_space i_data;
	void *i_private;
	struct super_block *i_sb;
	atomic_t i_count;
};
#define FOP_UNSIGNED_OFFSET 0x1u
struct file_operations {
	struct module *owner;
	unsigned int fop_flags;
	loff_t (*llseek)(struct file *file, loff_t offset, int whence);
	ssize_t (*read)(struct file *file, char __user *buf, size_t count, loff_t *pos);
	ssize_t (*write)(struct file *file, const char __user *buf, size_t count, loff_t *pos);
	__poll_t (*poll)(struct file *file, struct poll_table_struct *wait);
	long (*unlocked_ioctl)(struct file *file, unsigned int cmd, unsigned long arg);
	long (*compat_ioctl)(struct file *file, unsigned int cmd, unsigned long arg);
	int (*mmap)(struct file *file, struct vm_area_struct *vma);
	int (*open)(struct inode *inode, struct file *file);
	int (*flush)(struct file *file, void *id);
	int (*release)(struct inode *inode, struct file *file);
	int (*fasync)(int fd, struct file *file, int on);
	unsigned long (*get_unmapped_area)(struct file *file, unsigned long addr, unsigned long len,
					   unsigned long pgoff, unsigned long flags);
	void (*show_fdinfo)(struct seq_file *m, struct file *f);
};
struct file_system_type {
	const char *name;
	int (*init_fs_context)(struct fs_context *fc);
	void (*kill_sb)(struct super_block *sb);
	struct module *owner;
};
struct fs_context;

#define FMODE_READ ((fmode_t)0x1)
#define FMODE_WRITE ((fmode_t)0x2)
#define FMODE_LSEEK ((fmode_t)0x4)
#define FMODE_UNSIGNED_OFFSET ((fmode_t)0x2000)
#define O_ACCMODE 00000003
#define O_RDONLY 00000000
#define O_WRONLY 00000001
#define O_RDWR 00000002
#define O_NONBLOCK 00004000
#define O_CREAT 00000100
#define O_EXCL 00000200
#define O_TRUNC 00001000
#define O_APPEND 00002000
#define fops_get(fops) (fops)
#define fops_put(fops) do { (void)(fops); } while (0)
#define replace_fops(file, fops) ((file)->f_op = (fops))
static inline void kill_anon_super(struct super_block *sb) { (void)sb; }
static inline void kill_litter_super(struct super_block *sb) { (void)sb; }
#define O_CLOEXEC 02000000
#define MINORBITS 20
#define MINORMASK ((1U << MINORBITS) - 1)
#define MAJOR(dev) ((unsigned int)((dev) >> MINORBITS))
#define MINOR(dev) ((unsigned int)((dev) & MINORMASK))
#define MKDEV(ma, mi) (((ma) << MINORBITS) | (mi))
static inline unsigned int iminor(const struct inode *inode) { return MINOR(inode->i_rdev); }
static inline unsigned int imajor(const struct inode *inode) { return MAJOR(inode->i_rdev); }
static inline loff_t noop_llseek(struct file *file, loff_t offset, int whence) { (void)whence; (void)offset; (void)file; return 0; }
static inline int nonseekable_open(struct inode *inode, struct file *file) { (void)inode; (void)file; return 0; }
static inline void iput(struct inode *inode) { (void)inode; }
static inline struct inode *igrab(struct inode *inode) { return inode; }
struct inode *alloc_anon_inode(struct super_block *sb);
static inline int register_chrdev(unsigned int major, const char *name, const struct file_operations *fops)
{
	(void)name; (void)fops;
	return (int)major;
}
static inline void unregister_chrdev(unsigned int major, const char *name) { (void)major; (void)name; }
extern struct vfsmount aeros_pseudo_mount;
static inline int simple_pin_fs(struct file_system_type *type, struct vfsmount **mount, int *count)
{
	(void)type;
	*mount = &aeros_pseudo_mount;
	++*count;
	return 0;
}
static inline void simple_release_fs(struct vfsmount **mount, int *count) { (void)mount; (void)count; }
static inline bool capable(int cap) { (void)cap; return true; }
#define CAP_SYS_ADMIN 21
#define CAP_SYS_NICE 23

struct dentry {
	struct inode *d_inode;
};
static inline struct inode *d_inode(const struct dentry *dentry) { return dentry->d_inode; }
static inline loff_t default_llseek(struct file *file, loff_t offset, int whence)
{
	(void)file; (void)whence;
	return offset;
}
#define no_llseek NULL
#define noop_llseek default_llseek

typedef void *fl_owner_t;

#endif
