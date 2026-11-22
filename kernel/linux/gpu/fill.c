/* GPU self-test: thread i writes value + i. Built by tools/gpu-kernels.py. */
typedef unsigned int u32;

__attribute__((amdgpu_kernel)) void fill(u32 *out, u32 value)
{
	u32 i = __builtin_amdgcn_workgroup_id_x() * 64 + __builtin_amdgcn_workitem_id_x();

	out[i] = value + i;
}
