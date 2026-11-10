#ifndef _LINUX_CLEANUP_H
#define _LINUX_CLEANUP_H

/* Scope-based cleanup: a variable whose destructor runs when it goes out
 * of scope (__attribute__((cleanup))). */
#define __aeros_paste2(a, b) a##b
#define __aeros_paste(a, b) __aeros_paste2(a, b)
#define __aeros_unique(prefix) __aeros_paste(prefix, __COUNTER__)

#define DEFINE_FREE(name, type, free_expr) \
	static inline void __free_##name(void *p) { type _T = *(type *)p; free_expr; }
#define __free(name) __attribute__((cleanup(__free_##name)))
#define no_free_ptr(p) ({ __typeof__(p) __ptr = (p); (p) = NULL; __ptr; })
#define return_ptr(p) return no_free_ptr(p)
#define retain_and_null_ptr(p) ((void)no_free_ptr(p))

#define DEFINE_CLASS(name, type, exit_expr, init_expr, init_args...) \
	typedef type class_##name##_t; \
	static inline void class_##name##_destructor(type *p) { type _T = *p; exit_expr; } \
	static inline type class_##name##_constructor(init_args) { type t = init_expr; return t; }
#define CLASS(name, var) \
	class_##name##_t var __attribute__((cleanup(class_##name##_destructor))) = class_##name##_constructor

#define DEFINE_GUARD(name, type, lock, unlock) \
	DEFINE_CLASS(name, type, if (_T) { unlock; }, ({ lock; _T; }), type _T)
#define guard(name) CLASS(name, __aeros_unique(__guard_))
#define scoped_guard(name, args...) \
	for (CLASS(name, scope)(args), *__done = NULL; !__done; __done = (void *)1)

#endif
