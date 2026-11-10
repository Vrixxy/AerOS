// SPDX-License-Identifier: MIT
/* The file-side pieces the DRM core expects: anonymous-inode files,
 * kthread workers on the shared work queue, and fence unwrapping. File
 * descriptors don't exist on the Linux side yet. */
#include <linux/kernel.h>
#include <linux/fs.h>
#include <linux/file.h>
#include <linux/anon_inodes.h>
#include <linux/kthread.h>
#include <linux/dma-fence-unwrap.h>

struct file *anon_inode_getfile(const char *name, const struct file_operations *fops, void *priv, int flags)
{
	struct file *file = kzalloc(sizeof(*file), GFP_KERNEL);

	(void)name;
	if (!file)
		return ERR_PTR(-ENOMEM);
	atomic_long_set(&file->f_count, 1);
	file->f_op = fops;
	file->private_data = priv;
	file->f_flags = (unsigned int)flags;
	return file;
}

struct inode *alloc_anon_inode(struct super_block *sb)
{
	struct inode *inode = kzalloc(sizeof(*inode), GFP_KERNEL);

	if (!inode)
		return ERR_PTR(-ENOMEM);
	inode->i_sb = sb;
	inode->i_mapping = &inode->i_data;
	address_space_init_once(&inode->i_data);
	atomic_set(&inode->i_count, 1);
	return inode;
}

struct file *fget(unsigned int fd)
{
	(void)fd;
	return NULL;
}

int get_unused_fd_flags(unsigned int flags)
{
	(void)flags;
	return -EMFILE;
}

void put_unused_fd(unsigned int fd)
{
	(void)fd;
}

void fd_install(unsigned int fd, struct file *file)
{
	(void)fd;
	(void)file;
}

void aeros_kthread_work_run(struct work_struct *work)
{
	struct kthread_work *kwork = container_of(work, struct kthread_work, base);

	kwork->func(kwork);
}

struct kthread_worker *kthread_create_worker(unsigned int flags, const char namefmt[], ...)
{
	struct kthread_worker *worker = kzalloc(sizeof(*worker), GFP_KERNEL);

	(void)flags;
	if (!worker)
		return ERR_PTR(-ENOMEM);
	worker->name = namefmt;
	return worker;
}

/* The fences under one chain link or array element, depth first. */
static struct dma_fence *unwrap_array(struct dma_fence_unwrap *cursor)
{
	cursor->array = dma_fence_chain_contained(cursor->chain);
	cursor->index = 0;
	return dma_fence_array_first(cursor->array);
}

struct dma_fence *dma_fence_unwrap_first(struct dma_fence *head, struct dma_fence_unwrap *cursor)
{
	cursor->chain = dma_fence_get(head);
	return unwrap_array(cursor);
}

struct dma_fence *dma_fence_unwrap_next(struct dma_fence_unwrap *cursor)
{
	struct dma_fence *fence = dma_fence_array_next(cursor->array, ++cursor->index);

	if (fence)
		return fence;
	cursor->chain = dma_fence_chain_walk(cursor->chain);
	return cursor->chain ? unwrap_array(cursor) : NULL;
}

/* All unsignalled leaf fences of the inputs, keeping the latest per
 * context, as one fence (an array when there are several). */
struct dma_fence *__dma_fence_unwrap_merge(unsigned int count, struct dma_fence **fences,
					   struct dma_fence_unwrap *cursors)
{
	struct dma_fence **leaves, *fence, *result;
	unsigned int total = 0, kept = 0, i, j;

	for (i = 0; i < count; i++)
		dma_fence_unwrap_for_each(fence, &cursors[i], fences[i])
			total++;
	leaves = kcalloc(total ? total : 1, sizeof(*leaves), GFP_KERNEL);
	if (!leaves)
		return NULL;
	for (i = 0; i < count; i++) {
		dma_fence_unwrap_for_each(fence, &cursors[i], fences[i]) {
			if (dma_fence_is_signaled(fence))
				continue;
			for (j = 0; j < kept; j++) {
				if (leaves[j]->context == fence->context) {
					if (dma_fence_is_later(fence, leaves[j])) {
						dma_fence_put(leaves[j]);
						leaves[j] = dma_fence_get(fence);
					}
					break;
				}
			}
			if (j == kept)
				leaves[kept++] = dma_fence_get(fence);
		}
	}
	if (kept == 0) {
		kfree(leaves);
		return dma_fence_get_stub();
	}
	if (kept == 1) {
		result = leaves[0];
		kfree(leaves);
		return result;
	}
	result = &dma_fence_array_create((int)kept, leaves, dma_fence_context_alloc(1), 1, false)->base;
	return result;
}

struct vfsmount aeros_pseudo_mount;
