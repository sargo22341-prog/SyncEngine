//! Visual system: two hand-tuned palettes (dark « Discord-like », light
//! « classic Google-like ») blended continuously so that switching theme is an
//! animated cross-fade. Every style function captures a `Palette` by value.

use iced::widget::{button, checkbox, container, scrollable, text_input, toggler};
use iced::{Background, Border, Color, Shadow, Theme, Vector, border, color};

/// Resolved colors for one frame (already interpolated between themes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Palette {
    /// Darkest band: navigation rail.
    pub(crate) rail: Color,
    /// Secondary panel: profile sidebar.
    pub(crate) sidebar: Color,
    /// Main content background.
    pub(crate) bg: Color,
    /// Raised surfaces (cards, dialogs).
    pub(crate) surface: Color,
    /// Hovered surfaces and inputs.
    pub(crate) surface_hi: Color,
    /// Subtle separators.
    pub(crate) border: Color,
    pub(crate) text: Color,
    pub(crate) muted: Color,
    pub(crate) faint: Color,
    pub(crate) accent: Color,
    pub(crate) accent_hi: Color,
    /// Text drawn on top of the accent color.
    pub(crate) on_accent: Color,
    pub(crate) success: Color,
    pub(crate) warning: Color,
    pub(crate) danger: Color,
    pub(crate) info: Color,
    pub(crate) shadow: Color,
    /// Modal backdrop.
    pub(crate) scrim: Color,
}

impl Palette {
    pub(crate) const DARK: Self = Self {
        rail: color!(0x1e1f22),
        sidebar: color!(0x2b2d31),
        bg: color!(0x313338),
        surface: color!(0x2b2d31),
        surface_hi: color!(0x383a40),
        border: color!(0x3f4147),
        text: color!(0xdbdee1),
        muted: color!(0x949ba4),
        faint: color!(0x6d6f78),
        accent: color!(0x5865f2),
        accent_hi: color!(0x4752c4),
        on_accent: color!(0xffffff),
        success: color!(0x23a55a),
        warning: color!(0xf0b232),
        danger: color!(0xf23f43),
        info: color!(0x00a8fc),
        shadow: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
        scrim: Color::from_rgba(0.0, 0.0, 0.0, 0.6),
    };

    pub(crate) const LIGHT: Self = Self {
        rail: color!(0xe9eef6),
        sidebar: color!(0xf3f6fc),
        bg: color!(0xffffff),
        surface: color!(0xffffff),
        surface_hi: color!(0xf1f3f4),
        border: color!(0xdadce0),
        text: color!(0x1f1f1f),
        muted: color!(0x5f6368),
        faint: color!(0x80868b),
        accent: color!(0x1a73e8),
        accent_hi: color!(0x1765cc),
        on_accent: color!(0xffffff),
        success: color!(0x188038),
        warning: color!(0xe37400),
        danger: color!(0xd93025),
        info: color!(0x1a73e8),
        shadow: Color::from_rgba(0.24, 0.25, 0.26, 0.18),
        scrim: Color::from_rgba(0.12, 0.13, 0.15, 0.35),
    };

    /// Linear blend: `t = 0` is dark, `t = 1` is light.
    pub(crate) fn blend(t: f32) -> Self {
        let (a, b) = (Self::DARK, Self::LIGHT);
        let m = |x: Color, y: Color| mix(x, y, t);
        Self {
            rail: m(a.rail, b.rail),
            sidebar: m(a.sidebar, b.sidebar),
            bg: m(a.bg, b.bg),
            surface: m(a.surface, b.surface),
            surface_hi: m(a.surface_hi, b.surface_hi),
            border: m(a.border, b.border),
            text: m(a.text, b.text),
            muted: m(a.muted, b.muted),
            faint: m(a.faint, b.faint),
            accent: m(a.accent, b.accent),
            accent_hi: m(a.accent_hi, b.accent_hi),
            on_accent: m(a.on_accent, b.on_accent),
            success: m(a.success, b.success),
            warning: m(a.warning, b.warning),
            danger: m(a.danger, b.danger),
            info: m(a.info, b.info),
            shadow: m(a.shadow, b.shadow),
            scrim: m(a.scrim, b.scrim),
        }
    }

    /// Fades all foreground colors toward their background: used for step
    /// and page transitions (iced has no subtree opacity).
    pub(crate) fn faded(self, visibility: f32) -> Self {
        let v = visibility.clamp(0.0, 1.0);
        let f = |c: Color| alpha(c, c.a * v);
        Self {
            text: f(self.text),
            muted: f(self.muted),
            faint: f(self.faint),
            accent: f(self.accent),
            accent_hi: f(self.accent_hi),
            on_accent: f(self.on_accent),
            success: f(self.success),
            warning: f(self.warning),
            danger: f(self.danger),
            info: f(self.info),
            border: f(self.border),
            surface: mix(self.bg, self.surface, v),
            surface_hi: mix(self.bg, self.surface_hi, v),
            shadow: f(self.shadow),
            ..self
        }
    }

    /// Whether this palette is closer to the light theme.
    pub(crate) fn is_light(&self) -> bool {
        luminance(self.bg) > 0.5
    }

    /// Native iced theme used for widgets that are not explicitly styled.
    pub(crate) fn iced_theme(&self) -> Theme {
        Theme::custom(
            "SyncEngine",
            iced::theme::Palette {
                background: self.bg,
                text: self.text,
                primary: self.accent,
                success: self.success,
                warning: self.warning,
                danger: self.danger,
            },
        )
    }
}

pub(crate) fn mix(a: Color, b: Color, t: f32) -> Color {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    Color::from_rgba(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

pub(crate) fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

fn luminance(c: Color) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

/// Corner radii used across the application.
pub(crate) const RADIUS: f32 = 10.0;
pub(crate) const RADIUS_SM: f32 = 6.0;

fn shadow(p: &Palette, y: f32, blur: f32) -> Shadow {
    Shadow {
        color: p.shadow,
        offset: Vector::new(0.0, y),
        blur_radius: blur,
    }
}

// ----- containers -----------------------------------------------------------

pub(crate) fn panel(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(color.into()),
        ..container::Style::default()
    }
}

/// Elevated card; `lift` in 0..1 animates hover elevation and border glow.
pub(crate) fn card(p: Palette, lift: f32, selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(mix(p.surface, p.surface_hi, lift * 0.6).into()),
        text_color: Some(p.text),
        border: Border {
            color: if selected {
                p.accent
            } else {
                mix(p.border, p.accent, lift * 0.55)
            },
            width: if selected { 2.0 } else { 1.0 },
            radius: RADIUS.into(),
        },
        shadow: shadow(&p, 1.0 + 5.0 * lift, 4.0 + 14.0 * lift),
        snap: true,
    }
}

pub(crate) fn outline(p: Palette, color: Color, fill: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(alpha(color, 0.12 * fill).into()),
        text_color: Some(p.text),
        border: Border {
            color: alpha(color, 0.55),
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..container::Style::default()
    }
}

/// Dashed-looking drop zone highlight while dragging profiles.
pub(crate) fn drop_zone(p: Palette, active: bool, hot: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(alpha(p.accent, if active { 0.04 + 0.12 * hot } else { 0.0 }).into()),
        border: Border {
            color: if active {
                alpha(p.accent, 0.35 + 0.65 * hot)
            } else {
                Color::TRANSPARENT
            },
            width: if active { 1.5 } else { 0.0 },
            radius: RADIUS.into(),
        },
        ..container::Style::default()
    }
}

pub(crate) fn pill(bg: Color, fg: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(bg.into()),
        text_color: Some(fg),
        border: border::rounded(999),
        ..container::Style::default()
    }
}

pub(crate) fn dialog(p: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(p.surface.into()),
        text_color: Some(p.text),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: 14.0.into(),
        },
        shadow: shadow(&p, 18.0, 48.0),
        snap: true,
    }
}

pub(crate) fn menu_surface(p: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(if p.is_light() { p.surface } else { p.rail }.into()),
        text_color: Some(p.text),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: shadow(&p, 8.0, 24.0),
        snap: true,
    }
}

pub(crate) fn tooltip(p: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(
            if p.is_light() {
                color!(0x303134)
            } else {
                p.rail
            }
            .into(),
        ),
        text_color: Some(if p.is_light() { Color::WHITE } else { p.text }),
        border: border::rounded(RADIUS_SM),
        shadow: shadow(&p, 4.0, 12.0),
        snap: true,
    }
}

pub(crate) fn scrim(p: Palette, t: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(alpha(p.scrim, p.scrim.a * t).into()),
        ..container::Style::default()
    }
}

/// Colored left accent bar (selection indicator, toasts).
pub(crate) fn bar(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(color.into()),
        border: border::rounded(4),
        ..container::Style::default()
    }
}

// ----- buttons --------------------------------------------------------------

/// Visual intent of a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Primary,
    Secondary,
    Ghost,
    Danger,
    Success,
}

pub(crate) fn btn(p: Palette, tone: Tone) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered);
        let pressed = matches!(status, button::Status::Pressed);
        let disabled = matches!(status, button::Status::Disabled);
        let (base, hi, fg) = match tone {
            Tone::Primary => (p.accent, p.accent_hi, p.on_accent),
            Tone::Danger => (p.danger, mix(p.danger, Color::BLACK, 0.18), Color::WHITE),
            Tone::Success => (p.success, mix(p.success, Color::BLACK, 0.18), Color::WHITE),
            Tone::Secondary => (p.surface_hi, mix(p.surface_hi, p.text, 0.08), p.text),
            Tone::Ghost => (Color::TRANSPARENT, alpha(p.text, 0.08), p.muted),
        };
        let background = if pressed {
            mix(hi, Color::BLACK, 0.08)
        } else if hover {
            hi
        } else {
            base
        };
        button::Style {
            background: Some(Background::Color(if disabled {
                alpha(background, background.a * 0.45)
            } else {
                background
            })),
            text_color: if disabled {
                alpha(fg, 0.45)
            } else if tone == Tone::Ghost && hover {
                p.text
            } else {
                fg
            },
            border: Border {
                color: if tone == Tone::Secondary {
                    p.border
                } else {
                    Color::TRANSPARENT
                },
                width: if tone == Tone::Secondary { 1.0 } else { 0.0 },
                radius: RADIUS_SM.into(),
            },
            shadow: if hover && matches!(tone, Tone::Primary | Tone::Danger | Tone::Success) {
                Shadow {
                    color: alpha(base, 0.35),
                    offset: Vector::new(0.0, 3.0),
                    blur_radius: 10.0,
                }
            } else {
                Shadow::default()
            },
            snap: true,
        }
    }
}

/// Transparent clickable surface (rows, tiles) with a hover tint.
pub(crate) fn row_btn(
    p: Palette,
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let bg = if selected {
            alpha(p.accent, if p.is_light() { 0.12 } else { 0.22 })
        } else if hover {
            alpha(p.text, 0.06)
        } else {
            Color::TRANSPARENT
        };
        button::Style {
            background: Some(bg.into()),
            text_color: if selected || hover { p.text } else { p.muted },
            border: border::rounded(RADIUS_SM),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Selectable option tile (wizard choices).
pub(crate) fn tile(
    p: Palette,
    selected: bool,
    enabled: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = enabled && matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(
                if selected {
                    alpha(p.accent, if p.is_light() { 0.08 } else { 0.16 })
                } else if hover {
                    p.surface_hi
                } else {
                    p.surface
                }
                .into(),
            ),
            text_color: if enabled { p.text } else { p.faint },
            border: Border {
                color: if selected {
                    p.accent
                } else if hover {
                    mix(p.border, p.accent, 0.5)
                } else {
                    p.border
                },
                width: if selected { 2.0 } else { 1.0 },
                radius: RADIUS.into(),
            },
            shadow: if hover || selected {
                shadow(&p, 4.0, 14.0)
            } else {
                Shadow::default()
            },
            snap: true,
        }
    }
}

/// Rail navigation item: rounded square that becomes a squircle when active.
pub(crate) fn rail_btn(
    p: Palette,
    active: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(
                if active {
                    p.accent
                } else if hover {
                    mix(p.surface_hi, p.accent, 0.25)
                } else {
                    p.surface_hi
                }
                .into(),
            ),
            text_color: if active { p.on_accent } else { p.text },
            border: border::rounded(if active || hover { 14 } else { 22 }),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Small removable chip.
pub(crate) fn chip(
    p: Palette,
    color: Color,
    on: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(
                if on {
                    alpha(color, if hover { 0.30 } else { 0.18 })
                } else if hover {
                    alpha(p.text, 0.08)
                } else {
                    Color::TRANSPARENT
                }
                .into(),
            ),
            text_color: if on { p.text } else { p.muted },
            border: Border {
                color: if on { alpha(color, 0.7) } else { p.border },
                width: 1.0,
                radius: 999.0.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

pub(crate) fn menu_item(
    p: Palette,
    danger: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let tint = if danger { p.danger } else { p.accent };
        button::Style {
            background: Some(if hover { tint } else { Color::TRANSPARENT }.into()),
            text_color: if hover {
                Color::WHITE
            } else if danger {
                p.danger
            } else {
                p.text
            },
            border: border::rounded(4),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

// ----- inputs ---------------------------------------------------------------

pub(crate) fn input(
    p: Palette,
    invalid: bool,
) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| {
        let focused = matches!(status, text_input::Status::Focused { .. });
        let hovered = matches!(
            status,
            text_input::Status::Hovered | text_input::Status::Focused { is_hovered: true }
        );
        text_input::Style {
            background: (if p.is_light() { p.surface } else { p.rail }).into(),
            border: Border {
                color: if invalid {
                    p.danger
                } else if focused {
                    p.accent
                } else if hovered {
                    mix(p.border, p.text, 0.25)
                } else {
                    p.border
                },
                width: if focused { 2.0 } else { 1.0 },
                radius: RADIUS_SM.into(),
            },
            icon: p.muted,
            placeholder: p.faint,
            value: p.text,
            selection: alpha(p.accent, 0.35),
        }
    }
}

/// Search field embedded in a header: no border until focused.
pub(crate) fn search(p: Palette) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| {
        let focused = matches!(status, text_input::Status::Focused { .. });
        text_input::Style {
            background: (if p.is_light() { p.surface_hi } else { p.rail }).into(),
            border: Border {
                color: if focused {
                    p.accent
                } else {
                    Color::TRANSPARENT
                },
                width: 1.0,
                radius: RADIUS_SM.into(),
            },
            icon: p.muted,
            placeholder: p.faint,
            value: p.text,
            selection: alpha(p.accent, 0.35),
        }
    }
}

pub(crate) fn check(p: Palette) -> impl Fn(&Theme, checkbox::Status) -> checkbox::Style {
    move |_, status| {
        let (checked, hover, disabled) = match status {
            checkbox::Status::Active { is_checked } => (is_checked, false, false),
            checkbox::Status::Hovered { is_checked } => (is_checked, true, false),
            checkbox::Status::Disabled { is_checked } => (is_checked, false, true),
        };
        checkbox::Style {
            background: (if checked {
                if disabled {
                    alpha(p.accent, 0.4)
                } else {
                    p.accent
                }
            } else if hover {
                p.surface_hi
            } else {
                Color::TRANSPARENT
            })
            .into(),
            icon_color: p.on_accent,
            border: Border {
                color: if checked { p.accent } else { p.faint },
                width: 1.5,
                radius: 4.0.into(),
            },
            text_color: Some(if disabled { p.faint } else { p.text }),
        }
    }
}

pub(crate) fn switch(
    p: Palette,
    danger: bool,
) -> impl Fn(&Theme, toggler::Status) -> toggler::Style {
    move |_, status| {
        let (on, hover, disabled) = match status {
            toggler::Status::Active { is_toggled } => (is_toggled, false, false),
            toggler::Status::Hovered { is_toggled } => (is_toggled, true, false),
            toggler::Status::Disabled { is_toggled } => (is_toggled, false, true),
        };
        let tint = if danger { p.danger } else { p.success };
        toggler::Style {
            background: (if on {
                if hover {
                    mix(tint, Color::WHITE, 0.1)
                } else {
                    tint
                }
            } else {
                p.faint
            })
            .into(),
            background_border_width: 0.0,
            background_border_color: Color::TRANSPARENT,
            foreground: (if disabled {
                alpha(Color::WHITE, 0.5)
            } else {
                Color::WHITE
            })
            .into(),
            foreground_border_width: 0.0,
            foreground_border_color: Color::TRANSPARENT,
            text_color: Some(p.text),
            border_radius: None,
            padding_ratio: 0.12,
        }
    }
}

pub(crate) fn scroll(p: Palette) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    move |_, status| {
        let active = matches!(
            status,
            scrollable::Status::Hovered {
                is_vertical_scrollbar_hovered: true,
                ..
            } | scrollable::Status::Dragged { .. }
        );
        let rail = scrollable::Rail {
            background: None,
            border: Border::default(),
            scroller: scrollable::Scroller {
                background: (if active {
                    alpha(p.text, 0.35)
                } else {
                    alpha(p.text, 0.14)
                })
                .into(),
                border: border::rounded(999),
            },
        };
        scrollable::Style {
            container: container::Style::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: p.surface.into(),
                border: border::rounded(999),
                shadow: shadow(&p, 2.0, 8.0),
                icon: p.text,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_endpoints_match_the_reference_palettes() {
        assert_eq!(Palette::blend(0.0), Palette::DARK);
        assert_eq!(Palette::blend(1.0), Palette::LIGHT);
        assert!(!Palette::DARK.is_light());
        assert!(Palette::LIGHT.is_light());
    }

    #[test]
    fn fading_keeps_backgrounds_and_hides_text() {
        let faded = Palette::DARK.faded(0.0);
        assert_eq!(faded.bg, Palette::DARK.bg);
        assert!(faded.text.a.abs() < f32::EPSILON);
        assert_eq!(Palette::DARK.faded(1.0).text, Palette::DARK.text);
    }
}
