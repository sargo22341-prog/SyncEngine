//! Integrated window frame: the native title bar and border are disabled
//! (`decorations: false`) and replaced by a bar drawn in the application's
//! own colors, as Firefox or the Claude desktop app do, plus invisible resize
//! grips along the edges.
//!
//! Window management stays with the system: dragging the bar starts the
//! native move loop (Aero Snap, multi-monitor), the grips start the native
//! resize loop, right click opens the system menu and Alt+F4 still closes.
//! Closing goes through the same path as the system close request, so the
//! « minimize instead of quit » setting is honored.

use crate::app::{App, Message};
use crate::charts::{self, Mark};
use crate::icons::{self, Glyph};
use crate::theme::{self, Palette};
use crate::ui::{self, El};
use iced::widget::{Space, button, column, container, mouse_area, row, stack};
use iced::window::Direction;
use iced::{Alignment, Color, Length, Padding, Point, Task, Theme, mouse, window};

/// Height of the bar, in logical pixels. Content coordinates (cursor, window
/// size used by overlays) start below it.
pub(crate) const HEIGHT: f32 = 36.0;
/// Width of one caption button (Windows 11 metrics).
const BUTTON_WIDTH: f32 = 46.0;
/// Thickness of the resize grips along the edges.
const GRIP: f32 = 5.0;
/// Length of the corner grips along each edge.
const CORNER: f32 = 12.0;
/// Pointer travel before a press on the bar becomes a window move, so a
/// double click (maximize) is not swallowed by the system move loop.
const DRAG_THRESHOLD: f32 = 3.0;
/// System close-button red (Windows 11 caption guidelines).
const CLOSE_RED: Color = Color::from_rgb(0.769, 0.169, 0.110);

/// Window-frame interactions.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Chrome {
    /// Left press on the bar; becomes a move once the pointer travels.
    Press,
    Minimize,
    ToggleMaximize,
    Close,
    SystemMenu,
    Resize(Direction),
    Maximized(bool),
    Focused(bool),
    /// Pointer over the close button (its glyph turns white on red).
    CloseHover(bool),
}

/// Frame state owned by the shell.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame {
    pub(crate) maximized: bool,
    pub(crate) focused: bool,
    pub(crate) close_hover: bool,
    /// Content-coordinate pointer position of a pending press on the bar.
    pub(crate) press: Option<Point>,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            maximized: false,
            focused: true,
            close_hover: false,
            press: None,
        }
    }
}

impl App {
    pub(crate) fn chrome(&mut self, action: Chrome) -> Task<Message> {
        let Some(id) = self.window_id else {
            return Task::none();
        };
        match action {
            Chrome::Press => {
                self.frame.press = Some(self.cursor);
                Task::none()
            }
            Chrome::Minimize => self.minimize_window(id),
            Chrome::ToggleMaximize => {
                self.frame.press = None;
                window::toggle_maximize(id)
            }
            // Same path as the system close request (Alt+F4, taskbar).
            Chrome::Close => self.update(Message::CloseRequested(id)),
            Chrome::SystemMenu => window::show_system_menu(id),
            Chrome::Resize(direction) => window::drag_resize(id, direction),
            Chrome::Maximized(maximized) => {
                self.frame.maximized = maximized;
                Task::none()
            }
            Chrome::Focused(focused) => {
                self.frame.focused = focused;
                Task::none()
            }
            Chrome::CloseHover(hover) => {
                self.frame.close_hover = hover;
                Task::none()
            }
        }
    }

    /// Starts the native move loop once a press on the bar has travelled.
    pub(crate) fn drag_window(&mut self, at: Point) -> Task<Message> {
        match (self.frame.press, self.window_id) {
            (Some(origin), Some(id))
                if (at.x - origin.x).hypot(at.y - origin.y) > DRAG_THRESHOLD =>
            {
                self.frame.press = None;
                window::drag(id)
            }
            _ => Task::none(),
        }
    }

    /// Re-reads the maximized state after a size change (snap, double click,
    /// Win+Up…): the system may change it without going through the bar.
    pub(crate) fn sync_maximized(&self) -> Task<Message> {
        match self.window_id {
            Some(id) => window::is_maximized(id).map(|m| Message::Chrome(Chrome::Maximized(m))),
            None => Task::none(),
        }
    }

    /// Whole window: title bar above `body`, resize grips on top unless
    /// maximized.
    pub(crate) fn framed<'a>(&'a self, p: &Palette, body: El<'a, Message>) -> El<'a, Message> {
        let window = column![self.title_bar(p), body]
            .width(Length::Fill)
            .height(Length::Fill);
        if self.frame.maximized {
            return window.into();
        }
        stack![window, grips()]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn title_bar(&self, p: &Palette) -> El<'_, Message> {
        let focused = self.frame.focused;
        let fg = if focused { p.text } else { p.faint };
        let mark = charts::mark(
            Mark {
                from: p.accent,
                to: theme::mix(p.accent, p.info, 0.6),
                turn: 0.0,
            },
            16.0,
        );
        let caption = mouse_area(
            container(
                row![mark, ui::colored(fg, self.title(), 12.0)]
                    .spacing(10)
                    .align_y(Alignment::Center),
            )
            .padding(Padding {
                left: 14.0,
                ..Padding::ZERO
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .center_y(Length::Fill),
        )
        .on_press(Message::Chrome(Chrome::Press))
        .on_double_click(Message::Chrome(Chrome::ToggleMaximize))
        .on_right_press(Message::Chrome(Chrome::SystemMenu));
        let maximize = if self.frame.maximized {
            (Glyph::WinRestore, "Restaurer")
        } else {
            (Glyph::WinMaximize, "Agrandir")
        };
        container(
            row![
                caption,
                control(
                    p,
                    fg,
                    Glyph::WinMinimize,
                    "Réduire",
                    Chrome::Minimize,
                    false
                ),
                control(p, fg, maximize.0, maximize.1, Chrome::ToggleMaximize, false),
                mouse_area(control(
                    p,
                    if self.frame.close_hover {
                        Color::WHITE
                    } else {
                        fg
                    },
                    Glyph::WinClose,
                    "Fermer",
                    Chrome::Close,
                    true
                ))
                .on_enter(Message::Chrome(Chrome::CloseHover(true)))
                .on_exit(Message::Chrome(Chrome::CloseHover(false))),
            ]
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(HEIGHT))
        .style(theme::panel(p.rail))
        .into()
    }
}

/// One caption button. The label is the accessible tooltip text.
fn control<'a>(
    p: &Palette,
    fg: Color,
    glyph: Glyph,
    label: &'static str,
    action: Chrome,
    close: bool,
) -> El<'a, Message> {
    let p = *p;
    let hover_bg = if close {
        CLOSE_RED
    } else {
        theme::alpha(p.text, 0.08)
    };
    let pressed_bg = if close {
        theme::alpha(CLOSE_RED, 0.8)
    } else {
        theme::alpha(p.text, 0.05)
    };
    ui::with_tip(
        &p,
        button(container(icons::thin(glyph, 24.0, fg)).center(Length::Fill))
            .padding(0)
            .width(Length::Fixed(BUTTON_WIDTH))
            .height(Length::Fill)
            .style(move |_: &Theme, status| button::Style {
                background: match status {
                    button::Status::Hovered => Some(hover_bg.into()),
                    button::Status::Pressed => Some(pressed_bg.into()),
                    _ => None,
                },
                ..button::Style::default()
            })
            .on_press(Message::Chrome(action)),
        label,
        iced::widget::tooltip::Position::Bottom,
    )
}

/// Invisible edges starting the native resize loop. Only the grips react to
/// the pointer; the rest of the layer lets events through to the window.
fn grips<'a>() -> El<'a, Message> {
    let grip = |direction: Direction, width: Length, height: Length| -> El<'a, Message> {
        let interaction = match direction {
            Direction::North | Direction::South => mouse::Interaction::ResizingVertically,
            Direction::East | Direction::West => mouse::Interaction::ResizingHorizontally,
            Direction::NorthWest | Direction::SouthEast => {
                mouse::Interaction::ResizingDiagonallyDown
            }
            Direction::NorthEast | Direction::SouthWest => mouse::Interaction::ResizingDiagonallyUp,
        };
        mouse_area(Space::new().width(width).height(height))
            .interaction(interaction)
            .on_press(Message::Chrome(Chrome::Resize(direction)))
            .into()
    };
    let (edge, corner) = (Length::Fixed(GRIP), Length::Fixed(CORNER));
    column![
        row![
            grip(Direction::NorthWest, corner, edge),
            grip(Direction::North, Length::Fill, edge),
            grip(Direction::NorthEast, corner, edge),
        ],
        row![
            column![
                grip(Direction::NorthWest, edge, corner),
                grip(Direction::West, edge, Length::Fill),
                grip(Direction::SouthWest, edge, corner),
            ],
            Space::new().width(Length::Fill).height(Length::Fill),
            column![
                grip(Direction::NorthEast, edge, corner),
                grip(Direction::East, edge, Length::Fill),
                grip(Direction::SouthEast, edge, corner),
            ],
        ]
        .height(Length::Fill),
        row![
            grip(Direction::SouthWest, corner, edge),
            grip(Direction::South, Length::Fill, edge),
            grip(Direction::SouthEast, corner, edge),
        ],
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
