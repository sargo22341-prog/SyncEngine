//! Hand-drawn vector icons rendered with the iced canvas: no image assets,
//! no icon font, crisp at any scale and animatable (rotation, progress).

use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, LineCap, LineJoin, Path, Stroke, path::Arc};
use iced::{Color, Element, Length, Point, Radians, Rectangle, Renderer, Theme};
use std::f32::consts::PI;

/// Icon catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Glyph {
    Logo,
    Sync,
    Backup,
    Mirror,
    TwoWay,
    Folder,
    Server,
    Network,
    Cloud,
    Globe,
    Terminal,
    Lock,
    Play,
    Stop,
    Pencil,
    Clock,
    Trash,
    Plus,
    Search,
    Sliders,
    History,
    Pulse,
    Grid,
    Check,
    Close,
    Alert,
    Info,
    Shield,
    Moon,
    Sun,
    Monitor,
    ChevronDown,
    ChevronUp,
    ChevronRight,
    ChevronLeft,
    Filter,
    ArrowRight,
    Copy,
    Eye,
    Bolt,
    Layers,
    Calendar,
    Refresh,
    Power,
}

/// Canvas program drawing one glyph.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Icon {
    glyph: Glyph,
    color: Color,
    weight: f32,
    rotation: f32,
}

/// Square icon element of `size` logical pixels.
pub(crate) fn icon<'a, M: 'a>(glyph: Glyph, size: f32, color: Color) -> Element<'a, M> {
    canvas(
        Icon {
            glyph,
            color,
            weight: 1.0,
            rotation: 0.0,
        },
        size,
    )
}

/// Icon rotated by `turns` (1.0 = full turn), for spinners.
pub(crate) fn spinning<'a, M: 'a>(
    glyph: Glyph,
    size: f32,
    color: Color,
    turns: f32,
) -> Element<'a, M> {
    canvas(
        Icon {
            glyph,
            color,
            weight: 1.0,
            rotation: turns * 2.0 * PI,
        },
        size,
    )
}

/// Bolder variant for large illustrations.
pub(crate) fn bold<'a, M: 'a>(glyph: Glyph, size: f32, color: Color) -> Element<'a, M> {
    canvas(
        Icon {
            glyph,
            color,
            weight: 0.8,
            rotation: 0.0,
        },
        size,
    )
}

fn canvas<'a, M: 'a>(program: Icon, size: f32) -> Element<'a, M> {
    iced::widget::canvas(program)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

struct Pen<'f> {
    frame: &'f mut Frame,
    scale: f32,
    rotation: f32,
    stroke: Stroke<'static>,
    color: Color,
}

impl Pen<'_> {
    fn p(&self, x: f32, y: f32) -> Point {
        let (dx, dy) = (x - 12.0, y - 12.0);
        let (s, c) = self.rotation.sin_cos();
        Point::new(
            (12.0 + dx * c - dy * s) * self.scale,
            (12.0 + dx * s + dy * c) * self.scale,
        )
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32)) {
        let path = Path::line(self.p(a.0, a.1), self.p(b.0, b.1));
        self.frame.stroke(&path, self.stroke);
    }
    fn poly(&mut self, points: &[(f32, f32)], close: bool) {
        let path = Path::new(|b| {
            for (i, (x, y)) in points.iter().enumerate() {
                if i == 0 {
                    b.move_to(self.p(*x, *y));
                } else {
                    b.line_to(self.p(*x, *y));
                }
            }
            if close {
                b.close();
            }
        });
        self.frame.stroke(&path, self.stroke);
    }
    fn circle(&mut self, x: f32, y: f32, r: f32) {
        let path = Path::circle(self.p(x, y), r * self.scale);
        self.frame.stroke(&path, self.stroke);
    }
    fn dot(&mut self, x: f32, y: f32, r: f32) {
        let path = Path::circle(self.p(x, y), r * self.scale);
        self.frame.fill(&path, self.color);
    }
    /// Arc in degrees, clockwise from +x in screen coordinates.
    fn arc(&mut self, x: f32, y: f32, r: f32, from: f32, to: f32) {
        let rot = self.rotation;
        let path = Path::new(|b| {
            b.arc(Arc {
                center: self.p(x, y),
                radius: r * self.scale,
                start_angle: Radians(from.to_radians() + rot),
                end_angle: Radians(to.to_radians() + rot),
            });
        });
        self.frame.stroke(&path, self.stroke);
    }
    /// Outline of circle 1 minus circle 2 (both must intersect).
    fn crescent(&mut self, c1: (f32, f32), r1: f32, c2: (f32, f32), r2: f32) {
        let (dx, dy) = (c2.0 - c1.0, c2.1 - c1.1);
        let d = dx.hypot(dy);
        let toward = dy.atan2(dx);
        let a = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
        let phi = (a / r1).clamp(-1.0, 1.0).acos();
        let psi = ((d - a) / r2).clamp(-1.0, 1.0).acos();
        let rot = self.rotation;
        let path = Path::new(|b| {
            b.arc(Arc {
                center: self.p(c1.0, c1.1),
                radius: r1 * self.scale,
                start_angle: Radians(toward + phi + rot),
                end_angle: Radians(toward + 2.0 * PI - phi + rot),
            });
            let back = toward + PI;
            b.arc(Arc {
                center: self.p(c2.0, c2.1),
                radius: r2 * self.scale,
                start_angle: Radians(back + psi + rot),
                end_angle: Radians(back - psi + rot),
            });
        });
        self.frame.stroke(&path, self.stroke);
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let (x2, y2) = (x + w, y + h);
        // Rounded rectangle built from arcs so it rotates correctly.
        let rot = self.rotation;
        let path = Path::new(|b| {
            b.move_to(self.p(x + r, y));
            b.line_to(self.p(x2 - r, y));
            b.arc(Arc {
                center: self.p(x2 - r, y + r),
                radius: r * self.scale,
                start_angle: Radians(-PI / 2.0 + rot),
                end_angle: Radians(rot),
            });
            b.line_to(self.p(x2, y2 - r));
            b.arc(Arc {
                center: self.p(x2 - r, y2 - r),
                radius: r * self.scale,
                start_angle: Radians(rot),
                end_angle: Radians(PI / 2.0 + rot),
            });
            b.line_to(self.p(x + r, y2));
            b.arc(Arc {
                center: self.p(x + r, y2 - r),
                radius: r * self.scale,
                start_angle: Radians(PI / 2.0 + rot),
                end_angle: Radians(PI + rot),
            });
            b.line_to(self.p(x, y + r));
            b.arc(Arc {
                center: self.p(x + r, y + r),
                radius: r * self.scale,
                start_angle: Radians(PI + rot),
                end_angle: Radians(1.5 * PI + rot),
            });
            b.close();
        });
        self.frame.stroke(&path, self.stroke);
    }
    fn fill_poly(&mut self, points: &[(f32, f32)]) {
        let path = Path::new(|b| {
            for (i, (x, y)) in points.iter().enumerate() {
                if i == 0 {
                    b.move_to(self.p(*x, *y));
                } else {
                    b.line_to(self.p(*x, *y));
                }
            }
            b.close();
        });
        self.frame.fill(&path, self.color);
    }
    /// Arrow head at the end of an arc segment, tangent to the circle.
    fn arc_head(&mut self, x: f32, y: f32, r: f32, at: f32, clockwise: bool) {
        let a = at.to_radians();
        let tip = (x + r * a.cos(), y + r * a.sin());
        let dir = if clockwise { 1.0 } else { -1.0 };
        let (tx, ty) = (-a.sin() * dir, a.cos() * dir);
        let (nx, ny) = (a.cos(), a.sin());
        let len = 3.2;
        self.poly(
            &[
                (
                    tip.0 - tx * len + nx * len * 0.8,
                    tip.1 - ty * len + ny * len * 0.8,
                ),
                tip,
                (
                    tip.0 - tx * len - nx * len * 0.8,
                    tip.1 - ty * len - ny * len * 0.8,
                ),
            ],
            false,
        );
    }
}

impl<M> canvas::Program<M> for Icon {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let scale = bounds.width.min(bounds.height) / 24.0;
        let stroke = Stroke::default()
            .with_color(self.color)
            .with_width((2.0 * scale / self.weight.max(0.1)).max(1.0))
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round);
        let mut pen = Pen {
            frame: &mut frame,
            scale,
            rotation: self.rotation,
            stroke,
            color: self.color,
        };
        draw(self.glyph, &mut pen);
        vec![frame.into_geometry()]
    }
}

fn draw(glyph: Glyph, d: &mut Pen<'_>) {
    match glyph {
        Glyph::Logo | Glyph::Sync => {
            d.arc(12.0, 12.0, 7.5, 200.0, 330.0);
            d.arc_head(12.0, 12.0, 7.5, 330.0, true);
            d.arc(12.0, 12.0, 7.5, 20.0, 150.0);
            d.arc_head(12.0, 12.0, 7.5, 150.0, true);
            if glyph == Glyph::Logo {
                d.dot(12.0, 12.0, 2.0);
            }
        }
        Glyph::Refresh => {
            d.arc(12.0, 12.0, 7.5, -60.0, 240.0);
            d.arc_head(12.0, 12.0, 7.5, 240.0, true);
        }
        Glyph::Power => {
            d.arc(12.0, 13.0, 7.5, -45.0, 225.0);
            d.line((12.0, 3.5), (12.0, 11.0));
        }
        Glyph::Backup => {
            d.rect(4.0, 12.0, 16.0, 8.0, 2.0);
            d.line((12.0, 3.0), (12.0, 14.0));
            d.poly(&[(8.5, 10.5), (12.0, 14.0), (15.5, 10.5)], false);
            d.line((8.0, 16.5), (16.0, 16.5));
        }
        Glyph::Mirror => {
            d.line((12.0, 3.0), (12.0, 5.0));
            d.line((12.0, 8.0), (12.0, 10.0));
            d.line((12.0, 13.0), (12.0, 15.0));
            d.line((12.0, 18.0), (12.0, 21.0));
            d.poly(&[(9.0, 5.0), (9.0, 19.0), (3.0, 19.0)], true);
            d.poly(&[(15.0, 5.0), (15.0, 19.0), (21.0, 19.0)], true);
        }
        Glyph::TwoWay => {
            d.line((4.0, 8.0), (20.0, 8.0));
            d.poly(&[(16.0, 4.0), (20.0, 8.0), (16.0, 12.0)], false);
            d.line((20.0, 16.0), (4.0, 16.0));
            d.poly(&[(8.0, 12.0), (4.0, 16.0), (8.0, 20.0)], false);
        }
        Glyph::Folder => {
            d.poly(
                &[
                    (3.0, 7.0),
                    (3.0, 18.5),
                    (21.0, 18.5),
                    (21.0, 8.5),
                    (12.0, 8.5),
                    (10.0, 5.5),
                    (3.0, 5.5),
                ],
                true,
            );
        }
        Glyph::Server => {
            d.rect(3.5, 4.0, 17.0, 7.0, 2.0);
            d.rect(3.5, 13.0, 17.0, 7.0, 2.0);
            d.dot(7.5, 7.5, 1.1);
            d.dot(7.5, 16.5, 1.1);
            d.line((12.0, 7.5), (17.0, 7.5));
            d.line((12.0, 16.5), (17.0, 16.5));
        }
        Glyph::Network => {
            d.rect(9.0, 3.0, 6.0, 5.0, 1.0);
            d.rect(2.5, 16.0, 6.0, 5.0, 1.0);
            d.rect(15.5, 16.0, 6.0, 5.0, 1.0);
            d.line((12.0, 8.0), (12.0, 12.0));
            d.poly(
                &[(5.5, 16.0), (5.5, 12.0), (18.5, 12.0), (18.5, 16.0)],
                false,
            );
        }
        Glyph::Cloud => {
            d.line((7.0, 18.0), (17.0, 18.0));
            d.arc(7.0, 14.0, 4.0, 90.0, 270.0);
            d.arc(11.5, 10.5, 5.0, 205.0, 330.0);
            d.arc(17.0, 13.5, 4.5, 270.0, 450.0);
        }
        Glyph::Globe => {
            d.circle(12.0, 12.0, 9.0);
            d.line((3.0, 12.0), (21.0, 12.0));
            d.arc(20.0, 12.0, 11.0, 145.0, 215.0);
            d.arc(4.0, 12.0, 11.0, -35.0, 35.0);
        }
        Glyph::Terminal => {
            d.rect(3.0, 4.5, 18.0, 15.0, 2.5);
            d.poly(&[(7.0, 9.5), (10.0, 12.0), (7.0, 14.5)], false);
            d.line((12.0, 15.0), (17.0, 15.0));
        }
        Glyph::Lock => {
            d.rect(5.0, 11.0, 14.0, 10.0, 2.0);
            d.arc(12.0, 9.0, 4.0, 180.0, 360.0);
            d.line((8.0, 9.0), (8.0, 11.0));
            d.line((16.0, 9.0), (16.0, 11.0));
            d.dot(12.0, 16.0, 1.2);
        }
        Glyph::Play => {
            d.poly(&[(7.5, 4.5), (19.0, 12.0), (7.5, 19.5)], true);
        }
        Glyph::Stop => {
            d.rect(6.0, 6.0, 12.0, 12.0, 2.0);
        }
        Glyph::Pencil => {
            d.poly(
                &[
                    (15.5, 4.5),
                    (19.5, 8.5),
                    (8.5, 19.5),
                    (4.5, 19.5),
                    (4.5, 15.5),
                ],
                true,
            );
            d.line((13.0, 7.0), (17.0, 11.0));
        }
        Glyph::Clock => {
            d.circle(12.0, 12.0, 9.0);
            d.poly(&[(12.0, 7.0), (12.0, 12.0), (15.5, 14.0)], false);
        }
        Glyph::Trash => {
            d.line((4.0, 7.0), (20.0, 7.0));
            d.poly(&[(9.0, 7.0), (9.5, 4.0), (14.5, 4.0), (15.0, 7.0)], false);
            d.poly(&[(6.0, 7.0), (7.0, 20.0), (17.0, 20.0), (18.0, 7.0)], false);
            d.line((10.0, 11.0), (10.0, 16.0));
            d.line((14.0, 11.0), (14.0, 16.0));
        }
        Glyph::Plus => {
            d.line((12.0, 5.0), (12.0, 19.0));
            d.line((5.0, 12.0), (19.0, 12.0));
        }
        Glyph::Search => {
            d.circle(10.5, 10.5, 6.5);
            d.line((15.5, 15.5), (20.0, 20.0));
        }
        Glyph::Sliders => {
            d.line((4.0, 7.0), (20.0, 7.0));
            d.line((4.0, 17.0), (20.0, 17.0));
            d.dot(9.0, 7.0, 2.6);
            d.dot(15.0, 17.0, 2.6);
        }
        Glyph::History => {
            d.arc(12.0, 12.0, 8.5, 200.0, 520.0);
            d.poly(&[(3.0, 6.5), (4.2, 9.2), (7.0, 8.4)], false);
            d.poly(&[(12.0, 8.0), (12.0, 12.0), (14.8, 13.8)], false);
        }
        Glyph::Pulse => {
            d.poly(
                &[
                    (2.5, 12.0),
                    (7.0, 12.0),
                    (9.5, 6.0),
                    (14.0, 18.0),
                    (16.5, 12.0),
                    (21.5, 12.0),
                ],
                false,
            );
        }
        Glyph::Grid => {
            d.rect(4.0, 4.0, 7.0, 7.0, 1.5);
            d.rect(13.0, 4.0, 7.0, 7.0, 1.5);
            d.rect(4.0, 13.0, 7.0, 7.0, 1.5);
            d.rect(13.0, 13.0, 7.0, 7.0, 1.5);
        }
        Glyph::Check => {
            d.poly(&[(5.0, 12.5), (10.0, 17.5), (19.0, 7.0)], false);
        }
        Glyph::Close => {
            d.line((6.0, 6.0), (18.0, 18.0));
            d.line((18.0, 6.0), (6.0, 18.0));
        }
        Glyph::Alert => {
            d.poly(&[(12.0, 3.5), (21.5, 20.0), (2.5, 20.0)], true);
            d.line((12.0, 9.5), (12.0, 14.0));
            d.dot(12.0, 17.0, 1.2);
        }
        Glyph::Info => {
            d.circle(12.0, 12.0, 9.0);
            d.line((12.0, 11.0), (12.0, 16.5));
            d.dot(12.0, 7.8, 1.2);
        }
        Glyph::Shield => {
            d.poly(
                &[
                    (12.0, 3.0),
                    (19.5, 6.0),
                    (19.0, 13.0),
                    (12.0, 21.0),
                    (5.0, 13.0),
                    (4.5, 6.0),
                ],
                true,
            );
            d.poly(&[(8.5, 12.0), (11.0, 14.5), (15.5, 9.5)], false);
        }
        Glyph::Moon => d.crescent((12.0, 12.0), 8.5, (16.5, 7.5), 6.5),
        Glyph::Sun => {
            d.circle(12.0, 12.0, 4.0);
            for i in 0..8 {
                let a = (i as f32) * PI / 4.0;
                d.line(
                    (12.0 + 7.0 * a.cos(), 12.0 + 7.0 * a.sin()),
                    (12.0 + 9.5 * a.cos(), 12.0 + 9.5 * a.sin()),
                );
            }
        }
        Glyph::Monitor => {
            d.rect(3.0, 4.0, 18.0, 12.5, 2.0);
            d.line((12.0, 16.5), (12.0, 20.0));
            d.line((8.0, 20.0), (16.0, 20.0));
        }
        Glyph::ChevronDown => d.poly(&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)], false),
        Glyph::ChevronUp => d.poly(&[(6.0, 15.0), (12.0, 9.0), (18.0, 15.0)], false),
        Glyph::ChevronRight => d.poly(&[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)], false),
        Glyph::ChevronLeft => d.poly(&[(15.0, 6.0), (9.0, 12.0), (15.0, 18.0)], false),
        Glyph::Filter => {
            d.poly(
                &[
                    (3.5, 5.0),
                    (20.5, 5.0),
                    (14.0, 12.5),
                    (14.0, 19.0),
                    (10.0, 17.0),
                    (10.0, 12.5),
                ],
                true,
            );
        }
        Glyph::ArrowRight => {
            d.line((4.0, 12.0), (20.0, 12.0));
            d.poly(&[(14.0, 6.0), (20.0, 12.0), (14.0, 18.0)], false);
        }
        Glyph::Copy => {
            d.rect(8.0, 8.0, 12.0, 12.0, 2.0);
            d.poly(
                &[
                    (16.0, 8.0),
                    (16.0, 4.0),
                    (4.0, 4.0),
                    (4.0, 16.0),
                    (8.0, 16.0),
                ],
                false,
            );
        }
        Glyph::Eye => {
            d.arc(12.0, 19.0, 11.0, 220.0, 320.0);
            d.arc(12.0, 5.0, 11.0, 40.0, 140.0);
            d.circle(12.0, 12.0, 3.0);
        }
        Glyph::Bolt => {
            d.fill_poly(&[
                (13.5, 2.5),
                (5.0, 13.5),
                (11.0, 13.5),
                (10.0, 21.5),
                (19.0, 10.0),
                (13.0, 10.0),
            ]);
        }
        Glyph::Layers => {
            d.poly(&[(12.0, 3.5), (21.0, 8.0), (12.0, 12.5), (3.0, 8.0)], true);
            d.poly(&[(3.0, 12.0), (12.0, 16.5), (21.0, 12.0)], false);
            d.poly(&[(3.0, 16.0), (12.0, 20.5), (21.0, 16.0)], false);
        }
        Glyph::Calendar => {
            d.rect(3.5, 5.0, 17.0, 15.5, 2.5);
            d.line((3.5, 10.0), (20.5, 10.0));
            d.line((8.0, 3.0), (8.0, 7.0));
            d.line((16.0, 3.0), (16.0, 7.0));
        }
    }
}
