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
/// splash screen and the rail (and rasterized for the window icon).
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
        let c = Point::new(s / 2.0, s / 2.0);
        let r = s * 0.27;
        let rot = self.turn * 2.0 * PI;
        let stroke = Stroke::default()
            .with_color(Color::WHITE)
            .with_width(s * 0.085)
            .with_line_cap(LineCap::Round);
        for base in [200.0_f32, 20.0] {
            let a0 = base.to_radians() + rot;
            let a1 = (base + 125.0).to_radians() + rot;
            frame.stroke(
                &Path::new(|b| {
                    b.arc(Arc {
                        center: c,
                        radius: r,
                        start_angle: Radians(a0),
                        end_angle: Radians(a1),
                    });
                }),
                stroke,
            );
            let tip = Point::new(c.x + r * a1.cos(), c.y + r * a1.sin());
            let (tx, ty) = (-a1.sin(), a1.cos());
            let (nx, ny) = (a1.cos(), a1.sin());
            let l = s * 0.12;
            frame.stroke(
                &Path::new(|b| {
                    b.move_to(Point::new(
                        tip.x - tx * l + nx * l * 0.8,
                        tip.y - ty * l + ny * l * 0.8,
                    ));
                    b.line_to(tip);
                    b.line_to(Point::new(
                        tip.x - tx * l - nx * l * 0.8,
                        tip.y - ty * l - ny * l * 0.8,
                    ));
                }),
                stroke,
            );
        }
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

/// Window icon: a rasterized version of [`Mark`].
pub(crate) fn window_icon() -> Option<iced::window::Icon> {
    iced::window::icon::from_rgba(icon_rgba(), ICON_SIZE, ICON_SIZE).ok()
}

/// [`Mark`] rasterized as straight RGBA, [`ICON_SIZE`]² (window and tray).
pub(crate) fn icon_rgba() -> Vec<u8> {
    const N: u32 = ICON_SIZE;
    let (from, to) = ([0x58_u8, 0x65, 0xf2], [0x8b_u8, 0x5c, 0xf6]);
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    let n = N as f32;
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded square coverage.
            let r = n * 0.3;
            let qx = (fx - n / 2.0).abs() - (n / 2.0 - r);
            let qy = (fy - n / 2.0).abs() - (n / 2.0 - r);
            let outside =
                (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
            let cover = (0.5 - outside).clamp(0.0, 1.0);
            let t = ((fx + fy) / (2.0 * n)).clamp(0.0, 1.0);
            let mut c = [0.0_f32; 3];
            for i in 0..3 {
                c[i] = f32::from(from[i]) + (f32::from(to[i]) - f32::from(from[i])) * t;
            }
            // Ring of the sync arrows with two gaps.
            let (dx, dy) = (fx - n / 2.0, fy - n / 2.0);
            let dist = (dx * dx + dy * dy).sqrt();
            let angle = dy.atan2(dx).to_degrees().rem_euclid(360.0);
            let on_arc = (200.0..325.0).contains(&angle) || (20.0..145.0).contains(&angle);
            let ring = (1.0 - ((dist - n * 0.27).abs() - n * 0.042).clamp(0.0, 1.0))
                * f32::from(u8::from(on_arc));
            for v in &mut c {
                *v += (255.0 - *v) * ring;
            }
            rgba.extend_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, (cover * 255.0) as u8]);
        }
    }
    rgba
}
