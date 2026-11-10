#ifndef _LINUX_DMA_BUF_H
#define _LINUX_DMA_BUF_H

#include <linux/kernel.h>
#include <linux/list.h>
#include <linux/dma-mapping.h>
#include <linux/dma-fence.h>

/* Buffer sharing between drivers. AerOS has one GPU driver and no other
 * importers yet, so this is the shape amdgpu/DRM PRIME compile against;
 * the export/import paths are not wired up. */
struct dma_buf;
struct dma_buf_attachment;
struct dma_resv;
struct sg_table;
struct vm_area_struct;
struct iosys_map;
struct module;

struct dma_buf_ops {
	bool cache_sgt_mapping;
	int (*attach)(struct dma_buf *dmabuf, struct dma_buf_attachment *attach);
	void (*detach)(struct dma_buf *dmabuf, struct dma_buf_attachment *attach);
	int (*pin)(struct dma_buf_attachment *attach);
	void (*unpin)(struct dma_buf_attachment *attach);
	struct sg_table *(*map_dma_buf)(struct dma_buf_attachment *attach, enum dma_data_direction direction);
	void (*unmap_dma_buf)(struct dma_buf_attachment *attach, struct sg_table *table,
			      enum dma_data_direction direction);
	void (*release)(struct dma_buf *dmabuf);
	int (*begin_cpu_access)(struct dma_buf *dmabuf, enum dma_data_direction direction);
	int (*end_cpu_access)(struct dma_buf *dmabuf, enum dma_data_direction direction);
	int (*mmap)(struct dma_buf *dmabuf, struct vm_area_struct *vma);
	int (*vmap)(struct dma_buf *dmabuf, struct iosys_map *map);
	void (*vunmap)(struct dma_buf *dmabuf, struct iosys_map *map);
};

struct dma_buf {
	size_t size;
	struct file *file;
	struct list_head attachments;
	const struct dma_buf_ops *ops;
	unsigned int vmapping_counter;
	const char *exp_name;
	const char *name;
	struct module *owner;
	void *priv;
	struct dma_resv *resv;
};

struct dma_buf_attach_ops {
	bool allow_peer2peer;
	void (*move_notify)(struct dma_buf_attachment *attach);
};

struct dma_buf_attachment {
	struct dma_buf *dmabuf;
	struct device *dev;
	struct list_head node;
	struct sg_table *sgt;
	enum dma_data_direction dir;
	bool peer2peer;
	const struct dma_buf_attach_ops *importer_ops;
	void *importer_priv;
	void *priv;
};

struct dma_buf_export_info {
	const char *exp_name;
	struct module *owner;
	const struct dma_buf_ops *ops;
	size_t size;
	int flags;
	struct dma_resv *resv;
	void *priv;
};
#define DEFINE_DMA_BUF_EXPORT_INFO(name) struct dma_buf_export_info name = { .exp_name = "aeros" }

static inline bool dma_buf_is_dynamic(struct dma_buf *dmabuf) { return dmabuf->ops->pin != NULL; }
static inline bool dma_buf_attachment_is_dynamic(struct dma_buf_attachment *attach)
{
	return attach->importer_ops != NULL;
}

struct dma_buf *dma_buf_export(const struct dma_buf_export_info *info);
struct dma_buf *dma_buf_get(int fd);
void dma_buf_put(struct dma_buf *dmabuf);
int dma_buf_fd(struct dma_buf *dmabuf, int flags);
struct dma_buf_attachment *dma_buf_attach(struct dma_buf *dmabuf, struct device *dev);
struct dma_buf_attachment *dma_buf_dynamic_attach(struct dma_buf *dmabuf, struct device *dev,
						  const struct dma_buf_attach_ops *importer_ops,
						  void *importer_priv);
void dma_buf_detach(struct dma_buf *dmabuf, struct dma_buf_attachment *attach);
int dma_buf_pin(struct dma_buf_attachment *attach);
void dma_buf_unpin(struct dma_buf_attachment *attach);
struct sg_table *dma_buf_map_attachment(struct dma_buf_attachment *attach, enum dma_data_direction direction);
struct sg_table *dma_buf_map_attachment_unlocked(struct dma_buf_attachment *attach,
						 enum dma_data_direction direction);
void dma_buf_unmap_attachment(struct dma_buf_attachment *attach, struct sg_table *table,
			      enum dma_data_direction direction);
void dma_buf_unmap_attachment_unlocked(struct dma_buf_attachment *attach, struct sg_table *table,
				       enum dma_data_direction direction);
void dma_buf_move_notify(struct dma_buf *dmabuf);
int dma_buf_vmap(struct dma_buf *dmabuf, struct iosys_map *map);
void dma_buf_vunmap(struct dma_buf *dmabuf, struct iosys_map *map);
int dma_buf_vmap_unlocked(struct dma_buf *dmabuf, struct iosys_map *map);
void dma_buf_vunmap_unlocked(struct dma_buf *dmabuf, struct iosys_map *map);
int dma_buf_mmap(struct dma_buf *dmabuf, struct vm_area_struct *vma, unsigned long pgoff);

#endif
