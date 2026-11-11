// SPDX-License-Identifier: MIT
/* Buffer sharing (dma-buf) for PRIME. A dma_buf lives on an anonymous file;
 * its last reference releases the exporter. There are no file descriptors
 * on AerOS's Linux side, so import by fd is not available. */
#include <linux/kernel.h>
#include <linux/dma-buf.h>
#include <linux/dma-resv.h>
#include <linux/anon_inodes.h>
#include <linux/fs.h>

struct dma_buf_with_resv {
	struct dma_buf buf;
	struct dma_resv resv;
};

static int dma_buf_file_release(struct inode *inode, struct file *file)
{
	struct dma_buf *dmabuf = file->private_data;
	struct dma_buf_with_resv *whole = container_of(dmabuf, struct dma_buf_with_resv, buf);

	(void)inode;
	WARN_ON(!list_empty(&dmabuf->attachments));
	dmabuf->ops->release(dmabuf);
	if (dmabuf->resv == &whole->resv)
		dma_resv_fini(&whole->resv);
	kfree(whole);
	return 0;
}

static const struct file_operations dma_buf_fops = {
	.release = dma_buf_file_release,
};

struct dma_buf *dma_buf_export(const struct dma_buf_export_info *info)
{
	struct dma_buf_with_resv *whole;
	struct dma_buf *dmabuf;
	struct file *file;

	if (WARN_ON(!info || !info->ops || !info->ops->map_dma_buf || !info->ops->unmap_dma_buf ||
		    !info->ops->release))
		return ERR_PTR(-EINVAL);
	whole = kzalloc(sizeof(*whole), GFP_KERNEL);
	if (!whole)
		return ERR_PTR(-ENOMEM);
	dmabuf = &whole->buf;
	dmabuf->priv = info->priv;
	dmabuf->ops = info->ops;
	dmabuf->size = info->size;
	dmabuf->exp_name = info->exp_name;
	dmabuf->owner = info->owner;
	INIT_LIST_HEAD(&dmabuf->attachments);
	if (info->resv) {
		dmabuf->resv = info->resv;
	} else {
		dma_resv_init(&whole->resv);
		dmabuf->resv = &whole->resv;
	}
	file = anon_inode_getfile("dmabuf", &dma_buf_fops, dmabuf, info->flags);
	if (IS_ERR(file)) {
		if (dmabuf->resv == &whole->resv)
			dma_resv_fini(&whole->resv);
		kfree(whole);
		return ERR_CAST(file);
	}
	dmabuf->file = file;
	return dmabuf;
}

struct dma_buf *dma_buf_get(int fd)
{
	(void)fd;
	return ERR_PTR(-EBADF);
}

void dma_buf_put(struct dma_buf *dmabuf)
{
	if (WARN_ON(!dmabuf || !dmabuf->file))
		return;
	fput(dmabuf->file);
}

int dma_buf_fd(struct dma_buf *dmabuf, int flags)
{
	(void)dmabuf;
	(void)flags;
	return -EMFILE;
}

struct dma_buf_attachment *dma_buf_dynamic_attach(struct dma_buf *dmabuf, struct device *dev,
						  const struct dma_buf_attach_ops *importer_ops,
						  void *importer_priv)
{
	struct dma_buf_attachment *attach;
	int ret;

	if (WARN_ON(!dmabuf || !dev))
		return ERR_PTR(-EINVAL);
	attach = kzalloc(sizeof(*attach), GFP_KERNEL);
	if (!attach)
		return ERR_PTR(-ENOMEM);
	attach->dev = dev;
	attach->dmabuf = dmabuf;
	attach->importer_ops = importer_ops;
	attach->importer_priv = importer_priv;
	attach->peer2peer = importer_ops && importer_ops->allow_peer2peer;
	if (dmabuf->ops->attach) {
		ret = dmabuf->ops->attach(dmabuf, attach);
		if (ret) {
			kfree(attach);
			return ERR_PTR(ret);
		}
	}
	dma_resv_lock(dmabuf->resv, NULL);
	list_add(&attach->node, &dmabuf->attachments);
	dma_resv_unlock(dmabuf->resv);
	return attach;
}

struct dma_buf_attachment *dma_buf_attach(struct dma_buf *dmabuf, struct device *dev)
{
	return dma_buf_dynamic_attach(dmabuf, dev, NULL, NULL);
}

void dma_buf_detach(struct dma_buf *dmabuf, struct dma_buf_attachment *attach)
{
	if (WARN_ON(!dmabuf || !attach || dmabuf != attach->dmabuf))
		return;
	dma_resv_lock(dmabuf->resv, NULL);
	if (attach->sgt) {
		dmabuf->ops->unmap_dma_buf(attach, attach->sgt, attach->dir);
		if (dma_buf_is_dynamic(dmabuf) && !dma_buf_attachment_is_dynamic(attach))
			dmabuf->ops->unpin(attach);
	}
	list_del(&attach->node);
	dma_resv_unlock(dmabuf->resv);
	if (dmabuf->ops->detach)
		dmabuf->ops->detach(dmabuf, attach);
	kfree(attach);
}

int dma_buf_pin(struct dma_buf_attachment *attach)
{
	struct dma_buf *dmabuf = attach->dmabuf;

	dma_resv_assert_held(dmabuf->resv);
	return dmabuf->ops->pin ? dmabuf->ops->pin(attach) : 0;
}

void dma_buf_unpin(struct dma_buf_attachment *attach)
{
	struct dma_buf *dmabuf = attach->dmabuf;

	dma_resv_assert_held(dmabuf->resv);
	if (dmabuf->ops->unpin)
		dmabuf->ops->unpin(attach);
}

struct sg_table *dma_buf_map_attachment(struct dma_buf_attachment *attach, enum dma_data_direction direction)
{
	struct dma_buf *dmabuf = attach->dmabuf;
	struct sg_table *table;
	int ret;

	dma_resv_assert_held(dmabuf->resv);
	if (attach->sgt) {
		if (attach->dir != direction && attach->dir != DMA_BIDIRECTIONAL)
			return ERR_PTR(-EBUSY);
		return attach->sgt;
	}
	if (dma_buf_is_dynamic(dmabuf) && !dma_buf_attachment_is_dynamic(attach)) {
		ret = dmabuf->ops->pin(attach);
		if (ret)
			return ERR_PTR(ret);
	}
	table = dmabuf->ops->map_dma_buf(attach, direction);
	if (!table)
		table = ERR_PTR(-ENOMEM);
	if (IS_ERR(table)) {
		if (dma_buf_is_dynamic(dmabuf) && !dma_buf_attachment_is_dynamic(attach))
			dmabuf->ops->unpin(attach);
		return table;
	}
	if (dmabuf->ops->cache_sgt_mapping) {
		attach->sgt = table;
		attach->dir = direction;
	}
	return table;
}

struct sg_table *dma_buf_map_attachment_unlocked(struct dma_buf_attachment *attach,
						 enum dma_data_direction direction)
{
	struct sg_table *table;

	dma_resv_lock(attach->dmabuf->resv, NULL);
	table = dma_buf_map_attachment(attach, direction);
	dma_resv_unlock(attach->dmabuf->resv);
	return table;
}

void dma_buf_unmap_attachment(struct dma_buf_attachment *attach, struct sg_table *table,
			      enum dma_data_direction direction)
{
	struct dma_buf *dmabuf = attach->dmabuf;

	dma_resv_assert_held(dmabuf->resv);
	if (attach->sgt == table)
		return;
	dmabuf->ops->unmap_dma_buf(attach, table, direction);
	if (dma_buf_is_dynamic(dmabuf) && !dma_buf_attachment_is_dynamic(attach))
		dmabuf->ops->unpin(attach);
}

void dma_buf_unmap_attachment_unlocked(struct dma_buf_attachment *attach, struct sg_table *table,
				       enum dma_data_direction direction)
{
	dma_resv_lock(attach->dmabuf->resv, NULL);
	dma_buf_unmap_attachment(attach, table, direction);
	dma_resv_unlock(attach->dmabuf->resv);
}

void dma_buf_move_notify(struct dma_buf *dmabuf)
{
	struct dma_buf_attachment *attach;

	dma_resv_assert_held(dmabuf->resv);
	list_for_each_entry(attach, &dmabuf->attachments, node)
		if (attach->importer_ops && attach->importer_ops->move_notify)
			attach->importer_ops->move_notify(attach);
}

int dma_buf_vmap(struct dma_buf *dmabuf, struct iosys_map *map)
{
	int ret;

	dma_resv_assert_held(dmabuf->resv);
	if (!dmabuf->ops->vmap)
		return -EINVAL;
	ret = dmabuf->ops->vmap(dmabuf, map);
	if (!ret)
		dmabuf->vmapping_counter++;
	return ret;
}

void dma_buf_vunmap(struct dma_buf *dmabuf, struct iosys_map *map)
{
	dma_resv_assert_held(dmabuf->resv);
	if (WARN_ON(!dmabuf->vmapping_counter))
		return;
	dmabuf->vmapping_counter--;
	if (dmabuf->ops->vunmap)
		dmabuf->ops->vunmap(dmabuf, map);
}

int dma_buf_vmap_unlocked(struct dma_buf *dmabuf, struct iosys_map *map)
{
	int ret;

	dma_resv_lock(dmabuf->resv, NULL);
	ret = dma_buf_vmap(dmabuf, map);
	dma_resv_unlock(dmabuf->resv);
	return ret;
}

void dma_buf_vunmap_unlocked(struct dma_buf *dmabuf, struct iosys_map *map)
{
	dma_resv_lock(dmabuf->resv, NULL);
	dma_buf_vunmap(dmabuf, map);
	dma_resv_unlock(dmabuf->resv);
}

int dma_buf_mmap(struct dma_buf *dmabuf, struct vm_area_struct *vma, unsigned long pgoff)
{
	(void)vma;
	(void)pgoff;
	if (!dmabuf->ops->mmap)
		return -EINVAL;
	return -EOPNOTSUPP;
}
