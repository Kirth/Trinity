use crate::appearance::{Color, Gradient, GradientInterpolation, GradientRelativeTo};
use crate::utils::{Flag, FloatOrInt, MergeWith};

/// Look of the strip at the right edge of workspaces in i3 mode, where windows set aside show as
/// live thumbnails.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strip {
    /// Whether to draw no panel behind the thumbnails.
    pub off: bool,
    /// Colour of the panel behind the thumbnails.
    pub background_color: Color,
    /// Gradient of the panel, drawn instead of the colour.
    pub background_gradient: Option<Gradient>,
    /// Colour of the thin rim around the panel; fully transparent for none.
    pub rim_color: Color,
    /// Radius of the panel's corners, in logical pixels.
    pub corner_radius: f64,
}

impl Default for Strip {
    fn default() -> Self {
        // Smoked glass, a bit lighter at the top, like a modern take on the Longhorn sidebar.
        Self {
            off: false,
            background_color: Color::from_rgba8_unpremul(0x1c, 0x23, 0x33, 0xb3),
            background_gradient: Some(Gradient {
                from: Color::from_rgba8_unpremul(0x2b, 0x36, 0x4d, 0xb3),
                to: Color::from_rgba8_unpremul(0x10, 0x14, 0x1e, 0xcc),
                angle: 180,
                relative_to: GradientRelativeTo::Window,
                in_: GradientInterpolation::default(),
            }),
            rim_color: Color::from_rgba8_unpremul(0xff, 0xff, 0xff, 0x26),
            corner_radius: 16.,
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
pub struct StripPart {
    #[knuffel(child)]
    pub off: Option<Flag>,
    #[knuffel(child)]
    pub on: Option<Flag>,
    #[knuffel(child, unwrap(argument, str))]
    pub background_color: Option<Color>,
    #[knuffel(child)]
    pub background_gradient: Option<Gradient>,
    #[knuffel(child, unwrap(argument, str))]
    pub rim_color: Option<Color>,
    #[knuffel(child, unwrap(argument))]
    pub corner_radius: Option<FloatOrInt<0, 1000>>,
}

impl MergeWith<StripPart> for Strip {
    fn merge_with(&mut self, part: &StripPart) {
        if let Some(Flag(off)) = part.off {
            self.off = off;
        }
        if let Some(Flag(on)) = part.on {
            self.off = !on;
        }
        merge_color_gradient!((self, part), (background_color, background_gradient));
        if let Some(x) = part.rim_color {
            self.rim_color = x;
        }
        if let Some(x) = part.corner_radius {
            self.corner_radius = x.0;
        }
    }
}
