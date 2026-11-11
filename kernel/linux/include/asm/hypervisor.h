#ifndef _AEROS_ASM_HYPERVISOR_H
#define _AEROS_ASM_HYPERVISOR_H

enum x86_hypervisor_type { X86_HYPER_NATIVE = 0, X86_HYPER_VMWARE, X86_HYPER_MS_HYPERV, X86_HYPER_XEN_PV, X86_HYPER_XEN_HVM, X86_HYPER_KVM };
static inline bool hypervisor_is_type(enum x86_hypervisor_type type) { return type == X86_HYPER_NATIVE; }
#endif
