// SPDX-License-Identifier: MIT
/* Runs AerOS's own compute programs on a GFX9 Radeon (the laptop's Cezanne).
 * The command buffer is the shape amdgpu itself uses for its GPR workaround:
 * set the compute registers, point at the program, dispatch, then wait for
 * the ring's fence. Programs come from kernel/linux/gpu via
 * tools/gpu-kernels.py; they read their arguments through user SGPRs 4-5. */
#include <linux/kernel.h>
#include "amdgpu.h"
#include "soc15.h"
#include "soc15d.h"
#include "soc15_common.h"
#include "gc/gc_9_0_offset.h"
#include "gc/gc_9_0_sh_mask.h"
#include "aeros_gpu_kernels.h"

#define CODE_OFFSET 1024

/* Set once the self-test passed: the device glass is drawn on. */
static struct amdgpu_device *glass_device;

static void set_sh(struct amdgpu_ib *ib, u32 reg, u32 value)
{
	ib->ptr[ib->length_dw++] = PACKET3(PACKET3_SET_SH_REG, 1);
	ib->ptr[ib->length_dw++] = reg - PACKET3_SET_SH_REG_START;
	ib->ptr[ib->length_dw++] = value;
}

/* Runs `code` on groups_x x groups_y groups of 64 threads, its arguments at
 * GPU address `args`, and waits (up to half a second) for it to finish. */
int aeros_gpu_dispatch(struct amdgpu_device *adev, const u32 *code, unsigned int words,
		       u32 rsrc1, u32 rsrc2, u64 args, u32 groups_x, u32 groups_y)
{
	struct amdgpu_ring *ring = &adev->gfx.compute_ring[0];
	struct dma_fence *fence = NULL;
	struct amdgpu_ib ib;
	u64 program;
	long left;
	int r;

	if (!adev->gfx.num_compute_rings || !ring->sched.ready)
		return -ENODEV;
	memset(&ib, 0, sizeof(ib));
	r = amdgpu_ib_get(adev, NULL, CODE_OFFSET + ALIGN(words * 4, 256), AMDGPU_IB_POOL_DIRECT, &ib);
	if (r)
		return r;
	memcpy(ib.ptr + CODE_OFFSET / 4, code, words * 4);
	ib.length_dw = 0;
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_RESOURCE_LIMITS), 0);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_NUM_THREAD_X), 64);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_NUM_THREAD_Y), 1);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_NUM_THREAD_Z), 1);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_PGM_RSRC1), rsrc1);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_PGM_RSRC2), rsrc2);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_STATIC_THREAD_MGMT_SE0), 0xffffffff);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_STATIC_THREAD_MGMT_SE1), 0xffffffff);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_STATIC_THREAD_MGMT_SE2), 0xffffffff);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_STATIC_THREAD_MGMT_SE3), 0xffffffff);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_USER_DATA_0), 0);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_USER_DATA_1), 0);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_USER_DATA_2), 0);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_USER_DATA_3), 0);
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_USER_DATA_4), lower_32_bits(args));
	set_sh(&ib, SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_USER_DATA_5), upper_32_bits(args));
	program = (ib.gpu_addr + CODE_OFFSET) >> 8;
	ib.ptr[ib.length_dw++] = PACKET3(PACKET3_SET_SH_REG, 2);
	ib.ptr[ib.length_dw++] = SOC15_REG_OFFSET(GC, 0, mmCOMPUTE_PGM_LO) - PACKET3_SET_SH_REG_START;
	ib.ptr[ib.length_dw++] = lower_32_bits(program);
	ib.ptr[ib.length_dw++] = upper_32_bits(program);
	ib.ptr[ib.length_dw++] = PACKET3(PACKET3_DISPATCH_DIRECT, 3);
	ib.ptr[ib.length_dw++] = groups_x;
	ib.ptr[ib.length_dw++] = groups_y;
	ib.ptr[ib.length_dw++] = 1;
	ib.ptr[ib.length_dw++] = REG_SET_FIELD(0, COMPUTE_DISPATCH_INITIATOR, COMPUTE_SHADER_EN, 1);
	ib.ptr[ib.length_dw++] = PACKET3(PACKET3_EVENT_WRITE, 0);
	ib.ptr[ib.length_dw++] = EVENT_TYPE(7) | EVENT_INDEX(4);
	r = amdgpu_ib_schedule(ring, 1, &ib, NULL, &fence);
	if (!r) {
		left = dma_fence_wait_timeout(fence, false, msecs_to_jiffies(500));
		r = left > 0 ? 0 : left == 0 ? -ETIMEDOUT : (int)left;
	}
	amdgpu_ib_free(adev, &ib, NULL);
	dma_fence_put(fence);
	return r;
}

/* Proves the path on real hardware: 256 threads each write one value into a
 * buffer in system memory, and every value is checked. */
void aeros_gpu_selftest(struct drm_device *ddev)
{
	struct amdgpu_device *adev = drm_to_adev(ddev);
	struct amdgpu_bo *bo = NULL;
	u64 gpu = 0;
	u32 *cpu = NULL;
	int r, good = 0;

	r = amdgpu_bo_create_kernel(adev, PAGE_SIZE, PAGE_SIZE, AMDGPU_GEM_DOMAIN_GTT, &bo, &gpu, (void **)&cpu);
	if (r) {
		pr_err("gpu compute: no buffer (%d)\n", r);
		return;
	}
	memset(cpu, 0, PAGE_SIZE);
	/* Arguments at 0 (output pointer, value), output from byte 256. */
	*(u64 *)cpu = gpu + 256;
	cpu[2] = 0x1000;
	mb();
	r = aeros_gpu_dispatch(adev, aeros_gpu_fill_code, ARRAY_SIZE(aeros_gpu_fill_code),
			       AEROS_GPU_FILL_RSRC1, AEROS_GPU_FILL_RSRC2, gpu, 4, 1);
	mb();
	for (int i = 0; i < 256; i++)
		good += cpu[64 + i] == 0x1000u + i;
	pr_info("gpu compute: dispatch %d, %d/256 values right (first %#x, last %#x)\n", r, good,
		cpu[64], cpu[64 + 255]);
	if (!r && good == 256)
		glass_device = adev;
	amdgpu_bo_free_kernel(&bo, &gpu, (void **)&cpu);
}

/* The glass job (kernel/linux/gpu/glass.c's struct glass_args, GlassArgs in
 * linux.rs): two blur passes each way between buffers A and B, then the
 * glass from A into C. */
struct aeros_glass_args {
	u64 in, out;
	s32 width, height, radius;
	s32 left, top;
	s32 ax, ay, aw, ah;
	s32 sx, sy, sw, sh;
	s32 r0, r1, r2, r3;
	s32 splay, bend, spread, gain, lift, rim, shade, light_x, light_y, intensity;
	s32 tint_r, tint_g, tint_b, tint_a, saturation, brightness, noise_alpha;
	u32 seed;
	s32 crystal_r, crystal_g, crystal_b, crystal_a;
};

#define GLASS_PIXELS (3200u * 2048u)

static struct amdgpu_bo *glass_bo[4];
static u64 glass_gpu[4];
static void *glass_cpu[4];

/* The source and result buffers' CPU addresses (allocated on first use);
 * returns how many pixels each holds, 0 when the GPU can't do glass. */
unsigned int aeros_gpu_glass_buffers(u32 **source, u32 **result)
{
	if (!glass_device)
		return 0;
	for (int i = 0; i < 4; i++) {
		if (glass_bo[i])
			continue;
		size_t size = i == 3 ? PAGE_SIZE : GLASS_PIXELS * 4;
		int r = amdgpu_bo_create_kernel(glass_device, size, PAGE_SIZE, AMDGPU_GEM_DOMAIN_GTT,
						&glass_bo[i], &glass_gpu[i], &glass_cpu[i]);
		if (r) {
			pr_err("gpu glass: no buffer %d (%d)\n", i, r);
			glass_device = NULL;
			return 0;
		}
	}
	*source = glass_cpu[0];
	*result = glass_cpu[2];
	return GLASS_PIXELS;
}

int aeros_gpu_glass(const struct aeros_glass_args *job)
{
	static const int steps[5][3] = { { 0, 1, 0 }, { 1, 0, 0 }, { 0, 1, 1 }, { 1, 0, 1 }, { 0, 2, 2 } };
	struct amdgpu_device *adev = glass_device;
	int r = 0;

	if (!adev || !glass_bo[3])
		return -ENODEV;
	for (int step = job->radius > 0 ? 0 : 4; step < 5 && !r; step++) {
		struct aeros_glass_args *args = (void *)((u8 *)glass_cpu[3] + step * 256);
		unsigned int threads = steps[step][2] == 2 ? job->aw * job->ah : job->width * job->height;
		const u32 *code;
		unsigned int words;
		u32 rsrc1;

		*args = *job;
		args->in = glass_gpu[steps[step][0]];
		args->out = glass_gpu[steps[step][1]];
		mb();
		if (steps[step][2] == 0) {
			code = aeros_gpu_hblur_code, words = ARRAY_SIZE(aeros_gpu_hblur_code), rsrc1 = AEROS_GPU_HBLUR_RSRC1;
		} else if (steps[step][2] == 1) {
			code = aeros_gpu_vblur_code, words = ARRAY_SIZE(aeros_gpu_vblur_code), rsrc1 = AEROS_GPU_VBLUR_RSRC1;
		} else {
			code = aeros_gpu_glass_code, words = ARRAY_SIZE(aeros_gpu_glass_code), rsrc1 = AEROS_GPU_GLASS_RSRC1;
		}
		r = aeros_gpu_dispatch(adev, code, words, rsrc1, AEROS_GPU_GLASS_RSRC2,
				       glass_gpu[3] + step * 256, DIV_ROUND_UP(threads, 64), 1);
	}
	mb();
	if (r) {
		pr_err("gpu glass: failed (%d), back to the CPU\n", r);
		glass_device = NULL;
	}
	return r;
}
