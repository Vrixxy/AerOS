#ifndef _LINUX_STRING_HELPERS_H
#define _LINUX_STRING_HELPERS_H

#include <linux/kernel.h>
static inline const char *str_yes_no(bool v) { return v ? "yes" : "no"; }
static inline const char *str_on_off(bool v) { return v ? "on" : "off"; }
static inline const char *str_enabled_disabled(bool v) { return v ? "enabled" : "disabled"; }
static inline const char *str_enable_disable(bool v) { return v ? "enable" : "disable"; }
static inline const char *str_true_false(bool v) { return v ? "true" : "false"; }
static inline const char *str_read_write(bool v) { return v ? "read" : "write"; }
static inline const char *str_plural(size_t n) { return n == 1 ? "" : "s"; }
enum string_size_units { STRING_UNITS_10, STRING_UNITS_2 };
static inline int string_get_size(u64 size, u64 blk_size, enum string_size_units units, char *buf, int len)
{
	(void)units;
	return snprintf(buf, len, "%llu B", (unsigned long long)(size * blk_size));
}

#endif
