#ifndef _AEROS_LINUX_HASHTABLE_H
#define _AEROS_LINUX_HASHTABLE_H

#include <linux/list.h>
#include <linux/hash.h>
#include <linux/log2.h>
#define DEFINE_HASHTABLE(name, bits) struct hlist_head name[1 << (bits)] = { [0 ... ((1 << (bits)) - 1)] = HLIST_HEAD_INIT }
#define DECLARE_HASHTABLE(name, bits) struct hlist_head name[1 << (bits)]
#define HASH_SIZE(name) (ARRAY_SIZE(name))
#define HASH_BITS(name) ilog2(HASH_SIZE(name))
#define hash_min(val, bits) (sizeof(val) <= 4 ? hash_32((u32)(val), bits) : hash_long(val, bits))
static inline void __hash_init(struct hlist_head *ht, unsigned int sz)
{
	unsigned int i;

	for (i = 0; i < sz; i++)
		INIT_HLIST_HEAD(&ht[i]);
}
#define hash_init(hashtable) __hash_init(hashtable, HASH_SIZE(hashtable))
#define hash_add(hashtable, node, key) hlist_add_head(node, &hashtable[hash_min(key, HASH_BITS(hashtable))])
#define hash_add_rcu hash_add
static inline bool hash_hashed(struct hlist_node *node) { return !hlist_unhashed(node); }
static inline bool __hash_empty(struct hlist_head *ht, unsigned int sz)
{
	unsigned int i;

	for (i = 0; i < sz; i++)
		if (!hlist_empty(&ht[i]))
			return false;
	return true;
}
#define hash_empty(hashtable) __hash_empty(hashtable, HASH_SIZE(hashtable))
static inline void hash_del(struct hlist_node *node) { hlist_del_init(node); }
#define hash_del_rcu hash_del
#define hash_for_each(name, bkt, obj, member) \
	for ((bkt) = 0, obj = NULL; obj == NULL && (bkt) < HASH_SIZE(name); (bkt)++) \
		hlist_for_each_entry(obj, &name[bkt], member)
#define hash_for_each_rcu hash_for_each
#define hash_for_each_safe(name, bkt, tmp, obj, member) \
	for ((bkt) = 0, obj = NULL; obj == NULL && (bkt) < HASH_SIZE(name); (bkt)++) \
		hlist_for_each_entry_safe(obj, tmp, &name[bkt], member)
#define hash_for_each_possible(name, obj, member, key) \
	hlist_for_each_entry(obj, &name[hash_min(key, HASH_BITS(name))], member)
#define hash_for_each_possible_rcu(name, obj, member, key, ...) hash_for_each_possible(name, obj, member, key)
#endif
