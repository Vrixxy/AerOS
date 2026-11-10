#ifndef _LINUX_FILE_H
#define _LINUX_FILE_H

#include <linux/types.h>
#include <linux/atomic.h>

struct file {
	atomic_long_t f_count;
	void *private_data;
	struct address_space *f_mapping;
	struct inode *f_inode;
	const struct file_operations *f_op;
	unsigned int f_flags;
	fmode_t f_mode;
};
void fput(struct file *file);
static inline struct file *get_file(struct file *file) { atomic_long_inc(&file->f_count); return file; }
static inline struct inode *file_inode(const struct file *file) { return file->f_inode; }
struct file *fget(unsigned int fd);
int get_unused_fd_flags(unsigned int flags);
void put_unused_fd(unsigned int fd);
void fd_install(unsigned int fd, struct file *file);

#include <linux/cleanup.h>
struct fd {
	struct file *file;
};
static inline struct fd fdget(unsigned int fd) { struct fd f = { fget(fd) }; return f; }
static inline void fdput(struct fd f) { if (f.file) fput(f.file); }
#define fd_file(f) ((f).file)
#define fd_empty(f) (!(f).file)
DEFINE_CLASS(fd, struct fd, fdput(_T), fdget(fd), int fd)

#endif
