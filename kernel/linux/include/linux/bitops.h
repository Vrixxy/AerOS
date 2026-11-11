#ifndef _LINUX_BITOPS_H
#define _LINUX_BITOPS_H

#include <linux/types.h>

#define BITS_PER_LONG 64
#define BITS_PER_BYTE 8
#define BITS_PER_TYPE(type) (sizeof(type) * BITS_PER_BYTE)
#define BIT_WORD(nr) ((nr) / BITS_PER_LONG)
#define BIT_MASK(nr) (1UL << ((nr) % BITS_PER_LONG))
#define BITS_TO_LONGS(nr) (((nr) + BITS_PER_LONG - 1) / BITS_PER_LONG)
#define DECLARE_BITMAP(name, bits) unsigned long name[BITS_TO_LONGS(bits)]

static inline bool test_bit(long nr, const volatile unsigned long *addr)
{
	return (addr[BIT_WORD(nr)] & BIT_MASK(nr)) != 0;
}
static inline void set_bit(long nr, volatile unsigned long *addr)
{
	__atomic_fetch_or(&addr[BIT_WORD(nr)], BIT_MASK(nr), __ATOMIC_SEQ_CST);
}
static inline void clear_bit(long nr, volatile unsigned long *addr)
{
	__atomic_fetch_and(&addr[BIT_WORD(nr)], ~BIT_MASK(nr), __ATOMIC_SEQ_CST);
}
static inline bool test_and_set_bit(long nr, volatile unsigned long *addr)
{
	return (__atomic_fetch_or(&addr[BIT_WORD(nr)], BIT_MASK(nr), __ATOMIC_SEQ_CST) & BIT_MASK(nr)) != 0;
}
static inline bool test_and_clear_bit(long nr, volatile unsigned long *addr)
{
	return (__atomic_fetch_and(&addr[BIT_WORD(nr)], ~BIT_MASK(nr), __ATOMIC_SEQ_CST) & BIT_MASK(nr)) != 0;
}
#define clear_bit_unlock clear_bit
#define test_and_set_bit_lock test_and_set_bit
static inline void __set_bit(long nr, volatile unsigned long *addr) { addr[BIT_WORD(nr)] |= BIT_MASK(nr); }
static inline void __clear_bit(long nr, volatile unsigned long *addr) { addr[BIT_WORD(nr)] &= ~BIT_MASK(nr); }

static inline unsigned long __ffs(unsigned long word) { return (unsigned long)__builtin_ctzl(word); }
static inline unsigned long __fls(unsigned long word) { return 63 - (unsigned long)__builtin_clzl(word); }
static inline int fls(unsigned int x) { return x ? 32 - __builtin_clz(x) : 0; }
static inline int fls64(u64 x) { return x ? 64 - __builtin_clzll(x) : 0; }
static inline int ffs(int x) { return __builtin_ffs(x); }
#define hweight32(w) __builtin_popcount(w)
#define hweight64(w) __builtin_popcountll(w)
#define hweight_long(w) __builtin_popcountl(w)
static inline unsigned long ilog2_u64(u64 n) { return (unsigned long)fls64(n) - 1; }
#define ilog2(n) ((int)ilog2_u64((u64)(n)))
#define is_power_of_2(n) ((n) != 0 && (((n) & ((n) - 1)) == 0))
#define roundup_pow_of_two(n) (1UL << fls64((u64)(n) - 1))
#define rounddown_pow_of_two(n) (1UL << (fls64((u64)(n)) - 1))

unsigned long find_next_bit(const unsigned long *addr, unsigned long size, unsigned long offset);
unsigned long find_next_zero_bit(const unsigned long *addr, unsigned long size, unsigned long offset);
#define find_first_bit(addr, size) find_next_bit(addr, size, 0)
#define find_first_zero_bit(addr, size) find_next_zero_bit(addr, size, 0)
#define for_each_set_bit(bit, addr, size) \
	for ((bit) = find_first_bit((addr), (size)); (bit) < (size); \
	     (bit) = find_next_bit((addr), (size), (bit) + 1))

#define hweight8(w) __builtin_popcount((u8)(w))
#define hweight16(w) __builtin_popcount((u16)(w))
#define order_base_2(n) ((n) > 1 ? ilog2((n) - 1) + 1 : 0)

#define ffz(x) __ffs(~(unsigned long)(x))

#endif
