#ifndef _LINUX_BITFIELD_H
#define _LINUX_BITFIELD_H

#include <linux/bits.h>

#define __bf_shf(x) (__builtin_ffsll(x) - 1)
#define FIELD_PREP(mask, val) (((__typeof__(mask))(val) << __bf_shf(mask)) & (mask))
#define FIELD_GET(mask, reg) ((__typeof__(mask))(((reg) & (mask)) >> __bf_shf(mask)))
#define FIELD_FIT(mask, val) (!((((__typeof__(mask))(val)) << __bf_shf(mask)) & ~(mask)))
#define FIELD_MAX(mask) ((__typeof__(mask))((mask) >> __bf_shf(mask)))
#define u32_encode_bits(v, field) FIELD_PREP(field, v)
#define u32_get_bits(v, field) FIELD_GET(field, v)
#define u64_encode_bits(v, field) FIELD_PREP(field, v)
#define u64_get_bits(v, field) FIELD_GET(field, v)
#define u16_encode_bits(v, field) FIELD_PREP(field, v)
#define u8_encode_bits(v, field) FIELD_PREP(field, v)

#endif
