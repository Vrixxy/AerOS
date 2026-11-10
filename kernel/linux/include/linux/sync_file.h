#ifndef _LINUX_SYNC_FILE_H
#define _LINUX_SYNC_FILE_H

#include <linux/dma-fence.h>
#include <linux/fs.h>
/* Sync files (fences as file descriptors) need AerOS file descriptors for
 * the Linux side; until then they can't be created or looked up. */
struct sync_file {
	struct file *file;
	struct dma_fence *fence;
};
struct sync_file *sync_file_create(struct dma_fence *fence);
struct dma_fence *sync_file_get_fence(int fd);

#endif
