#ifndef _LINUX_BYTEORDER_GENERIC_H
#define _LINUX_BYTEORDER_GENERIC_H

#include <linux/types.h>

/* x86 is little-endian. */
#define cpu_to_le16(x) ((__le16)(u16)(x))
#define cpu_to_le32(x) ((__le32)(u32)(x))
#define cpu_to_le64(x) ((__le64)(u64)(x))
#define le16_to_cpu(x) ((u16)(__le16)(x))
#define le32_to_cpu(x) ((u32)(__le32)(x))
#define le64_to_cpu(x) ((u64)(__le64)(x))
#define cpu_to_be16(x) ((__be16)__builtin_bswap16((u16)(x)))
#define cpu_to_be32(x) ((__be32)__builtin_bswap32((u32)(x)))
#define cpu_to_be64(x) ((__be64)__builtin_bswap64((u64)(x)))
#define be16_to_cpu(x) __builtin_bswap16((u16)(x))
#define be32_to_cpu(x) __builtin_bswap32((u32)(x))
#define be64_to_cpu(x) __builtin_bswap64((u64)(x))
#define le16_to_cpup(p) le16_to_cpu(*(p))
#define le32_to_cpup(p) le32_to_cpu(*(p))
#define le64_to_cpup(p) le64_to_cpu(*(p))
#define be16_to_cpup(p) be16_to_cpu(*(p))
#define be32_to_cpup(p) be32_to_cpu(*(p))
#define cpu_to_le32s(p) do { } while (0)
#define le32_to_cpus(p) do { } while (0)
#define le16_add_cpu(p, v) (*(p) = cpu_to_le16(le16_to_cpu(*(p)) + (v)))
#define le32_add_cpu(p, v) (*(p) = cpu_to_le32(le32_to_cpu(*(p)) + (v)))
#define swab16 __builtin_bswap16
#define swab32 __builtin_bswap32
#define swab64 __builtin_bswap64
#define __swab16 __builtin_bswap16
#define __swab32 __builtin_bswap32
#define __swab64 __builtin_bswap64
#define __cpu_to_le16 cpu_to_le16
#define __cpu_to_le32 cpu_to_le32
#define __le16_to_cpu le16_to_cpu
#define __le32_to_cpu le32_to_cpu
#define __cpu_to_be32 cpu_to_be32
#define __be32_to_cpu be32_to_cpu
#define __LITTLE_ENDIAN 1234
#define __LITTLE_ENDIAN_BITFIELD

#endif
