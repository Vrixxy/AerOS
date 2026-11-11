#ifndef _LINUX_UUID_H
#define _LINUX_UUID_H

#include <linux/types.h>
#include <linux/string.h>

#define UUID_SIZE 16
static inline bool guid_equal(const guid_t *a, const guid_t *b) { return memcmp(a, b, sizeof(guid_t)) == 0; }
static inline void guid_copy(guid_t *dst, const guid_t *src) { memcpy(dst, src, sizeof(guid_t)); }
static inline bool guid_is_null(const guid_t *guid) { return mem_is_zero(guid, sizeof(guid_t)); }
static inline void import_guid(guid_t *dst, const __u8 *src) { memcpy(dst, src, sizeof(guid_t)); }
static inline void export_guid(__u8 *dst, const guid_t *src) { memcpy(dst, src, sizeof(guid_t)); }
static inline bool uuid_equal(const uuid_t *a, const uuid_t *b) { return memcmp(a, b, sizeof(uuid_t)) == 0; }
static inline void uuid_copy(uuid_t *dst, const uuid_t *src) { memcpy(dst, src, sizeof(uuid_t)); }
void guid_gen(guid_t *guid);

#endif
