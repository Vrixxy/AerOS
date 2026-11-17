// SPDX-License-Identifier: MIT
/* AerOS's own stand-ins for drm_hdcp_helper.c (GPL-only upstream). No
 * player asks for protected content, so the property is not offered and
 * the state only records what the display driver reports. */
#include <linux/kernel.h>
#include <drm/drm_connector.h>
#include <drm/display/drm_hdcp_helper.h>

int drm_connector_attach_content_protection_property(struct drm_connector *connector, bool hdcp_content_type)
{
	return 0;
}

void drm_hdcp_update_content_protection(struct drm_connector *connector, u64 val)
{
	if (connector->state)
		connector->state->content_protection = val;
}
