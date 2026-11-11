#ifndef _LINUX_UNALIGNED_H
#define _LINUX_UNALIGNED_H

#include <linux/byteorder/generic.h>

#define __aeros_get_unaligned(type, p) ({ type __v; __builtin_memcpy(&__v, (p), sizeof(__v)); __v; })
#define __aeros_put_unaligned(type, v, p) do { type __v = (v); __builtin_memcpy((p), &__v, sizeof(__v)); } while (0)
#define get_unaligned(p) __aeros_get_unaligned(__typeof__(*(p)), (p))
#define put_unaligned(v, p) __aeros_put_unaligned(__typeof__(*(p)), (v), (p))
#define get_unaligned_le16(p) __aeros_get_unaligned(u16, (p))
#define get_unaligned_le32(p) __aeros_get_unaligned(u32, (p))
#define get_unaligned_le64(p) __aeros_get_unaligned(u64, (p))
#define get_unaligned_be16(p) be16_to_cpu(__aeros_get_unaligned(u16, (p)))
#define get_unaligned_be32(p) be32_to_cpu(__aeros_get_unaligned(u32, (p)))
#define get_unaligned_be64(p) be64_to_cpu(__aeros_get_unaligned(u64, (p)))
#define put_unaligned_le16(v, p) __aeros_put_unaligned(u16, (v), (p))
#define put_unaligned_le32(v, p) __aeros_put_unaligned(u32, (v), (p))
#define put_unaligned_le64(v, p) __aeros_put_unaligned(u64, (v), (p))
#define put_unaligned_be16(v, p) __aeros_put_unaligned(u16, cpu_to_be16(v), (p))
#define put_unaligned_be32(v, p) __aeros_put_unaligned(u32, cpu_to_be32(v), (p))
static inline u32 get_unaligned_le24(const void *p)
{
	const u8 *b = p;

	return b[0] | (u32)b[1] << 8 | (u32)b[2] << 16;
}

#endif
