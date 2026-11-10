#ifndef _LINUX_TRACEPOINT_H
#define _LINUX_TRACEPOINT_H

/* No tracing: every trace_<event>() is an empty inline function. */
#define TP_PROTO(args...) args
#define TP_ARGS(args...) args
#define TP_STRUCT__entry(args...)
#define TP_fast_assign(args...)
#define TP_printk(args...)
#define TP_CONDITION(args...)
#define __AEROS_TRACE(name, proto) 	static inline void trace_##name(proto) { } 	static inline bool trace_##name##_enabled(void) { return false; }
#define TRACE_EVENT(name, proto, args, tstruct, assign, print) __AEROS_TRACE(name, PARAMS(proto))
#define TRACE_EVENT_CONDITION(name, proto, args, cond, tstruct, assign, print) __AEROS_TRACE(name, PARAMS(proto))
#define DECLARE_EVENT_CLASS(name, proto, args, tstruct, assign, print)
#define DEFINE_EVENT(template, name, proto, args) __AEROS_TRACE(name, PARAMS(proto))
#define DEFINE_EVENT_PRINT(template, name, proto, args, print) __AEROS_TRACE(name, PARAMS(proto))
#define DEFINE_EVENT_CONDITION(template, name, proto, args, cond) __AEROS_TRACE(name, PARAMS(proto))
#define DECLARE_TRACE(name, proto, args) __AEROS_TRACE(name, PARAMS(proto))
#define PARAMS(args...) args
#define TRACE_DEFINE_ENUM(x)
#define TRACE_DEFINE_SIZEOF(x)

#endif
