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

#endif
