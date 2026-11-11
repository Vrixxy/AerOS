#ifndef _LINUX_STRING_H
#define _LINUX_STRING_H

#include <linux/types.h>

void *memcpy(void *destination, const void *source, size_t length);
void *memmove(void *destination, const void *source, size_t length);
void *memset(void *destination, int value, size_t length);
int memcmp(const void *a, const void *b, size_t length);
size_t strlen(const char *text);
size_t strnlen(const char *text, size_t limit);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t limit);
char *strchr(const char *text, int c);
size_t strscpy(char *destination, const char *source, size_t size);
char *kstrdup(const char *text, gfp_t flags);

void *kmemdup(const void *source, size_t length, gfp_t gfp);
char *kstrndup(const char *text, size_t limit, gfp_t gfp);
#define kmemdup_array(source, count, size, gfp) kmemdup((source), (count) * (size), (gfp))

char *strcpy(char *destination, const char *source);
ssize_t strscpy_pad(char *destination, const char *source, size_t size);

#define strtomem_pad(dest, src, pad) do { \
	size_t __n = strnlen((src), sizeof(dest)); \
	memcpy((dest), (src), __n); \
	memset((char *)(dest) + __n, (pad), sizeof(dest) - __n); \
} while (0)
#define strtomem(dest, src) memcpy((dest), (src), strnlen((src), sizeof(dest)))
#define memcpy_and_pad(dest, dest_len, src, count, pad) do { \
	size_t __c = (count) < (dest_len) ? (count) : (dest_len); \
	memcpy((dest), (src), __c); \
	memset((char *)(dest) + __c, (pad), (dest_len) - __c); \
} while (0)

long simple_strtol(const char *string, char **end, unsigned int base);
unsigned long simple_strtoul(const char *string, char **end, unsigned int base);
unsigned long long simple_strtoull(const char *string, char **end, unsigned int base);
int kstrtoint(const char *string, unsigned int base, int *result);
int kstrtouint(const char *string, unsigned int base, unsigned int *result);
int kstrtoul(const char *string, unsigned int base, unsigned long *result);
int kstrtobool(const char *string, bool *result);
static inline char *strnchr(const char *string, size_t count, int c)
{
	for (; count-- && *string; string++)
		if (*string == (char)c)
			return (char *)string;
	return NULL;
}
static inline size_t str_has_prefix(const char *string, const char *prefix)
{
	size_t length = strlen(prefix);

	return strncmp(string, prefix, length) == 0 ? length : 0;
}

static inline bool mem_is_zero(const void *s, size_t n)
{
	const unsigned char *p = s;

	while (n--)
		if (*p++)
			return false;
	return true;
}
static inline void *memchr_inv(const void *s, int c, size_t n)
{
	const unsigned char *p = s;

	for (; n--; p++)
		if (*p != (unsigned char)c)
			return (void *)p;
	return NULL;
}

char *strnstr(const char *haystack, const char *needle, size_t length);
char *strsep(char **stringp, const char *delimiters);
int sscanf(const char *string, const char *format, ...) __attribute__((format(scanf, 2, 3)));
int vsscanf(const char *string, const char *format, va_list args);
int kstrtol(const char *string, unsigned int base, long *result);
int kstrtou8(const char *string, unsigned int base, u8 *result);
int kstrtou16(const char *string, unsigned int base, u16 *result);
int kstrtou32(const char *string, unsigned int base, u32 *result);
int kstrtoull(const char *string, unsigned int base, unsigned long long *result);
int kstrtoll(const char *string, unsigned int base, long long *result);
#define kstrtos32 kstrtoint
#define kstrtou64 kstrtoull
#define kstrtos64 kstrtoll

size_t strspn(const char *s, const char *accept);
size_t strcspn(const char *s, const char *reject);

char *strstr(const char *haystack, const char *needle);
int strncasecmp(const char *a, const char *b, size_t n);
int strcasecmp(const char *a, const char *b);
char *skip_spaces(const char *s);
char *strim(char *s);
static inline void *memset32(u32 *s, u32 v, size_t count)
{
	u32 *p = s;

	while (count--)
		*p++ = v;
	return s;
}
static inline void *memset64(u64 *s, u64 v, size_t count)
{
	u64 *p = s;

	while (count--)
		*p++ = v;
	return s;
}

size_t strlcat(char *dest, const char *src, size_t count);
size_t strlcpy(char *dest, const char *src, size_t size);

#endif
