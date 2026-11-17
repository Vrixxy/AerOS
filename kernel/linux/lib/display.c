// SPDX-License-Identifier: MIT
/* Hands the screen to AerOS once a DRM driver with a display (amdgpu's
 * display core) has bound: an in-kernel DRM client sets every lit
 * connector's mode on one framebuffer, shows a test pattern for a moment so
 * a hardware test can see that the modeset worked, and then gives AerOS the
 * buffer to draw its desktop into. */
#include <linux/kernel.h>
#include <linux/delay.h>
#include <linux/iosys-map.h>
#include <drm/drm_client.h>
#include <drm/drm_device.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_modes.h>
#include <aeros/ops.h>

bool aeros_display_taken;
static struct drm_client_dev client;
static struct drm_client_buffer *buffer;

static const struct drm_client_funcs client_funcs = {
	.owner = NULL,
};

static void draw_pattern(u8 *pixels, u32 width, u32 height, u32 pitch)
{
	static const u32 bars[] = { 0xffffff, 0xffff00, 0x00ffff, 0x00ff00, 0xff00ff, 0xff0000, 0x0000ff, 0x202020 };

	for (u32 y = 0; y < height; y++) {
		u32 *row = (u32 *)(pixels + (size_t)y * pitch);

		for (u32 x = 0; x < width; x++) {
			bool border = x < 8 || y < 8 || x >= width - 8 || y >= height - 8;

			row[x] = border ? 0xffffff : bars[x * 8 / width];
		}
	}
}

int aeros_display_take_over(struct drm_device *dev)
{
	struct drm_mode_set *modeset;
	struct iosys_map map;
	unsigned int screen_width = 0, screen_height = 0;
	u32 width = 0, height = 0;
	int lit = 0;
	int ret;

	aeros->screen_size(&screen_width, &screen_height);
	ret = drm_client_init(dev, &client, "aeros", &client_funcs);
	if (ret) {
		pr_err("display: no DRM client (%d)\n", ret);
		return ret;
	}
	ret = drm_client_modeset_probe(&client, 0, 0);
	if (ret) {
		pr_err("display: probing connectors failed (%d)\n", ret);
		goto release;
	}
	mutex_lock(&client.modeset_mutex);
	drm_client_for_each_modeset(modeset, &client) {
		if (!modeset->mode || !modeset->num_connectors)
			continue;
		pr_info("display: %s: %dx%d@%d on %s\n", modeset->connectors[0]->name,
			modeset->mode->hdisplay, modeset->mode->vdisplay, drm_mode_vrefresh(modeset->mode),
			modeset->crtc->name);
		width = max_t(u32, width, modeset->mode->hdisplay);
		height = max_t(u32, height, modeset->mode->vdisplay);
		lit++;
	}
	mutex_unlock(&client.modeset_mutex);
	if (!lit) {
		pr_err("display: no connector has a mode\n");
		ret = -ENODEV;
		goto release;
	}
	/* At least as big as the firmware's screen, which AerOS keeps drawing. */
	width = max_t(u32, width, screen_width);
	height = max_t(u32, height, screen_height);
	buffer = drm_client_framebuffer_create(&client, width, height, DRM_FORMAT_XRGB8888);
	if (IS_ERR(buffer)) {
		ret = PTR_ERR(buffer);
		buffer = NULL;
		pr_err("display: no %ux%u framebuffer (%d)\n", width, height, ret);
		goto release;
	}
	ret = drm_client_buffer_vmap(buffer, &map);
	if (ret) {
		pr_err("display: framebuffer can't be mapped (%d)\n", ret);
		goto free_buffer;
	}
	draw_pattern(map.is_iomem ? (u8 *)map.vaddr_iomem : map.vaddr, width, height, buffer->pitch);
	mutex_lock(&client.modeset_mutex);
	drm_client_for_each_modeset(modeset, &client) {
		if (modeset->mode)
			modeset->fb = buffer->fb;
	}
	mutex_unlock(&client.modeset_mutex);
	ret = drm_client_modeset_commit(&client);
	if (ret) {
		pr_err("display: modeset failed (%d)\n", ret);
		goto unmap;
	}
	pr_info("display: test pattern on %d screen(s), %ux%u, pitch %u\n", lit, width, height, buffer->pitch);
	msleep(1500);
	ret = aeros->display(map.is_iomem ? (void *)map.vaddr_iomem : map.vaddr, width, height, buffer->pitch);
	aeros_display_taken = !ret;
	pr_info("display: handed to AerOS (%d)\n", ret);
	drm_client_register(&client);
	return 0;

unmap:
	drm_client_buffer_vunmap(buffer);
free_buffer:
	drm_client_framebuffer_delete(buffer);
	buffer = NULL;
release:
	drm_client_release(&client);
	return ret;
}
