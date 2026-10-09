//! i3-style resize mode: arrows and h/j/k/l resize the focused window until Escape or Return.

use niri_config::{Action, Bind, Key, Modifiers, Trigger};
use niri_ipc::SizeChange;
use ordered_float::NotNan;
use smithay::backend::renderer::element::Kind;
use smithay::input::keyboard::Keysym;
use smithay::utils::{Logical, Rectangle};

use crate::render_helpers::label::{render_pill, LabelCache};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};

const TEXT: &str = "resize";
const FONT: &str = "sans bold 15px";
/// Radius of the pill ends, in logical pixels.
const RADIUS: f64 = 16.;
/// Fill of the pill: a translucent dark grey, like the window hints.
const FILL: [f64; 4] = [0.12, 0.12, 0.12, 0.7];
/// How much one key press resizes, in percent of the working area.
const STEP: f64 = 10.;

pub struct ResizeMode {
    active: bool,
    /// Label textures by scale.
    textures: LabelCache<NotNan<f64>>,
}

impl ResizeMode {
    pub fn new() -> Self {
        Self {
            active: false,
            textures: LabelCache::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn toggle(&mut self) {
        self.active = !self.active;
    }

    /// Leaves resize mode. Returns whether it was active.
    pub fn close(&mut self) -> bool {
        std::mem::take(&mut self.active)
    }

    /// Renders the label centered in `rect`, in output-local logical coordinates.
    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        scale: f64,
        rect: Rectangle<f64, Logical>,
        push: &mut dyn FnMut(PrimaryGpuTextureRenderElement),
    ) {
        if !self.active {
            return;
        }
        let _span = tracy_client::span!("ResizeMode::render");

        let key = NotNan::new(scale).unwrap();
        let Some(buffer) = self
            .textures
            .get(key, || render_pill(TEXT, FONT, RADIUS, FILL, scale))
        else {
            return;
        };

        let Ok(texture) = TextureBuffer::from_memory_buffer(renderer.as_gles_renderer(), &buffer)
        else {
            return;
        };

        let size = texture.logical_size();
        let loc = rect.loc + (rect.size.to_point() - size.to_point()).downscale(2.);
        let loc = loc.to_physical_precise_round(scale).to_logical(scale);
        let elem = TextureRenderElement::from_texture_buffer(
            texture,
            loc,
            1.,
            None,
            None,
            Kind::Unspecified,
        );
        push(PrimaryGpuTextureRenderElement(elem));
    }
}

impl Default for ResizeMode {
    fn default() -> Self {
        Self::new()
    }
}

/// The resize bind for a key pressed in resize mode, if any.
pub fn resize_bind(raw: Keysym) -> Option<Bind> {
    let action = match raw {
        Keysym::Left | Keysym::h => Action::SetColumnWidth(SizeChange::AdjustProportion(-STEP)),
        Keysym::Right | Keysym::l => Action::SetColumnWidth(SizeChange::AdjustProportion(STEP)),
        Keysym::Up | Keysym::k => Action::SetWindowHeight(SizeChange::AdjustProportion(-STEP)),
        Keysym::Down | Keysym::j => Action::SetWindowHeight(SizeChange::AdjustProportion(STEP)),
        _ => return None,
    };

    Some(Bind {
        key: Key {
            trigger: Trigger::Keysym(raw),
            modifiers: Modifiers::empty(),
        },
        action,
        repeat: true,
        cooldown: None,
        allow_when_locked: false,
        allow_inhibiting: false,
        hotkey_overlay_title: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(raw: Keysym) -> Option<Action> {
        resize_bind(raw).map(|bind| bind.action)
    }

    #[test]
    fn keys_map_to_resizes() {
        let grow_w = Some(Action::SetColumnWidth(SizeChange::AdjustProportion(STEP)));
        let shrink_w = Some(Action::SetColumnWidth(SizeChange::AdjustProportion(-STEP)));
        let grow_h = Some(Action::SetWindowHeight(SizeChange::AdjustProportion(STEP)));
        let shrink_h = Some(Action::SetWindowHeight(SizeChange::AdjustProportion(-STEP)));

        assert_eq!(action(Keysym::Right), grow_w);
        assert_eq!(action(Keysym::l), grow_w);
        assert_eq!(action(Keysym::Left), shrink_w);
        assert_eq!(action(Keysym::h), shrink_w);
        assert_eq!(action(Keysym::Down), grow_h);
        assert_eq!(action(Keysym::j), grow_h);
        assert_eq!(action(Keysym::Up), shrink_h);
        assert_eq!(action(Keysym::k), shrink_h);

        assert_eq!(action(Keysym::q), None);
        assert_eq!(action(Keysym::Escape), None);
    }

    #[test]
    fn toggle_and_close() {
        let mut mode = ResizeMode::new();
        assert!(!mode.is_active());
        mode.toggle();
        assert!(mode.is_active());
        assert!(mode.close());
        assert!(!mode.is_active());
        assert!(!mode.close());
    }
}
