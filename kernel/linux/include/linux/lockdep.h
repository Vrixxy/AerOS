#ifndef _LINUX_LOCKDEP_H
#define _LINUX_LOCKDEP_H

struct lock_class_key {
	int unused;
};
struct lockdep_map {
	const char *name;
};
#define lockdep_is_held(lock) 1
#define lockdep_assert(condition) do { } while (0)
#define lockdep_assert_once(condition) do { } while (0)
#define lockdep_assert_held_once(lock) do { (void)(lock); } while (0)
#define lockdep_assert_none_held_once() do { } while (0)
#define lockdep_assert_not_held(lock) do { (void)(lock); } while (0)
#define lockdep_set_class(lock, key) do { } while (0)
#define lockdep_set_class_and_name(lock, key, name) do { } while (0)
#define lock_map_acquire(map) do { } while (0)
#define lock_map_release(map) do { } while (0)
#define might_lock(lock) do { } while (0)
#define lockdep_init_map(map, name, key, sub) do { } while (0)
#define STATIC_LOCKDEP_MAP_INIT(name, key) { (name) }

#endif
