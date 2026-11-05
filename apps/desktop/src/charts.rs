//! Animated canvas drawings: live throughput sparkline, progress ring and the
//! « flow » between source and destination. Purely presentational.

use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, LineCap, LineDash, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use std::f32::consts::PI;

/// Area chart of the latest samples (values are already scaled). An optional
/// second series shares the vertical scale, so the two directions stay
/// comparable. Flat translucent fills, no gradient.
#[derive(Debug, Clone)]
pub(crate) struct Spark {
    pub(crate) values: Vec<f32>,
    pub(crate) color: Color,
    /// Second series (rate toward the source) and its color.
    pub(crate) second: Option<(Vec<f32>, Color)>,
    pub(crate) grid: Color,
}

impl Spark {
    fn draw_series(frame: &mut Frame, (w, h): (f32, f32), max: f32, values: &[f32], color: Color) {
        if values.len() < 2 {
            return;
        }
        let step = w / (values.len() - 1) as f32;
        let point =
            |i: usize, v: f32| Point::new(i as f32 * step, h - 4.0 - (v / max) * (h - 10.0));
        let line = Path::new(|b| {
            for (i, v) in values.iter().enumerate() {
                if i == 0 {
                    b.move_to(point(i, *v));
                } else {
                    b.line_to(point(i, *v));
                }
            }
        });
        let area = Path::new(|b| {
            b.move_to(Point::new(0.0, h));
            for (i, v) in values.iter().enumerate() {
                b.line_to(point(i, *v));
            }
            b.line_to(Point::new(w, h));
            b.close();
        });
        frame.fill(&area, Color { a: 0.14, ..color });
        frame.stroke(
            &line,
            Stroke::default()
                .with_color(color)
                .with_width(2.0)
                .with_line_cap(LineCap::Round),
        );
        if let Some(last) = values.last() {
            let p = point(values.len() - 1, *last);
            frame.fill(&Path::circle(p, 4.0), color);
            frame.fill(&Path::circle(p, 8.0), Color { a: 0.2, ..color });
        }
    }
}

impl<M> canvas::Program<M> for Spark {
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
        let (w, h) = (bounds.width, bounds.height);
        for i in 1..4 {
            let y = h * i as f32 / 4.0;
            frame.stroke(
                &Path::line(Point::new(0.0, y), Point::new(w, y)),
                Stroke::default().with_color(self.grid).with_width(1.0),
            );
        }
        let max = self
            .values
            .iter()
            .chain(self.second.iter().flat_map(|(v, _)| v.iter()))
            .copied()
            .fold(f32::EPSILON, f32::max);
        if let Some((second, color)) = &self.second {
            Self::draw_series(&mut frame, (w, h), max, second, *color);
        }
        Self::draw_series(&mut frame, (w, h), max, &self.values, self.color);
        vec![frame.into_geometry()]
    }
}

pub(crate) fn spark<'a, M: 'a>(
    values: Vec<f32>,
    color: Color,
    second: Option<(Vec<f32>, Color)>,
    grid: Color,
    height: f32,
) -> Element<'a, M> {
    canvas::Canvas::new(Spark {
        values,
        color,
        second,
        grid,
    })
    .width(Length::Fill)
    .height(Length::Fixed(height))
    .into()
}

/// Dashed connector between endpoints; dashes travel when `phase` moves.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Flow {
    pub(crate) phase: f32,
    pub(crate) color: Color,
    pub(crate) two_way: bool,
    pub(crate) active: bool,
}

impl<M> canvas::Program<M> for Flow {
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
        let Size {
            width: w,
            height: h,
        } = bounds.size();
        let mid = h / 2.0;
        let lanes: &[(f32, bool)] = if self.two_way {
            &[(mid - 6.0, true), (mid + 6.0, false)]
        } else {
            &[(mid, true)]
        };
        let dash = [6.0, 6.0];
        for (y, forward) in lanes {
            let offset = if self.active {
                (self.phase * 24.0) % 12.0
            } else {
                0.0
            };
            let line = Path::line(Point::new(6.0, *y), Point::new(w - 10.0, *y));
            frame.stroke(
                &line,
                Stroke {
                    line_dash: LineDash {
                        segments: &dash,
                        offset: if *forward {
                            (12.0 - offset) as usize
                        } else {
                            offset as usize
                        },
                    },
                    ..Stroke::default()
                        .with_color(Color {
                            a: if self.active { 0.9 } else { 0.55 },
                            ..self.color
                        })
                        .with_width(2.0)
                        .with_line_cap(LineCap::Round)
                },
            );
            let (tip, back) = if *forward {
                (w - 6.0, w - 13.0)
            } else {
                (6.0, 13.0)
            };
            let head = Path::new(|b| {
                b.move_to(Point::new(back, *y - 5.0));
                b.line_to(Point::new(tip, *y));
                b.line_to(Point::new(back, *y + 5.0));
            });
            frame.stroke(
                &head,
                Stroke::default()
                    .with_color(self.color)
                    .with_width(2.0)
                    .with_line_cap(LineCap::Round),
            );
        }
        vec![frame.into_geometry()]
    }
}

pub(crate) fn flow<'a, M: 'a>(flow: Flow, width: f32) -> Element<'a, M> {
    canvas::Canvas::new(flow)
        .width(Length::Fixed(width))
        .height(Length::Fixed(28.0))
        .into()
}

/// Application mark: a rounded square split by a flowing S-curve, `from`
/// above and `to` below. `turn` (in turns) makes the curve ripple, so the
/// splash screen shows the logo alive.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mark {
    pub(crate) from: Color,
    pub(crate) to: Color,
    pub(crate) turn: f32,
}

/// Geometry of the logo in a `s`-sized square: the two fills as paths.
/// `phase` shifts the wave (0 for the static logo).
pub(crate) fn logo_paths(s: f32, phase: f32) -> (Path, Path) {
    let r = s * 0.3;
    let k = r * 0.5523;
    let wave = s * 0.17 * phase.cos();
    let (yl, yr) = (s * 0.46, s * 0.54);
    let top = Path::rounded_rectangle(Point::ORIGIN, Size::new(s, s), r.into());
    let bottom = Path::new(|b| {
        b.move_to(Point::new(0.0, yl));
        b.bezier_curve_to(
            Point::new(s * 0.35, yl - wave - s * 0.2),
            Point::new(s * 0.65, yr + wave + s * 0.2),
            Point::new(s, yr),
        );
        b.line_to(Point::new(s, s - r));
        b.bezier_curve_to(
            Point::new(s, s - r + k),
            Point::new(s - r + k, s),
            Point::new(s - r, s),
        );
        b.line_to(Point::new(r, s));
        b.bezier_curve_to(
            Point::new(r - k, s),
            Point::new(0.0, s - r + k),
            Point::new(0.0, s - r),
        );
        b.close();
    });
    (top, bottom)
}

impl<M> canvas::Program<M> for Mark {
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
        let s = bounds.width.min(bounds.height);
        let (top, bottom) = logo_paths(s, self.turn * 2.0 * PI);
        frame.fill(&top, self.from);
        frame.fill(&bottom, self.to);
        vec![frame.into_geometry()]
    }
}

pub(crate) fn mark<'a, M: 'a>(mark: Mark, size: f32) -> Element<'a, M> {
    canvas::Canvas::new(mark)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

/// Side of the square application icon rasters.
pub(crate) const ICON_SIZE: u32 = 64;

/// Window icon: the application icon (`apps/desktop/assets`).
pub(crate) fn window_icon() -> Option<iced::window::Icon> {
    iced::window::icon::from_rgba(icon_rgba(), ICON_SIZE, ICON_SIZE).ok()
}

/// Application icon as straight RGBA, [`ICON_SIZE`]² (window and tray),
/// generated from `docs/icon/syncengine.png` by `scripts/generate_icons.py`.
pub(crate) fn icon_rgba() -> Vec<u8> {
    const RGBA: &[u8; (ICON_SIZE * ICON_SIZE * 4) as usize] =
        include_bytes!("../assets/icon-64.rgba");
    RGBA.to_vec()
}
