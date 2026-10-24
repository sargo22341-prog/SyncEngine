//! Notification-area icon: left click shows the window, right click opens a
//! menu whose « Quitter » is the explicit way out when the close button only
//! minimizes. Windows uses the shell notification area; Linux uses the
//! StatusNotifierItem D-Bus protocol (KDE, XFCE, Cinnamon, GNOME with the
//! AppIndicator extension), without GTK. No tray is not an error: the window
//! keeps working and « Quitter » stays available in the settings (Ctrl+Q).

use iced::Subscription;
use iced::futures::channel::mpsc;
use iced::futures::stream::{self, BoxStream, StreamExt};
use std::sync::Mutex;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const SHOW: &str = "show";
const QUIT: &str = "quit";

/// What the user asked through the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayEvent {
    Show,
    Quit,
}

/// Receiving end handed once to the subscription. The OS callbacks are
/// process-wide and can be installed only once, hence a static inbox.
static INBOX: Mutex<Option<mpsc::UnboundedReceiver<TrayEvent>>> = Mutex::new(None);

/// Live icon; dropping it removes it from the notification area.
pub(crate) struct Tray {
    icon: TrayIcon,
    tooltip: String,
}

impl Tray {
    /// Creates the icon on the UI thread (Windows needs its message loop).
    pub(crate) fn create(rgba: Vec<u8>, size: u32) -> Result<Self, String> {
        let icon = tray_icon::Icon::from_rgba(rgba, size, size).map_err(|e| e.to_string())?;
        let menu = Menu::new();
        menu.append_items(&[
            &MenuItem::with_id(SHOW, "Ouvrir SyncEngine", true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(QUIT, "Quitter SyncEngine", true, None),
        ])
        .map_err(|e| e.to_string())?;
        let (sender, receiver) = mpsc::unbounded();
        let menu_sender = sender.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = match event.id().as_ref() {
                SHOW => TrayEvent::Show,
                QUIT => TrayEvent::Quit,
                _ => return,
            };
            // A closed inbox only means the window is already gone.
            let _window_gone = menu_sender.unbounded_send(action).is_err();
        }));
        // Only completed left clicks: hover/move events are not forwarded.
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let show = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if show {
                let _window_gone = sender.unbounded_send(TrayEvent::Show).is_err();
            }
        }));
        let tooltip = "SyncEngine".to_owned();
        let icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip(&tooltip)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;
        if let Ok(mut inbox) = INBOX.lock() {
            *inbox = Some(receiver);
        }
        Ok(Self { icon, tooltip })
    }

    /// Mirrors the window title (current job) in the icon tooltip.
    pub(crate) fn set_tooltip(&mut self, text: &str) {
        if self.tooltip != text && self.icon.set_tooltip(Some(text)).is_ok() {
            text.clone_into(&mut self.tooltip);
        }
    }
}

/// Icon events; stays empty when no icon could be created.
pub(crate) fn events() -> Subscription<TrayEvent> {
    Subscription::run(inbox)
}

fn inbox() -> BoxStream<'static, TrayEvent> {
    match INBOX.lock().ok().and_then(|mut inbox| inbox.take()) {
        Some(receiver) => receiver.boxed(),
        None => stream::pending().boxed(),
    }
}
