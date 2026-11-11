#ifndef _LINUX_PRINTK_H
#define _LINUX_PRINTK_H

#include <linux/types.h>

#define KERN_SOH "\001"
#define KERN_EMERG KERN_SOH "0"
#define KERN_ALERT KERN_SOH "1"
#define KERN_CRIT KERN_SOH "2"
#define KERN_ERR KERN_SOH "3"
#define KERN_WARNING KERN_SOH "4"
#define KERN_NOTICE KERN_SOH "5"
#define KERN_INFO KERN_SOH "6"
#define KERN_DEBUG KERN_SOH "7"

struct va_format {
	const char *fmt;
	va_list *va;
};

int printk(const char *format, ...) __attribute__((format(printf, 1, 2)));
int vprintk(const char *format, va_list args);
int snprintf(char *buffer, size_t size, const char *format, ...)
	__attribute__((format(printf, 3, 4)));
int vsnprintf(char *buffer, size_t size, const char *format, va_list args);
int scnprintf(char *buffer, size_t size, const char *format, ...)
	__attribute__((format(printf, 3, 4)));

#ifndef pr_fmt
#define pr_fmt(fmt) fmt
#endif
#define pr_err(fmt, ...) printk(KERN_ERR pr_fmt(fmt), ##__VA_ARGS__)
#define pr_warn(fmt, ...) printk(KERN_WARNING pr_fmt(fmt), ##__VA_ARGS__)
#define pr_notice(fmt, ...) printk(KERN_NOTICE pr_fmt(fmt), ##__VA_ARGS__)
#define pr_info(fmt, ...) printk(KERN_INFO pr_fmt(fmt), ##__VA_ARGS__)
#define pr_debug(fmt, ...) do { } while (0)

#define __aeros_printk_once(call) ({ static bool __done; if (!__done) { __done = true; call; } })
#define printk_once(fmt, ...) __aeros_printk_once(printk(fmt, ##__VA_ARGS__))
#define pr_err_once(fmt, ...) __aeros_printk_once(pr_err(fmt, ##__VA_ARGS__))
#define pr_warn_once(fmt, ...) __aeros_printk_once(pr_warn(fmt, ##__VA_ARGS__))
#define pr_info_once(fmt, ...) __aeros_printk_once(pr_info(fmt, ##__VA_ARGS__))
#define pr_notice_once(fmt, ...) __aeros_printk_once(pr_notice(fmt, ##__VA_ARGS__))
#define pr_debug_once(fmt, ...) do { } while (0)
#define pr_err_ratelimited pr_err
#define pr_warn_ratelimited pr_warn
#define pr_info_ratelimited pr_info
#define pr_debug_ratelimited pr_debug
#define pr_cont(fmt, ...) printk(fmt, ##__VA_ARGS__)
#define pr_emerg pr_err
#define pr_alert pr_err
#define pr_crit pr_err

char *kasprintf(gfp_t gfp, const char *format, ...) __attribute__((format(printf, 2, 3)));
char *kvasprintf(gfp_t gfp, const char *format, va_list args);

int sprintf(char *buffer, const char *format, ...) __attribute__((format(printf, 2, 3)));
enum { DUMP_PREFIX_NONE, DUMP_PREFIX_ADDRESS, DUMP_PREFIX_OFFSET };
void print_hex_dump(const char *level, const char *prefix, int prefix_type, int rowsize, int groupsize,
		    const void *buffer, size_t length, bool ascii);
#define print_hex_dump_debug(prefix, type, rowsize, groupsize, buffer, length, ascii) do { } while (0)
#define print_hex_dump_bytes(prefix, type, buffer, length) print_hex_dump(KERN_DEBUG, prefix, type, 16, 1, buffer, length, true)

#define HW_ERR "[Hardware Error]: "
#define FW_BUG "[Firmware Bug]: "
#define FW_WARN "[Firmware Warn]: "
#define FW_INFO "[Firmware Info]: "

#endif
