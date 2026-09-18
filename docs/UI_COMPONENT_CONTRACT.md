# AerOS UI component contract

The Figma button is the visual source of truth. Its AerOS implementation should describe layout, appearance, and content while delegating backdrop capture, blur, clipping, pointer capture, keyboard activation, and framebuffer access to `aerui`.

## Source constraints

- Use `core`, `crate::aerui`, and the existing font interfaces.
- Do not depend on `std`, Windows APIs, `winit`, `egui`, Slint, GTK, Qt, HTML, or a browser runtime.
- Do not perform raw framebuffer writes or introduce `unsafe` code.
- Do not allocate during layout, event handling, or painting.
- Use borrowed `&str` labels and fixed-size value types.
- Keep all measurements in logical pixels until the caller applies `Scale`.
- Return activation as data. Do not invoke application callbacks from the paint path.

## Component shape

The button should expose one component with configurable bounds, padding, four corner radii, content, semantic label, selected state, disabled state, and visual styles. Dock and app-selector elements should select different values rather than fork the implementation.

The expected structure is equivalent to:

```rust
use crate::aerui::{
    ButtonInteraction, CornerRadii, Event, FrostReport, FrostStyle, Insets, InteractionResponse,
    Painter, Rect, Scale, Size,
};

pub struct ButtonStyle {
    pub minimum_size: Size,
    pub padding: Insets,
    pub radii: CornerRadii,
    pub surface: FrostStyle,
}

pub struct ButtonStyles {
    pub resting: ButtonStyle,
    pub hovered: ButtonStyle,
    pub pressed: ButtonStyle,
    pub focused: ButtonStyle,
    pub selected: ButtonStyle,
    pub disabled: ButtonStyle,
}

pub struct Button<'a> {
    pub bounds: Rect,
    pub label: &'a str,
    pub semantic_label: &'a str,
    pub styles: &'a ButtonStyles,
    pub interaction: ButtonInteraction,
    pub selected: bool,
}

impl Button<'_> {
    pub fn measure(&self, scale: Scale) -> Size;
    pub fn handle(&mut self, event: Event, now_ns: u64) -> InteractionResponse;
    pub fn paint(&self, painter: &mut Painter<'_>, scale: Scale, now_ns: u64) -> FrostReport;
}
```

Names may differ, but the separation of measurement, input, and painting should remain.

## Figma values to preserve

Record the frame size, minimum size, horizontal and vertical padding, gap between icon and label, all four corner radii, fill RGBA, backdrop blur radius, saturation, brightness, border width and RGBA, inner highlight, shadow RGBA, offset, spread, softness, font family, font size, weight, line height, letter spacing, icon dimensions, and optical offsets.

If Figma shows only one state, implement that state exactly. AerOS can derive temporary hover, pressed, focused, selected, and disabled styles during integration and replace them when final designs exist.

The current dock source values are 527×71, radius 30, `#C4C4C4` at 20 percent, a one-pixel inside black stroke, glass light at −45 degrees and 80 percent, refraction 0, depth 100, dispersion 15, frost 45, splay 100, and a black 25 percent inner shadow at x 9.06, y 9.06, blur 10, spread 0. The app switcher uses the same surface at 527×414. The reusable icon button starts at 50×50 with radius 17.

## Interaction ownership

Store `ButtonInteraction` on each button instance and forward AerOS `Event` values to it. The state machine already provides primary-pointer capture, release-inside activation, cancellation outside the bounds, focus changes, Enter activation, Space activation, disabled suppression, and redraw requests.

The button should paint according to interaction state but must not duplicate hit testing or keyboard rules.

## Rendering order

Render the surface through `Painter::frosted_rounded_rect`, then the icon, label, selection indicator, and focus indicator. Backdrop blur must be requested before opaque foreground content is drawn. Use `FrostReport::captured` to detect the bounded fallback path for surfaces larger than the scratch capacity.

Animation should be driven by `now_ns` and stored transition timestamps. Painting must never sleep or wait. Geometry, color, and opacity transitions should remain deterministic for the same state and timestamp.

## Integration location

The finished component lives in `C:\Users\hkvla\AerOS\kernel\src\button.rs` and is registered by the kernel entry point. Desktop callers provide logical bounds and use the shared scaled and translated paint path for both dock and app-switcher instances.
