//! Animated canvas drawings: live throughput sparkline, progress ring and the
//! « flow » between source and destination. Purely presentational.

use iced::mouse;
use iced::widget::canvas::gradient;
use iced::widget::canvas::{self, Frame, Geometry, LineCap, LineDash, Path, Stroke, path::Arc};
use iced::{Color, Element, Length, Point, Radians, Rectangle, Renderer, Size, Theme};
use std::f32::consts::PI;

/// Filled area chart of the latest samples (values are already scaled).
#[derive(Debug, Clone)]
pub(crate) struct Spark {
    pub(crate) values: Vec<f32>,
    pub(crate) color: Color,
    pub(crate) grid: Color,
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
        if self.values.len() >= 2 {
            let max = self.values.iter().copied().fold(f32::EPSILON, f32::max);
            let step = w / (self.values.len() - 1) as f32;
            let point =
                |i: usize, v: f32| Point::new(i as f32 * step, h - 4.0 - (v / max) * (h - 10.0));
            let line = Path::new(|b| {
                for (i, v) in self.values.iter().enumerate() {
                    if i == 0 {
                        b.move_to(point(i, *v));
                    } else {
                        b.line_to(point(i, *v));
                    }
                }
            });
            let area = Path::new(|b| {
                b.move_to(Point::new(0.0, h));
                for (i, v) in self.values.iter().enumerate() {
                    b.line_to(point(i, *v));
                }
                b.line_to(Point::new(w, h));
                b.close();
            });
            let fill = gradient::Linear::new(Point::new(0.0, 0.0), Point::new(0.0, h))
                .add_stop(
                    0.0,
                    Color {
                        a: 0.35,
                        ..self.color
                    },
                )
                .add_stop(
                    1.0,
                    Color {
                        a: 0.0,
                        ..self.color
                    },
                );
            frame.fill(&area, fill);
            frame.stroke(
                &line,
                Stroke::default()
                    .with_color(self.color)
                    .with_width(2.0)
                    .with_line_cap(LineCap::Round),
            );
            if let Some(last) = self.values.last() {
                let p = point(self.values.len() - 1, *last);
                frame.fill(&Path::circle(p, 4.0), self.color);
                frame.fill(
                    &Path::circle(p, 8.0),
                    Color {
                        a: 0.2,
                        ..self.color
                    },
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

pub(crate) fn spark<'a, M: 'a>(
    values: Vec<f32>,
    color: Color,
    grid: Color,
    height: f32,
) -> Element<'a, M> {
    canvas::Canvas::new(Spark {
        values,
        color,
        grid,
    })
    .width(Length::Fill)
    .height(Length::Fixed(height))
    .into()
}

/// Circular progress. `progress = None` draws an indeterminate comet whose
/// position follows `phase` (turns).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ring {
    pub(crate) progress: Option<f32>,
    pub(crate) phase: f32,
    pub(crate) color: Color,
    pub(crate) track: Color,
    pub(crate) width: f32,
}

impl<M> canvas::Program<M> for Ring {
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
        let center = frame.center();
        let radius = bounds.width.min(bounds.height) / 2.0 - self.width;
        frame.stroke(
            &Path::circle(center, radius),
            Stroke::default()
                .with_color(self.track)
                .with_width(self.width),
        );
        let (start, sweep) = match self.progress {
            Some(p) => (-PI / 2.0, p.clamp(0.0, 1.0) * 2.0 * PI),
            None => {
                let head = self.phase * 2.0 * PI;
                let breathe = 0.9 + 0.6 * (self.phase * 2.0 * PI).sin().abs();
                (head - PI / 2.0, breathe)
            }
        };
        if sweep > 0.001 {
            let arc = Path::new(|b| {
                b.arc(Arc {
                    center,
                    radius,
                    start_angle: Radians(start),
                    end_angle: Radians(start + sweep),
                });
            });
            frame.stroke(
                &arc,
                Stroke::default()
                    .with_color(self.color)
                    .with_width(self.width)
                    .with_line_cap(LineCap::Round),
            );
        }
        vec![frame.into_geometry()]
    }
}

pub(crate) fn ring<'a, M: 'a>(ring: Ring, size: f32) -> Element<'a, M> {
    canvas::Canvas::new(ring)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
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

/// Application mark: gradient squircle with the sync arrows, used for the
/// splash screen and the rail.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mark {
    pub(crate) from: Color,
    pub(crate) to: Color,
    pub(crate) turn: f32,
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
        let bg = Path::rounded_rectangle(Point::ORIGIN, Size::new(s, s), (s * 0.3).into());
        frame.fill(
            &bg,
            gradient::Linear::new(Point::ORIGIN, Point::new(s, s))
                .add_stop(0.0, self.from)
                .add_stop(1.0, self.to),
        );
        crate::icons::sync_arrows(
            &mut frame,
            Point::new(s / 2.0, s / 2.0),
            s * 0.27,
            s * 0.065,
            self.turn * 2.0 * PI,
            Color::WHITE,
        );
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
