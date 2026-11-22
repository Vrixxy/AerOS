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
#include <drm/drm_vblank.h>
#include <drm/drm_crtc.h>
#include <linux/backlight.h>

bool aeros_display_taken;
struct backlight_device *aeros_backlight;

/* AerOS's brightness setting, on the first registered panel backlight. */
int aeros_set_backlight(unsigned int percent)
{
	struct backlight_device *bd = aeros_backlight;

	if (!bd || bd->props.max_brightness <= 0)
		return -ENODEV;
	percent = min(percent, 100u);
	bd->props.brightness = DIV_ROUND_UP(bd->props.max_brightness * percent * percent, 10000);
	return backlight_update_status(bd);
}
static struct drm_client_dev client;
static struct drm_client_buffer *buffer;
static struct drm_crtc *scanout_crtc;

static const struct drm_client_funcs client_funcs = {
	.owner = NULL,
};

/* Waits for the scanned-out CRTC's next vblank (at most 20 ms), so AerOS can
 * copy a frame in from the top while the panel is between refreshes. */
int aeros_display_wait_vblank(void)
{
	struct drm_crtc *crtc = scanout_crtc;
	u64 last;
	long left;

	if (!crtc || drm_crtc_vblank_get(crtc))
		return -ENODEV;
	last = drm_crtc_vblank_count(crtc);
	left = wait_event_timeout(crtc->dev->vblank[drm_crtc_index(crtc)].queue,
				  drm_crtc_vblank_count(crtc) != last, msecs_to_jiffies(20));
	drm_crtc_vblank_put(crtc);
	return left ? 0 : -ETIMEDOUT;
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
		if (!lit)
			scanout_crtc = modeset->crtc;
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
	mutex_lock(&client.modeset_mutex);
	drm_client_for_each_modeset(modeset, &client) {
		if (modeset->mode)
			modeset->fb = buffer->fb;
	}
	mutex_unlock(&client.modeset_mutex);
	/* AerOS copies the firmware screen in first, so the switch is invisible. */
	ret = aeros->display(map.is_iomem ? (void *)map.vaddr_iomem : map.vaddr, width, height, buffer->pitch);
	if (ret) {
		pr_err("display: AerOS refused the framebuffer (%d)\n", ret);
		goto unmap;
	}
	ret = drm_client_modeset_commit(&client);
	if (ret) {
		pr_err("display: modeset failed (%d)\n", ret);
		aeros->display(NULL, 0, 0, 0);
		goto unmap;
	}
	pr_info("display: %d screen(s), %ux%u, pitch %u\n", lit, width, height, buffer->pitch);
	aeros_display_taken = 1;
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
