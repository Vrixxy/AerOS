/* The optical glass on the GPU: the same integer maths as aerui.rs's
 * optical_glass (box blur passes, then bend, dispersion, grade, tint and
 * light per pixel), so both paths draw the same pixels. One thread per
 * pixel; pixels are 0x00RRGGBB, results carry coverage in the top byte.
 * Built by tools/gpu-kernels.py; the argument block is aeros_glass_args in
 * aeros_gpu.c and GlassArgs in aerui.rs. */
typedef unsigned int u32;
typedef int i32;
typedef unsigned long long u64;
typedef long long i64;

struct glass_args {
	u32 *in;
	u32 *out;
	i32 width, height, radius;
	i32 left, top;
	i32 ax, ay, aw, ah;
	i32 sx, sy, sw, sh;
	i32 r0, r1, r2, r3;
	i32 splay, bend, spread, gain, lift, rim, shade, light_x, light_y, intensity;
	i32 tint_r, tint_g, tint_b, tint_a, saturation, brightness, noise_alpha;
	u32 seed;
	i32 crystal_r, crystal_g, crystal_b, crystal_a;
};

static inline u32 thread_id(void)
{
	return __builtin_amdgcn_workgroup_id_x() * 64 + __builtin_amdgcn_workitem_id_x();
}

static inline i32 min_i(i32 a, i32 b) { return a < b ? a : b; }
static inline i32 max_i(i32 a, i32 b) { return a > b ? a : b; }
static inline i32 clamp_i(i32 v, i32 lo, i32 hi) { return v < lo ? lo : v > hi ? hi : v; }

__attribute__((amdgpu_kernel)) void hblur(const struct glass_args a)
{
	u32 i = thread_id();
	if (i >= (u32)(a.width * a.height))
		return;
	i32 x = i % a.width, y = i / a.width;
	i32 start = max_i(x - a.radius, 0), end = min_i(x + a.radius, a.width - 1);
	u32 r = 0, g = 0, b = 0;
	for (i32 k = start; k <= end; k++) {
		u32 p = a.in[y * a.width + k];
		r += p >> 16 & 255;
		g += p >> 8 & 255;
		b += p & 255;
	}
	u32 n = end - start + 1;
	a.out[i] = (r / n) << 16 | (g / n) << 8 | (b / n);
}

__attribute__((amdgpu_kernel)) void vblur(const struct glass_args a)
{
	u32 i = thread_id();
	if (i >= (u32)(a.width * a.height))
		return;
	i32 x = i % a.width, y = i / a.width;
	i32 start = max_i(y - a.radius, 0), end = min_i(y + a.radius, a.height - 1);
	u32 r = 0, g = 0, b = 0;
	for (i32 k = start; k <= end; k++) {
		u32 p = a.in[k * a.width + x];
		r += p >> 16 & 255;
		g += p >> 8 & 255;
		b += p & 255;
	}
	u32 n = end - start + 1;
	a.out[i] = (r / n) << 16 | (g / n) << 8 | (b / n);
}

static inline i32 channel(u32 p, i32 c)
{
	return c == 0 ? (i32)(p >> 16 & 255) : c == 1 ? (i32)(p >> 8 & 255) : (i32)(p & 255);
}

static i32 bilinear(const u32 *in, i32 width, i32 height, i32 x, i32 y, i32 c)
{
	x = clamp_i(x, 0, (width - 1) * 16);
	y = clamp_i(y, 0, (height - 1) * 16);
	i32 x0 = x >> 4, y0 = y >> 4, fx = x & 15, fy = y & 15;
	i32 x1 = min_i(x0 + 1, width - 1), y1 = min_i(y0 + 1, height - 1);
	i32 top = channel(in[y0 * width + x0], c) * (16 - fx) + channel(in[y0 * width + x1], c) * fx;
	i32 bottom = channel(in[y1 * width + x0], c) * (16 - fx) + channel(in[y1 * width + x1], c) * fx;
	return (top * (16 - fy) + bottom * fy + 128) / 256;
}

static u32 noise(i32 x, i32 y, u32 seed)
{
	u32 v = seed ^ (u32)x * 0x9e3779b9u ^ (u32)y * 0x85ebca6bu;
	v ^= v >> 16;
	v *= 0x7feb352du;
	v ^= v >> 15;
	v *= 0x846ca68bu;
	return (v ^ v >> 16) & 255;
}

static i64 isqrt(i64 v)
{
	if (v <= 0)
		return 0;
	i64 s = (i64)__builtin_sqrtf((float)v);
	while (s * s > v)
		s--;
	while ((s + 1) * (s + 1) <= v)
		s++;
	return s;
}

__attribute__((amdgpu_kernel)) void glass(const struct glass_args a)
{
	u32 i = thread_id();
	if (i >= (u32)(a.aw * a.ah))
		return;
	i32 x = a.ax + (i32)(i % a.aw), y = a.ay + (i32)(i / a.aw);
	i32 px = x * 16 + 8 - (a.sx * 16 + a.sw * 8);
	i32 py = y * 16 + 8 - (a.sy * 16 + a.sh * 8);
	i32 radius = (px < 0 ? (py < 0 ? a.r0 : a.r3) : (py < 0 ? a.r1 : a.r2)) * 16;
	i32 qx = (px < 0 ? -px : px) - (a.sw * 8 - radius);
	i32 qy = (py < 0 ? -py : py) - (a.sh * 8 - radius);
	i64 ox = max_i(qx, 0), oy = max_i(qy, 0);
	i32 outside = (i32)isqrt(ox * ox + oy * oy);
	i32 distance = -(outside + min_i(max_i(qx, qy), 0) - radius);
	i32 nx, ny;
	if (qx > 0 && qy > 0 && outside > 0) {
		nx = qx * 256 / outside;
		ny = qy * 256 / outside;
	} else if (qx > qy) {
		nx = 256;
		ny = 0;
	} else {
		nx = 0;
		ny = 256;
	}
	if (px < 0)
		nx = -nx;
	if (py < 0)
		ny = -ny;
	if (distance <= -8) {
		a.out[i] = 0;
		return;
	}
	i32 coverage = clamp_i((distance + 8) * 255 / 16, 0, 255);
	i32 lx = x - a.left, ly = y - a.top;
	i32 rgb[3];
	if (distance >= a.splay || a.bend == 0) {
		u32 p = a.in[ly * a.width + lx];
		rgb[0] = p >> 16 & 255;
		rgb[1] = p >> 8 & 255;
		rgb[2] = p & 255;
	} else {
		i32 t = (a.splay - distance) * 256 / a.splay;
		i32 shift = a.bend * t / 256 * t / 256;
		i32 percent[3] = { 100 + a.spread, 100, 100 - a.spread };
		for (i32 c = 0; c < 3; c++) {
			i32 s = shift * percent[c] / 100;
			rgb[c] = bilinear(a.in, a.width, a.height, lx * 16 + nx * s / 256, ly * 16 + ny * s / 256, c);
		}
	}
	for (i32 c = 0; c < 3; c++) {
		i32 v = rgb[c] * a.gain / 256;
		v += (255 - v) * a.lift / 255;
		rgb[c] = clamp_i(v, 0, 255);
	}
	i32 luma = (rgb[0] * 54 + rgb[1] * 183 + rgb[2] * 19) / 256;
	i32 tint[3] = { a.tint_r, a.tint_g, a.tint_b };
	i32 crystal[3] = { a.crystal_r, a.crystal_g, a.crystal_b };
	i32 grain = (i32)noise(x, y, a.seed) - 128;
	i32 facing = (nx * a.light_x + ny * a.light_y) / 256;
	i32 highlight = 0, shadow = 0;
	if (facing > 0 && distance < a.rim)
		highlight = a.intensity * facing / 256 * facing / 256 * (a.rim - distance) / a.rim * 255 / 100;
	if (facing < 0 && distance < a.shade)
		shadow = a.intensity * -facing / 256 * (a.shade - distance) / a.shade * 90 / 100;
	u32 result = (u32)coverage << 24;
	for (i32 c = 0; c < 3; c++) {
		i32 v = luma + (rgb[c] - luma) * a.saturation / 100;
		v = v * a.brightness / 100;
		v = (v * (255 - a.tint_a) + tint[c] * a.tint_a) / 255;
		v += grain * a.noise_alpha / 255;
		v = clamp_i(v, 0, 255);
		v = (v * (255 - a.crystal_a) + crystal[c] * a.crystal_a) / 255;
		v -= v * shadow / 256;
		v += (255 - v) * highlight / 256;
		result |= (u32)clamp_i(v, 0, 255) << (16 - 8 * c);
	}
	a.out[i] = result;
}
