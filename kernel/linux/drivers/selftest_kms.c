// SPDX-License-Identifier: MIT
/* Boot self-test of the unmodified KMS core: a virtual display pipeline
 * (plane -> CRTC -> encoder -> HDMI connector) probes modes from a synthetic
 * 1080p EDID, then sets and clears a mode through the atomic helpers. */
#include <linux/module.h>
#include <linux/slab.h>
#include <drm/drm_drv.h>
#include <drm/drm_device.h>
#include <drm/drm_gem.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_crtc.h>
#include <drm/drm_plane.h>
#include <drm/drm_encoder.h>
#include <drm/drm_connector.h>
#include <drm/drm_edid.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_gem_framebuffer_helper.h>
#include <drm/drm_mode_config.h>
#include <drm/drm_modeset_helper_vtables.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_probe_helper.h>

static const u8 edid[128] = {
	0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00,
	0x04, 0xb2, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 36, 0x01, 0x04,
	0x80, 53, 30, 0x78, 0x0a,
	0xee, 0x91, 0xa3, 0x54, 0x4c, 0x99, 0x26, 0x0f, 0x50, 0x54,
	0x00, 0x00, 0x00,
	0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
	0x02, 0x3a, 0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c, 0x45, 0x00, 0x13, 0x8e, 0x21, 0x00, 0x00, 0x1e,
	0x00, 0x00, 0x00, 0xfc, 0x00, 'A', 'e', 'r', 'O', 'S', ' ', 'T', 'e', 's', 't', '\n', ' ', ' ',
	0x00, 0x00, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
	0x00, 0x00, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
	0x00, 0x18,
};

struct pipe {
	struct drm_device *dev;
	struct drm_plane plane;
	struct drm_crtc crtc;
	struct drm_encoder encoder;
	struct drm_connector connector;
	int enables, disables, updates;
	struct drm_framebuffer *scanout;
};

static struct pipe *pipe_of(struct drm_device *dev)
{
	return dev->dev_private;
}

static int freed;
static const char *edid_problem;

static void object_free(struct drm_gem_object *obj)
{
	drm_gem_object_release(obj);
	kfree(obj);
	freed++;
}

static const struct drm_gem_object_funcs object_funcs = {
	.free = object_free,
};

static void plane_update(struct drm_plane *plane, struct drm_atomic_state *state)
{
	struct drm_plane_state *new_state = drm_atomic_get_new_plane_state(state, plane);

	pipe_of(plane->dev)->updates++;
	pipe_of(plane->dev)->scanout = new_state->fb;
}

static int plane_check(struct drm_plane *plane, struct drm_atomic_state *state)
{
	struct drm_plane_state *new_state = drm_atomic_get_new_plane_state(state, plane);
	struct drm_crtc_state *crtc_state;

	if (!new_state->crtc)
		return 0;
	crtc_state = drm_atomic_get_new_crtc_state(state, new_state->crtc);
	return drm_atomic_helper_check_plane_state(new_state, crtc_state, DRM_PLANE_NO_SCALING,
						   DRM_PLANE_NO_SCALING, false, true);
}

static const struct drm_plane_helper_funcs plane_helpers = {
	.atomic_check = plane_check,
	.atomic_update = plane_update,
};

static const struct drm_plane_funcs plane_funcs = {
	.update_plane = drm_atomic_helper_update_plane,
	.disable_plane = drm_atomic_helper_disable_plane,
	.destroy = drm_plane_cleanup,
	.reset = drm_atomic_helper_plane_reset,
	.atomic_duplicate_state = drm_atomic_helper_plane_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_plane_destroy_state,
};

static void crtc_enable(struct drm_crtc *crtc, struct drm_atomic_state *state)
{
	pipe_of(crtc->dev)->enables++;
}

static void crtc_disable(struct drm_crtc *crtc, struct drm_atomic_state *state)
{
	pipe_of(crtc->dev)->disables++;
}

static const struct drm_crtc_helper_funcs crtc_helpers = {
	.atomic_enable = crtc_enable,
	.atomic_disable = crtc_disable,
};

static const struct drm_crtc_funcs crtc_funcs = {
	.set_config = drm_atomic_helper_set_config,
	.page_flip = drm_atomic_helper_page_flip,
	.destroy = drm_crtc_cleanup,
	.reset = drm_atomic_helper_crtc_reset,
	.atomic_duplicate_state = drm_atomic_helper_crtc_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_crtc_destroy_state,
};

static const struct drm_encoder_funcs encoder_funcs = {
	.destroy = drm_encoder_cleanup,
};

static int connector_get_modes(struct drm_connector *connector)
{
	const struct drm_edid *drm_edid = drm_edid_alloc(edid, sizeof(edid));
	int count, error;

	if (!drm_edid_valid(drm_edid))
		edid_problem = "rejected";
	error = drm_edid_connector_update(connector, drm_edid);
	if (error)
		edid_problem = "not stored on the connector";
	count = drm_edid_connector_add_modes(connector);
	drm_edid_free(drm_edid);
	return count;
}

static const struct drm_connector_helper_funcs connector_helpers = {
	.get_modes = connector_get_modes,
};

static const struct drm_connector_funcs connector_funcs = {
	.fill_modes = drm_helper_probe_single_connector_modes,
	.destroy = drm_connector_cleanup,
	.reset = drm_atomic_helper_connector_reset,
	.atomic_duplicate_state = drm_atomic_helper_connector_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_connector_destroy_state,
};

static const struct drm_mode_config_funcs mode_config_funcs = {
	.atomic_check = drm_atomic_helper_check,
	.atomic_commit = drm_atomic_helper_commit,
};

static const struct drm_framebuffer_funcs fb_funcs = {
	.destroy = drm_gem_fb_destroy,
};

static const struct drm_driver driver = {
	.driver_features = DRIVER_GEM | DRIVER_MODESET | DRIVER_ATOMIC,
	.name = "aeros-kms-test",
	.desc = "AerOS KMS self-test",
	.major = 1,
};

static const u32 formats[] = { DRM_FORMAT_XRGB8888 };

static int build_pipe(struct pipe *p)
{
	struct drm_device *dev = p->dev;
	int error;

	error = drmm_mode_config_init(dev);
	if (error)
		return error;
	dev->mode_config.min_width = 64;
	dev->mode_config.min_height = 64;
	dev->mode_config.max_width = 4096;
	dev->mode_config.max_height = 4096;
	dev->mode_config.funcs = &mode_config_funcs;

	error = drm_universal_plane_init(dev, &p->plane, 1, &plane_funcs, formats, ARRAY_SIZE(formats), NULL,
					 DRM_PLANE_TYPE_PRIMARY, NULL);
	if (error)
		return error;
	drm_plane_helper_add(&p->plane, &plane_helpers);
	error = drm_crtc_init_with_planes(dev, &p->crtc, &p->plane, NULL, &crtc_funcs, NULL);
	if (error)
		return error;
	drm_crtc_helper_add(&p->crtc, &crtc_helpers);
	p->encoder.possible_crtcs = drm_crtc_mask(&p->crtc);
	error = drm_encoder_init(dev, &p->encoder, &encoder_funcs, DRM_MODE_ENCODER_TMDS, NULL);
	if (error)
		return error;
	error = drm_connector_init(dev, &p->connector, &connector_funcs, DRM_MODE_CONNECTOR_HDMIA);
	if (error)
		return error;
	drm_connector_helper_add(&p->connector, &connector_helpers);
	error = drm_connector_attach_encoder(&p->connector, &p->encoder);
	if (error)
		return error;
	drm_mode_config_reset(dev);
	return 0;
}

static struct drm_framebuffer *make_fb(struct drm_device *dev, const struct drm_display_mode *mode)
{
	struct drm_framebuffer *fb = kzalloc(sizeof(*fb), GFP_KERNEL);
	struct drm_gem_object *obj = kzalloc(sizeof(*obj), GFP_KERNEL);
	int error;

	if (!fb || !obj) {
		kfree(fb);
		kfree(obj);
		return ERR_PTR(-ENOMEM);
	}
	obj->funcs = &object_funcs;
	drm_gem_private_object_init(dev, obj, PAGE_ALIGN((size_t)mode->hdisplay * mode->vdisplay * 4));
	fb->dev = dev;
	fb->format = drm_format_info(DRM_FORMAT_XRGB8888);
	fb->width = mode->hdisplay;
	fb->height = mode->vdisplay;
	fb->pitches[0] = mode->hdisplay * 4;
	fb->obj[0] = obj;
	error = drm_framebuffer_init(dev, fb, &fb_funcs);
	if (error) {
		drm_gem_object_put(obj);
		kfree(fb);
		return ERR_PTR(error);
	}
	return fb;
}

static int set_mode(struct pipe *p, struct drm_framebuffer *fb, struct drm_display_mode *mode)
{
	struct drm_connector *connectors[] = { &p->connector };
	struct drm_mode_set set = {
		.crtc = &p->crtc,
		.fb = fb,
		.mode = mode,
		.connectors = fb ? connectors : NULL,
		.num_connectors = fb ? 1 : 0,
	};
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	DRM_MODESET_LOCK_ALL_BEGIN(p->dev, ctx, 0, ret);
	ret = drm_atomic_helper_set_config(&set, &ctx);
	DRM_MODESET_LOCK_ALL_END(p->dev, ctx, ret);
	return ret;
}

static int kms_check(struct pipe *p)
{
	struct drm_display_mode *mode, *preferred = NULL;
	struct drm_framebuffer *fb;
	int count, error;

	mutex_lock(&p->dev->mode_config.mutex);
	count = p->connector.funcs->fill_modes(&p->connector, 4096, 4096);
	mutex_unlock(&p->dev->mode_config.mutex);
	if (edid_problem) {
		pr_err("kms selftest: EDID %s\n", edid_problem);
		return -EINVAL;
	}
	list_for_each_entry(mode, &p->connector.modes, head)
		if (mode->type & DRM_MODE_TYPE_PREFERRED)
			preferred = mode;
	if (!count || !preferred || preferred->hdisplay != 1920 || preferred->vdisplay != 1080 ||
	    drm_mode_vrefresh(preferred) != 60) {
		pr_err("kms selftest: EDID probe gave %d modes, preferred %s\n", count,
		       preferred ? preferred->name : "none");
		return -EINVAL;
	}
	if (p->connector.display_info.width_mm != 530 || p->connector.status != connector_status_connected) {
		pr_err("kms selftest: display info wrong (%u mm wide, status %d)\n",
		       p->connector.display_info.width_mm, p->connector.status);
		return -EINVAL;
	}

	fb = make_fb(p->dev, preferred);
	if (IS_ERR(fb))
		return (int)PTR_ERR(fb);
	error = set_mode(p, fb, preferred);
	if (error || p->enables != 1 || p->scanout != fb || !p->crtc.state->active) {
		pr_err("kms selftest: modeset failed (%d, %d enables, scanout %s)\n", error, p->enables,
		       p->scanout == fb ? "ok" : "wrong");
		return error ? error : -EINVAL;
	}
	pr_info("kms selftest: %d modes from EDID, %s@%d set on %s via %s (%d plane updates)\n", count,
		preferred->name, drm_mode_vrefresh(preferred), p->connector.name, p->crtc.name, p->updates);

	error = set_mode(p, NULL, NULL);
	drm_framebuffer_put(fb);
	if (error || p->disables != 1 || p->crtc.state->active) {
		pr_err("kms selftest: disabling the CRTC failed (%d, %d disables)\n", error, p->disables);
		return error ? error : -EINVAL;
	}
	return 0;
}

static int __init kms_selftest(void)
{
	static struct device parent;
	struct drm_device *dev;
	struct pipe *p;
	int error;

	dev = drm_dev_alloc(&driver, &parent);
	if (IS_ERR(dev))
		return (int)PTR_ERR(dev);
	p = kzalloc(sizeof(*p), GFP_KERNEL);
	if (!p)
		return -ENOMEM;
	p->dev = dev;
	dev->dev_private = p;
	error = build_pipe(p);
	if (!error)
		error = drm_dev_register(dev, 0);
	if (error) {
		pr_err("kms selftest: building the pipeline failed (%d)\n", error);
		return error;
	}
	error = kms_check(p);
	drm_dev_unregister(dev);
	drm_dev_put(dev);
	kfree(p);
	if (!error && freed != 1) {
		pr_err("kms selftest: framebuffer memory not released\n");
		error = -EINVAL;
	}
	if (!error)
		pr_info("kms selftest: pipeline torn down, framebuffer released\n");
	return error;
}

module_init(kms_selftest);
