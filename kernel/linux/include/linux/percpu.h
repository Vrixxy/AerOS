#ifndef _LINUX_PERCPU_H
#define _LINUX_PERCPU_H

/* The Linux side runs on one CPU: a per-CPU variable is a plain one. */
#define DEFINE_PER_CPU(type, name) type name
#define DECLARE_PER_CPU(type, name) extern type name
#define this_cpu_ptr(pointer) (pointer)
#define per_cpu_ptr(pointer, cpu) ((void)(cpu), (pointer))
#define per_cpu(name, cpu) (*((void)(cpu), &(name)))
#define __this_cpu_read(name) (name)
#define this_cpu_read(name) (name)
#define __this_cpu_write(name, value) ((name) = (value))
#define this_cpu_write(name, value) ((name) = (value))
#define __this_cpu_inc_return(name) (++(name))
#define __this_cpu_dec_return(name) (--(name))
#define this_cpu_inc(name) ((void)++(name))
#define this_cpu_dec(name) ((void)--(name))

#endif
