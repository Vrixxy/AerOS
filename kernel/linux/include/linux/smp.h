#ifndef _LINUX_SMP_H
#define _LINUX_SMP_H
#define smp_processor_id() 0
#define raw_smp_processor_id() 0
#define get_cpu() 0
#define put_cpu() do { } while (0)
#define num_online_cpus() 1U
#define num_possible_cpus() 1U
#define nr_cpu_ids 1U
#define on_each_cpu(func, info, wait) ((func)(info))
#endif
