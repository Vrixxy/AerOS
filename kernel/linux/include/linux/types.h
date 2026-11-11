#ifndef _LINUX_TYPES_H
#define _LINUX_TYPES_H

#include <stddef.h>
#include <stdbool.h>
#include <stdarg.h>
#include <linux/kconfig.h>

struct seq_file;
struct device;
struct module;
struct file;
struct inode;
struct dentry;
struct page;
struct vm_area_struct;
struct sg_table;
struct drm_printer;
struct device_node;
struct fwnode_handle;
struct dma_buf;
struct dma_buf_attachment;
struct iosys_map;
struct folio;

/* drm-kmod's BSD headers declare FreeBSD malloc types. */
#define MALLOC_DECLARE(type) extern int aeros_malloc_type_unused
#define MALLOC_DEFINE(type, shortdesc, longdesc) int aeros_malloc_type_unused

typedef signed char s8;
typedef unsigned char u8;
typedef short s16;
typedef unsigned short u16;
typedef int s32;
typedef unsigned int u32;
typedef long long s64;
typedef unsigned long long u64;

typedef s8 __s8;
typedef u8 __u8;
typedef s16 __s16;
typedef u16 __u16;
typedef s32 __s32;
typedef u32 __u32;
typedef s64 __s64;
typedef u64 __u64;
typedef u16 __le16;
typedef u32 __le32;
typedef u64 __le64;
typedef u16 __be16;
typedef u32 __be32;
typedef u64 __be64;

typedef long ssize_t;
typedef unsigned long __kernel_size_t;
typedef long __kernel_ssize_t;
typedef long __kernel_long_t;
typedef unsigned long __kernel_ulong_t;
typedef int __kernel_pid_t;
typedef unsigned int __kernel_uid32_t;
typedef unsigned int __kernel_gid32_t;
typedef long long __kernel_loff_t;
typedef long __kernel_time_t;
typedef long long time64_t;
typedef unsigned short umode_t;
typedef unsigned int __poll_t;
typedef unsigned long pgoff_t;
typedef unsigned int fmode_t;
typedef u32 dev_t;
typedef s64 ktime_t;
typedef unsigned int uint;
typedef unsigned long ulong;
typedef unsigned short ushort;
typedef unsigned char u_char;
typedef long long loff_t;
typedef u64 phys_addr_t;
typedef u64 resource_size_t;
typedef u64 dma_addr_t;
typedef unsigned int gfp_t;
typedef int pid_t;
typedef long intptr_t;
typedef unsigned long uintptr_t;
typedef u8 uint8_t;
typedef u16 uint16_t;
typedef u32 uint32_t;
typedef u64 uint64_t;
typedef s8 int8_t;
typedef s16 int16_t;
typedef s32 int32_t;
typedef s64 int64_t;

typedef struct {
	int counter;
} atomic_t;

typedef struct {
	long long counter;
} atomic64_t;

struct list_head {
	struct list_head *next, *prev;
};

#define __iomem
#define __user
#define __force
#define __must_check __attribute__((warn_unused_result))
#define __init
#define __exit
#define __initdata
#define __maybe_unused __attribute__((unused))
#define __always_unused __attribute__((unused))
#define __packed __attribute__((packed))
#define __aligned(x) __attribute__((aligned(x)))
#define __always_inline inline __attribute__((always_inline))
#define noinline __attribute__((noinline))
#define likely(x) __builtin_expect(!!(x), 1)
#define unlikely(x) __builtin_expect(!!(x), 0)

typedef struct {
	__u8 b[16];
} guid_t;
typedef struct {
	__u8 b[16];
} uuid_t;
#include <linux/compiler.h>
#endif
