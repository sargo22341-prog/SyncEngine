//! Soft colored halos drawn behind the content area. iced has neither blur nor
//! radial gradients, so each halo is a stack of concentric translucent
//! circles whose alpha accumulates toward the center. Static on purpose: no
//! frame subscription is needed, so it costs nothing while the UI is idle.

use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, Path};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme};

/// Number of stacked circles per halo; more layers means a smoother falloff.
const LAYERS: u32 = 36;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Halo {
    /// Color of both halos (mode of the selected profile, accent otherwise).
    pub(crate) color: Color,
    /// Peak opacity of the main halo (top right corner).
    pub(crate) strength: f32,
}

/// One halo: `center` and `radius` in pixels, `peak` the center opacity.
fn glow(frame: &mut Frame, color: Color, center: Point, radius: f32, peak: f32) {
    // Each layer adds `step`; after all layers the center reaches about `peak`.
    let step = 1.0 - (1.0 - peak).powf(1.0 / LAYERS as f32);
    for layer in 0..LAYERS {
        let k = 1.0 - layer as f32 / LAYERS as f32;
        // Smooth shrink: dense near the center, sparse at the edge.
        let r = radius * k * k.sqrt();
        frame.fill(
            &Path::circle(center, r.max(1.0)),
            Color { a: step, ..color },
        );
    }
}

impl<M> canvas::Program<M> for Halo {
    type State = ();
    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width;
        // Main halo, partly outside the corner; a faint echo on the left.
        glow(
            &mut frame,
            self.color,
            Point::new(w - 40.0, 10.0),
            280.0,
            self.strength,
        );
        glow(
            &mut frame,
            self.color,
            Point::new(60.0, -30.0),
            210.0,
            self.strength * 0.35,
        );
        vec![frame.into_geometry()]
    }
}

/// Canvas filling its parent; place it under the page in a `stack!`.
pub(crate) fn halo<'a, M: 'a>(halo: Halo) -> Element<'a, M> {
    canvas::Canvas::new(halo)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
