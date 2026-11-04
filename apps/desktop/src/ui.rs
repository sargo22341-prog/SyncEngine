//! Small composable building blocks shared by every screen.

use crate::icons::{self, Glyph};
use crate::theme::{self, Palette, Tone};
use iced::font::Weight;
use iced::widget::{Space, button, column, container, row, text, text_input, tooltip};
use iced::{Alignment, Color, Element, Font, Length, Padding};

pub(crate) type El<'a, M> = Element<'a, M>;

/// Embedded fonts (SIL OFL, see `assets/fonts/OFL-*.txt`): the application is
/// portable and must look the same without installed fonts. Static instances,
/// because the text renderer does not apply the weight axis of variable fonts.
pub(crate) const FONT_FILES: [&[u8]; 9] = [
    include_bytes!("../assets/fonts/Inter-400.ttf"),
    include_bytes!("../assets/fonts/Inter-500.ttf"),
    include_bytes!("../assets/fonts/Inter-600.ttf"),
    include_bytes!("../assets/fonts/Inter-700.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-500.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-700.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-400.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-500.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-700.ttf"),
];

/// Text font: Inter, with every weight the interface uses.
pub(crate) fn base_font() -> Font {
    Font::with_name("Inter")
}

/// Titles and figures: Space Grotesk (500 and 700 are embedded).
pub(crate) fn display_font(w: Weight) -> Font {
    Font {
        weight: w,
        ..Font::with_name("Space Grotesk")
    }
}

/// Paths and numbers: JetBrains Mono.
pub(crate) fn mono_font() -> Font {
    Font::with_name("JetBrains Mono")
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
        .font(display_font(Weight::Bold))
        .color(p.text)
        .into()
}

pub(crate) fn heading<'a, M: 'a>(p: &Palette, value: impl Into<String>) -> El<'a, M> {
    text(value.into())
        .size(15)
        .font(display_font(Weight::Medium))
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
        .font(mono_font())
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

/// Icon + label button (40 px high pill). `on` = None renders it disabled.
pub(crate) fn action<'a, M: Clone + 'a>(
    p: &Palette,
    tone: Tone,
    glyph: Option<Glyph>,
    label: impl Into<String>,
    on: Option<M>,
) -> El<'a, M> {
    button_pill(
        p,
        tone,
        glyph,
        label.into(),
        on,
        14.0,
        Padding::from([10, 20]),
    )
}

/// Compact variant (32 px high) for rows and toolbars.
pub(crate) fn action_sm<'a, M: Clone + 'a>(
    p: &Palette,
    tone: Tone,
    glyph: Option<Glyph>,
    label: impl Into<String>,
    on: Option<M>,
) -> El<'a, M> {
    button_pill(
        p,
        tone,
        glyph,
        label.into(),
        on,
        13.0,
        Padding::from([7, 14]),
    )
}

fn button_pill<'a, M: Clone + 'a>(
    p: &Palette,
    tone: Tone,
    glyph: Option<Glyph>,
    label: String,
    on: Option<M>,
    size: f32,
    padding: Padding,
) -> El<'a, M> {
    let fg = match tone {
        // Primary is the ink button (text color on the page color); lime
        // takes dark ink; danger keeps white.
        Tone::Primary => p.bg,
        Tone::Success => Color::from_rgb(0.043, 0.047, 0.055),
        Tone::Danger => Color::WHITE,
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
        content = content.push(icons::icon(g, size + 2.0, fg));
    }
    content = content.push(
        text(label)
            .size(size)
            .font(weight(Weight::Semibold))
            .color(fg),
    );
    button(content)
        .padding(padding)
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

/// « i » icon placed after a label: hovering it shows a longer, wrapped
/// explanation written for people who do not know the synchronization logic.
pub(crate) fn info_tip<'a, M: 'a>(p: &Palette, hint: &str) -> El<'a, M> {
    tooltip(
        container(icons::icon(Glyph::Info, 15.0, p.accent)).padding(2),
        container(text(hint.to_owned()).size(12).line_height(1.45))
            .padding(Padding::from([10, 12]))
            .max_width(400.0)
            .style(theme::tooltip(*p)),
        tooltip::Position::Top,
    )
    .gap(6)
    .delay(std::time::Duration::from_millis(150))
    .into()
}

/// Flat tinted chip (mode, option, counter): 12 px semibold on a soft wash.
pub(crate) fn chip<'a, M: 'a>(bg: Color, fg: Color, label: impl Into<String>) -> El<'a, M> {
    container(
        text(label.into())
            .size(12)
            .font(weight(Weight::Semibold))
            .color(fg)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .padding(Padding::from([3, 10]))
    .style(theme::pill(bg, fg))
    .into()
}

/// Neutral chip on the secondary surface.
pub(crate) fn tag<'a, M: 'a>(p: &Palette, label: impl Into<String>) -> El<'a, M> {
    chip(p.surface_hi, p.muted, label)
}

/// Mode chip: soft wash of the mode color with its text-safe ink.
pub(crate) fn mode_chip<'a, M: 'a>(p: &Palette, mode: sync_plan::SyncMode) -> El<'a, M> {
    let ink = crate::shell::mode_color(p, mode);
    let glyph = match mode {
        sync_plan::SyncMode::Backup => "→",
        sync_plan::SyncMode::Mirror => "⇉",
        _ => "⇄",
    };
    chip(
        theme::alpha(
            crate::shell::mode_fill(p, mode),
            if p.is_light() { 0.2 } else { 0.16 },
        ),
        ink,
        format!("{glyph} {}", crate::model::mode_label(mode)),
    )
}

/// Square rounded tile in the mode color with its arrow glyph.
pub(crate) fn mode_tile<'a, M: 'a>(p: &Palette, mode: sync_plan::SyncMode, size: f32) -> El<'a, M> {
    let ink = if mode == sync_plan::SyncMode::Bidirectional {
        p.bg
    } else {
        Color::from_rgb(0.043, 0.047, 0.055)
    };
    container(icons::icon(
        crate::shell::mode_glyph(mode),
        size * 0.46,
        ink,
    ))
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .center(Length::Fixed(size))
    .style(theme::swatch(crate::shell::mode_fill(p, mode)))
    .into()
}

/// Status: colored dot and muted label, no background.
pub(crate) fn status<'a, M: 'a>(p: &Palette, color: Color, label: impl Into<String>) -> El<'a, M> {
    row![dot(color, 8.0), colored(p.muted, label, 12.0)]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

/// Big figure with a small unit next to it (monospace), e.g. `37,4 Mo/s`.
pub(crate) fn metric<'a, M: 'a>(
    p: &Palette,
    value: impl Into<String>,
    unit: impl Into<String>,
    size: f32,
) -> El<'a, M> {
    let unit = unit.into();
    let mut line = row![
        text(value.into())
            .size(size)
            .font(Font {
                weight: Weight::Medium,
                ..mono_font()
            })
            .color(p.text)
            .wrapping(iced::widget::text::Wrapping::None)
    ]
    .spacing(6)
    .align_y(Alignment::End);
    if !unit.is_empty() {
        line = line.push(
            container(
                text(unit)
                    .size((size * 0.3).clamp(12.0, 15.0))
                    .font(mono_font())
                    .color(p.muted),
            )
            .padding(Padding {
                bottom: size * 0.12,
                ..Padding::ZERO
            }),
        );
    }
    line.into()
}

/// Horizontal progress bar (`fraction` in 0..=1) on a neutral track.
pub(crate) fn progress<'a, M: 'a>(
    p: &Palette,
    fraction: f32,
    color: Color,
    height: f32,
) -> El<'a, M> {
    let done = (fraction.clamp(0.0, 1.0) * 1000.0).round() as u16;
    let bar = |fill: Color, portion: u16| -> El<'a, M> {
        container(Space::new())
            .width(Length::FillPortion(portion.max(1)))
            .height(Length::Fixed(height))
            .style(theme::pill(fill, fill))
            .into()
    };
    let mut line = row![].width(Length::Fill);
    if done > 0 {
        line = line.push(bar(color, done));
    }
    if done < 1000 {
        line = line.push(
            container(Space::new())
                .width(Length::FillPortion(1000 - done))
                .height(Length::Fixed(height)),
        );
    }
    container(line)
        .width(Length::Fill)
        .style(theme::pill(p.surface_hi, p.surface_hi))
        .into()
}

/// Setting row: toggle, title with an optional explanation, optional widget
/// on the right (a size field, a counter…).
pub(crate) fn toggle_row<'a, M: Clone + 'a>(
    p: &Palette,
    on: bool,
    title: &str,
    sub: Option<&str>,
    danger: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
    right: Option<El<'a, M>>,
) -> El<'a, M> {
    let mut text_col = column![strong(p.text, title.to_owned(), 14.0)].spacing(2);
    if let Some(sub) = sub {
        text_col = text_col.push(colored(p.muted, sub.to_owned(), 12.0));
    }
    let mut line = row![
        iced::widget::toggler(on)
            .on_toggle(on_toggle)
            .size(22)
            .style(theme::switch(*p, danger)),
        container(text_col).width(Length::Fill),
    ]
    .spacing(14)
    .align_y(Alignment::Center);
    if let Some(right) = right {
        line = line.push(right);
    }
    container(line).padding(Padding::from([7, 0])).into()
}

/// Segmented control.
pub(crate) fn seg<'a, M: Clone + 'a, T: Copy + PartialEq + 'a>(
    p: &Palette,
    options: &[(T, &'a str)],
    selected: T,
    on: impl Fn(T) -> M,
) -> El<'a, M> {
    let items = options.iter().map(|(value, label)| {
        let active = *value == selected;
        button(
            text((*label).to_owned())
                .size(13)
                .font(weight(Weight::Semibold))
                .color(if active { p.text } else { p.muted }),
        )
        .padding(Padding::from([5, 14]))
        .style(theme::seg_item(*p, active))
        .on_press(on(*value))
        .into()
    });
    container(row(items).spacing(4))
        .padding(4)
        .style(theme::seg_track(*p))
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

/// Small solid badge (counts), in the monospace face.
pub(crate) fn badge<'a, M: 'a>(bg: Color, fg: Color, label: impl Into<String>) -> El<'a, M> {
    container(text(label.into()).size(11).font(mono_font()).color(fg))
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
            .font(mono_font())
            .color(p.muted),
    )
    .padding(Padding::from([2, 7]))
    .style(theme::field_box(*p, 6.0))
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
    .padding(Padding::from([14, 16]))
    .width(Length::Fill)
    .style(theme::tint(
        color,
        if p.is_light() { 0.16 } else { 0.15 },
        12.0,
    ))
    .into()
}

/// Discreet colored glyph (profiles, modes, providers): no tile behind it, the
/// color alone carries the meaning. Occupies a `size` square so layouts that
/// align on it keep their rhythm.
pub(crate) fn glyph_box<'a, M: 'a>(glyph: Glyph, color: Color, size: f32) -> El<'a, M> {
    container(icons::icon(glyph, (size * 0.5).max(16.0), color))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .center_x(Length::Fixed(size))
        .center_y(Length::Fixed(size))
        .into()
}
