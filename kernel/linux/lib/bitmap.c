#include <linux/bitops.h>

unsigned long find_next_bit(const unsigned long *addr, unsigned long size, unsigned long offset)
{
	for (; offset < size; offset++)
		if (test_bit((long)offset, addr))
			return offset;
	return size;
}

unsigned long find_next_zero_bit(const unsigned long *addr, unsigned long size, unsigned long offset)
{
	for (; offset < size; offset++)
		if (!test_bit((long)offset, addr))
			return offset;
	return size;
}
