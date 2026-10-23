//! Small composable building blocks shared by every screen.

use crate::icons::{self, Glyph};
use crate::theme::{self, Palette, Tone};
use iced::font::Weight;
use iced::widget::{Space, button, column, container, row, text, text_input, tooltip};
use iced::{Alignment, Color, Element, Font, Length, Padding};

pub(crate) type El<'a, M> = Element<'a, M>;

/// Platform UI font with all weights available.
pub(crate) fn base_font() -> Font {
    if cfg!(windows) {
        Font::with_name("Segoe UI")
    } else {
        Font::DEFAULT
    }
}

pub(crate) fn weight(w: Weight) -> Font {
    Font {
        weight: w,
        ..base_font()
    }
}

pub(crate) fn title<'a, M: 'a>(p: &Palette, value: impl Into<String>, size: f32) -> El<'a, M> {
    text(value.into())
        .size(size)
        .font(weight(Weight::Bold))
        .color(p.text)
        .into()
}

pub(crate) fn heading<'a, M: 'a>(p: &Palette, value: impl Into<String>) -> El<'a, M> {
    text(value.into())
        .size(15)
        .font(weight(Weight::Semibold))
        .color(p.text)
        .into()
}

pub(crate) fn muted<'a, M: 'a>(p: &Palette, value: impl Into<String>) -> El<'a, M> {
    text(value.into()).size(13).color(p.muted).into()
}

pub(crate) fn caption<'a, M: 'a>(p: &Palette, value: impl Into<String>) -> El<'a, M> {
    text(value.into()).size(12).color(p.faint).into()
}

/// Uppercase small label (section and group headers, Discord style).
pub(crate) fn eyebrow<'a, M: 'a>(color: Color, value: &str) -> El<'a, M> {
    text(value.to_uppercase())
        .size(11)
        .font(weight(Weight::Bold))
        .color(color)
        .into()
}

pub(crate) fn mono<'a, M: 'a>(color: Color, value: impl Into<String>, size: f32) -> El<'a, M> {
    text(value.into())
        .size(size)
        .font(Font::MONOSPACE)
        .color(color)
        .wrapping(iced::widget::text::Wrapping::None)
        .into()
}

pub(crate) fn colored<'a, M: 'a>(color: Color, value: impl Into<String>, size: f32) -> El<'a, M> {
    text(value.into()).size(size).color(color).into()
}

pub(crate) fn strong<'a, M: 'a>(color: Color, value: impl Into<String>, size: f32) -> El<'a, M> {
    text(value.into())
        .size(size)
        .font(weight(Weight::Semibold))
        .color(color)
        .into()
}

pub(crate) fn hspace<'a, M: 'a>(w: f32) -> El<'a, M> {
    Space::new().width(Length::Fixed(w)).into()
}

pub(crate) fn vspace<'a, M: 'a>(h: f32) -> El<'a, M> {
    Space::new().height(Length::Fixed(h)).into()
}

pub(crate) fn fill<'a, M: 'a>() -> El<'a, M> {
    Space::new().width(Length::Fill).into()
}

/// Icon + label button. `on` = None renders it disabled.
pub(crate) fn action<'a, M: Clone + 'a>(
    p: &Palette,
    tone: Tone,
    glyph: Option<Glyph>,
    label: impl Into<String>,
    on: Option<M>,
) -> El<'a, M> {
    let fg = match tone {
        Tone::Primary | Tone::Danger | Tone::Success => p.on_accent,
        Tone::Secondary => p.text,
        Tone::Ghost => p.muted,
    };
    let fg = if on.is_some() {
        fg
    } else {
        theme::alpha(fg, 0.45)
    };
    let mut content = row![].spacing(8).align_y(Alignment::Center);
    if let Some(g) = glyph {
        content = content.push(icons::icon(g, 16.0, fg));
    }
    content = content.push(
        text(label.into())
            .size(14)
            .font(weight(Weight::Semibold))
            .color(fg),
    );
    button(content)
        .padding(Padding::from([8, 14]))
        .style(theme::btn(*p, tone))
        .on_press_maybe(on)
        .into()
}

/// Square icon-only button with a tooltip.
pub(crate) fn icon_button<'a, M: Clone + 'a>(
    p: &Palette,
    glyph: Glyph,
    hint: &str,
    on: Option<M>,
) -> El<'a, M> {
    let color = if on.is_some() { p.muted } else { p.faint };
    with_tip(
        p,
        button(icons::icon(glyph, 16.0, color))
            .padding(7)
            .style(theme::btn(*p, Tone::Ghost))
            .on_press_maybe(on),
        hint,
        tooltip::Position::Bottom,
    )
}

pub(crate) fn with_tip<'a, M: 'a>(
    p: &Palette,
    content: impl Into<El<'a, M>>,
    hint: &str,
    position: tooltip::Position,
) -> El<'a, M> {
    tooltip(
        content,
        container(text(hint.to_owned()).size(12))
            .padding(Padding::from([6, 10]))
            .style(theme::tooltip(*p)),
        position,
    )
    .gap(6)
    .delay(std::time::Duration::from_millis(350))
    .into()
}

/// Rounded status pill with a dot.
pub(crate) fn pill<'a, M: 'a>(color: Color, label: impl Into<String>) -> El<'a, M> {
    container(
        row![
            dot(color, 7.0),
            text(label.into())
                .size(12)
                .font(weight(Weight::Semibold))
                .color(color)
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([3, 10]))
    .style(theme::pill(theme::alpha(color, 0.14), color))
    .into()
}

/// Small solid badge (counts).
pub(crate) fn badge<'a, M: 'a>(bg: Color, fg: Color, label: impl Into<String>) -> El<'a, M> {
    container(
        text(label.into())
            .size(11)
            .font(weight(Weight::Bold))
            .color(fg),
    )
    .padding(Padding::from([1, 7]))
    .style(theme::pill(bg, fg))
    .into()
}

pub(crate) fn dot<'a, M: 'a>(color: Color, size: f32) -> El<'a, M> {
    container(Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(theme::pill(color, color))
        .into()
}

/// Keyboard shortcut chip.
pub(crate) fn kbd<'a, M: 'a>(p: &Palette, keys: &str) -> El<'a, M> {
    container(
        text(keys.to_owned())
            .size(11)
            .font(weight(Weight::Semibold))
            .color(p.muted),
    )
    .padding(Padding::from([2, 6]))
    .style(theme::outline(*p, p.faint, 0.0))
    .into()
}

pub(crate) fn divider<'a, M: 'a>(p: &Palette) -> El<'a, M> {
    container(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(theme::panel(p.border))
        .into()
}

/// Labeled text field with optional help or error line.
pub(crate) fn field<'a, M: Clone + 'a>(
    p: &Palette,
    label: &str,
    placeholder: &str,
    value: &str,
    on_input: impl Fn(String) -> M + 'a,
    help: Option<&str>,
    error: Option<&str>,
) -> El<'a, M> {
    let mut col = column![
        strong(p.muted, label.to_owned(), 12.0),
        text_input(placeholder, value)
            .on_input(on_input)
            .padding(Padding::from([9, 12]))
            .size(14)
            .style(theme::input(*p, error.is_some())),
    ]
    .spacing(6);
    if let Some(e) = error {
        col = col.push(
            row![
                icons::icon(Glyph::Alert, 13.0, p.danger),
                colored(p.danger, e.to_owned(), 12.0)
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    } else if let Some(h) = help {
        col = col.push(caption(p, h.to_owned()));
    }
    col.width(Length::Fill).into()
}

/// Colored callout box.
pub(crate) fn callout<'a, M: 'a>(
    p: &Palette,
    color: Color,
    glyph: Glyph,
    title_text: &str,
    message: impl Into<String>,
) -> El<'a, M> {
    container(
        row![
            icons::icon(glyph, 18.0, color),
            column![
                strong(p.text, title_text.to_owned(), 13.0),
                text(message.into()).size(13).color(p.muted)
            ]
            .spacing(2)
            .width(Length::Fill)
        ]
        .spacing(12)
        .align_y(Alignment::Start),
    )
    .padding(Padding::from([12, 14]))
    .width(Length::Fill)
    .style(theme::outline(*p, color, 1.0))
    .into()
}

/// Rounded square holding a glyph (avatars of profiles, modes, providers).
pub(crate) fn glyph_box<'a, M: 'a>(glyph: Glyph, color: Color, size: f32) -> El<'a, M> {
    container(icons::icon(glyph, size * 0.55, color))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .center_x(Length::Fixed(size))
        .center_y(Length::Fixed(size))
        .style(move |_| container::Style {
            background: Some(theme::alpha(color, 0.16).into()),
            border: iced::border::rounded(size * 0.3),
            ..container::Style::default()
        })
        .into()
}
