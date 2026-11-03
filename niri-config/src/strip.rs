use knuffel::errors::DecodeError;

use crate::appearance::{Color, Gradient, GradientInterpolation, GradientRelativeTo};
use crate::limbo::LimboEdge;
use crate::utils::{Flag, FloatOrInt, MergeWith};

/// Place and look of the strip at the edge of workspaces in i3 mode, where windows set aside
/// show as live thumbnails.
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
    /// Which workspaces share a strip.
    pub share: StripShare,
    /// Edge of the screen where the strip sits.
    pub position: LimboEdge,
    /// How far the strip reaches into the screen.
    pub size: StripSize,
}

/// How far the strip reaches into the screen: its width at the sides, its height at the top and
/// bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StripSize {
    /// In logical pixels.
    Fixed(f64),
    /// As a share of the working area, from 0 to 1.
    Proportion(f64),
}

impl StripSize {
    /// The size in logical pixels, for a working area this long across the strip.
    pub fn resolve(self, across: f64) -> f64 {
        let size = match self {
            StripSize::Fixed(px) => px,
            StripSize::Proportion(p) => p * across,
        };
        size.clamp(1., across.max(1.))
    }
}

/// Which workspaces share a strip.
#[derive(knuffel::DecodeScalar, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum StripShare {
    /// Each workspace has its own strip.
    Workspace,
    /// Each output has one strip, which follows its active workspace.
    #[default]
    Output,
    /// There is one strip, which follows the active workspace of the output where a window last
    /// went into it.
    All,
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
            corner_radius: 0.,
            share: StripShare::default(),
            position: LimboEdge::Right,
            size: StripSize::Fixed(216.),
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
    #[knuffel(child, unwrap(argument))]
    pub share: Option<StripShare>,
    #[knuffel(child, unwrap(argument))]
    pub position: Option<LimboEdge>,
    #[knuffel(child, unwrap(argument))]
    pub size: Option<StripSize>,
}

impl StripPart {
    /// These overrides with those of `other` on top.
    pub fn with(self, other: &StripPart) -> StripPart {
        StripPart {
            off: other.off.or(self.off),
            on: other.on.or(self.on),
            background_color: other.background_color.or(self.background_color),
            background_gradient: other.background_gradient.or(self.background_gradient),
            rim_color: other.rim_color.or(self.rim_color),
            corner_radius: other.corner_radius.or(self.corner_radius),
            share: other.share.or(self.share),
            position: other.position.or(self.position),
            size: other.size.or(self.size),
        }
    }
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
        if let Some(x) = part.share {
            self.share = x;
        }
        if let Some(x) = part.position {
            self.position = x;
        }
        if let Some(x) = part.size {
            self.size = x;
        }
    }
}

/// A number of logical pixels, or a percentage like `"20%"`.
impl<S: knuffel::traits::ErrorSpan> knuffel::DecodeScalar<S> for StripSize {
    fn type_check(
        type_name: &Option<knuffel::span::Spanned<knuffel::ast::TypeName, S>>,
        ctx: &mut knuffel::decode::Context<S>,
    ) {
        if let Some(type_name) = &type_name {
            ctx.emit_error(DecodeError::unexpected(
                type_name,
                "type name",
                "no type name expected for this node",
            ));
        }
    }

    fn raw_decode(
        val: &knuffel::span::Spanned<knuffel::ast::Literal, S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Strip::default().size;
        match &**val {
            knuffel::ast::Literal::String(s) => {
                let percent = s
                    .trim()
                    .strip_suffix('%')
                    .and_then(|p| p.trim().parse::<f64>().ok())
                    .filter(|p| *p > 0. && *p <= 100.);
                match percent {
                    Some(p) => Ok(StripSize::Proportion(p / 100.)),
                    None => {
                        ctx.emit_error(DecodeError::conversion(
                            val,
                            "expected a percentage between 0% and 100%, like \"20%\"",
                        ));
                        Ok(default)
                    }
                }
            }
            _ => {
                let px = FloatOrInt::<1, 65535>::raw_decode(val, ctx)?;
                Ok(StripSize::Fixed(px.0))
            }
        }
    }
}
