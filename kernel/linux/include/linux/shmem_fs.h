#ifndef _LINUX_SHMEM_FS_H
#define _LINUX_SHMEM_FS_H

#include <linux/pagemap.h>
#include <linux/errno.h>

/* No swap: buffers can't be pushed out to shmem files. */
static inline struct file *shmem_file_setup(const char *name, loff_t size, unsigned long flags)
{
	(void)name; (void)size; (void)flags;
	return (struct file *)ERR_PTR(-ENOSYS);
}
static inline struct page *shmem_read_mapping_page_gfp(struct address_space *mapping, pgoff_t index, gfp_t gfp)
{
	(void)mapping; (void)index; (void)gfp;
	return (struct page *)ERR_PTR(-ENOSYS);
}
static inline struct folio *shmem_read_folio_gfp(struct address_space *mapping, pgoff_t index, gfp_t gfp)
{
	(void)mapping; (void)index; (void)gfp;
	return (struct folio *)ERR_PTR(-ENOSYS);
}
static inline bool shmem_file(struct file *file) { (void)file; return false; }

#endif
