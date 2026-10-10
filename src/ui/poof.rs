//! A puff of smoke where a window poofs away to the scratchpad.
//!
//! It's timed by itself rather than by an `Animation`, so it plays even with animations turned
//! off.

use std::cell::RefCell;
use std::f64::consts::PI;
use std::time::Duration;

use pangocairo::cairo::{self, ImageSurface, RadialGradient};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::output::Output;
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::{Logical, Point, Size, Transform};

use crate::render_helpers::memory::MemoryBuffer;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};

/// How long a puff lasts.
const DURATION: Duration = Duration::from_millis(700);
const PARTICLES: usize = 14;
/// Radius of a particle at its starting size, in logical pixels.
const RADIUS: f64 = 14.;

#[derive(Debug)]
struct Particle {
    /// Where the particle ends up, relative to the puff origin.
    offset: Point<f64, Logical>,
    /// Size relative to the base radius.
    size: f64,
    /// Fraction of the duration before the particle starts moving.
    delay: f64,
}

#[derive(Debug)]
struct Puff {
    output: Output,
    /// Center of the puff, in output-local logical coordinates.
    origin: Point<f64, Logical>,
    start: Duration,
    particles: Vec<Particle>,
    /// One texture per particle, so that each element has its own id, with the scale they're for.
    textures: RefCell<Option<(f64, Vec<TextureBuffer<GlesTexture>>)>>,
}

#[derive(Debug, Default)]
pub struct Poof {
    puffs: Vec<Puff>,
}

impl Poof {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a puff at `origin`, spraying to the left, away from the right edge of the output.
    pub fn spawn(&mut self, output: Output, origin: Point<f64, Logical>, now: Duration) {
        let particles = (0..PARTICLES)
            .map(|i| {
                // Spread over the left half circle, with some jitter.
                let angle = PI / 2. + PI * (i as f64 + fastrand::f64()) / PARTICLES as f64;
                let distance = 40. + 90. * fastrand::f64();
                Particle {
                    offset: Point::from((angle.cos() * distance, angle.sin() * distance)),
                    size: 0.6 + 0.8 * fastrand::f64(),
                    delay: 0.15 * fastrand::f64(),
                }
            })
            .collect();

        self.puffs.push(Puff {
            output,
            origin,
            start: now,
            particles,
            textures: RefCell::new(None),
        });
    }

    /// Drops all puffs, for undoing a poof.
    pub fn cancel(&mut self) {
        self.puffs.clear();
    }

    pub fn advance_animations(&mut self, now: Duration) {
        self.puffs
            .retain(|puff| now.saturating_sub(puff.start) < DURATION);
    }

    pub fn are_animations_ongoing(&self, output: &Output) -> bool {
        self.puffs.iter().any(|puff| &puff.output == output)
    }

    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
        scale: f64,
        now: Duration,
        push: &mut dyn FnMut(PrimaryGpuTextureRenderElement),
    ) {
        for puff in self.puffs.iter().filter(|puff| &puff.output == output) {
            let mut textures = puff.textures.borrow_mut();
            if textures.as_ref().is_none_or(|(s, _)| *s != scale) {
                let Ok(buffer) = render_particle(scale) else {
                    return;
                };
                let renderer = renderer.as_gles_renderer();
                let buffers: Result<Vec<_>, _> = puff
                    .particles
                    .iter()
                    .map(|_| TextureBuffer::from_memory_buffer(renderer, &buffer))
                    .collect();
                let Ok(buffers) = buffers else {
                    return;
                };
                *textures = Some((scale, buffers));
            }
            let (_, buffers) = textures.as_ref().unwrap();

            let elapsed = now.saturating_sub(puff.start);
            let t = elapsed.as_secs_f64() / DURATION.as_secs_f64();

            for (particle, texture) in puff.particles.iter().zip(buffers) {
                let t = ((t - particle.delay) / (1. - particle.delay)).clamp(0., 1.);
                let ease = 1. - (1. - t).powi(3);

                // Smoke spreads out, grows, drifts up a little and fades.
                let size = RADIUS * 2. * particle.size * (0.5 + t);
                let center =
                    puff.origin + particle.offset.upscale(ease) + Point::from((0., -20. * ease));
                let loc = center - Point::from((size / 2., size / 2.));
                let alpha = (1. - t).powi(2) * 0.8;

                let elem = TextureRenderElement::from_texture_buffer(
                    texture.clone(),
                    loc,
                    alpha as f32,
                    None,
                    Some(Size::from((size, size))),
                    Kind::Unspecified,
                );
                push(PrimaryGpuTextureRenderElement(elem));
            }
        }
    }
}

/// Draws a soft, light grey dot, fading out towards its edge.
fn render_particle(scale: f64) -> anyhow::Result<MemoryBuffer> {
    let r = (RADIUS * scale).ceil();
    let size = (r * 2.) as i32;

    let surface = ImageSurface::create(cairo::Format::ARgb32, size, size)?;
    let cr = cairo::Context::new(&surface)?;

    let gradient = RadialGradient::new(r, r, 0., r, r, r);
    gradient.add_color_stop_rgba(0., 0.92, 0.92, 0.92, 0.9);
    gradient.add_color_stop_rgba(0.6, 0.85, 0.85, 0.85, 0.5);
    gradient.add_color_stop_rgba(1., 0.8, 0.8, 0.8, 0.);
    cr.set_source(&gradient)?;
    cr.arc(r, r, r, 0., PI * 2.);
    cr.fill()?;
    drop(cr);

    let data = surface.take_data().unwrap();
    Ok(MemoryBuffer::new(
        data.to_vec(),
        Fourcc::Argb8888,
        (size, size),
        scale,
        Transform::Normal,
    ))
}
