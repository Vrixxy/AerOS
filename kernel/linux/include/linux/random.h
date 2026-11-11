#ifndef _LINUX_RANDOM_H
#define _LINUX_RANDOM_H

#include <linux/types.h>

/* RDRAND when the CPU has it, else a xorshift mixed with the time stamp counter. */
u64 get_random_u64(void);
static inline u32 get_random_u32(void) { return (u32)get_random_u64(); }
static inline u16 get_random_u16(void) { return (u16)get_random_u64(); }
static inline u8 get_random_u8(void) { return (u8)get_random_u64(); }
static inline unsigned long get_random_long(void) { return (unsigned long)get_random_u64(); }
static inline u32 get_random_u32_below(u32 ceil) { return ceil ? (u32)((get_random_u32() * (u64)ceil) >> 32) : 0; }
static inline u32 get_random_u32_inclusive(u32 floor, u32 ceil) { return floor + get_random_u32_below(ceil - floor + 1); }
void get_random_bytes(void *buffer, size_t length);
#define prandom_u32_max get_random_u32_below

#endif
