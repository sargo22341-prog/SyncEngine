//! SyncEngine desktop: native Rust interface built with iced.
//!
//! Presentation layer only. Profiles, plans, executions and schedules are
//! owned by the `sync-orchestrator` facade, called on worker threads; this
//! crate never compares trees, decides policies or mutates synchronized files.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod anim;
mod app;
mod app_update;
mod backend;
mod charts;
mod draft;
mod group_run;
mod icons;
mod interrupted;
mod model;
mod native;
mod overlays;
mod pages;
mod profile;
mod purge;
mod purge_view;
mod schedules;
mod shell;
mod speed;
mod theme;
mod titlebar;
mod trash;
mod tray;
mod tree;
mod ui;
mod ui_prefs;
mod update;
#[cfg(windows)]
mod windows_tasks;
mod wizard;

use app::App;

/// Window opened by the agent to follow a run starts as a compact tracker.
fn follows_agent() -> bool {
    std::env::args().skip(1).any(|a| a == app::FOLLOW_AGENT_ARG)
}

fn main() -> iced::Result {
    let mini = follows_agent();
    let mut application = iced::application(App::boot, App::update, App::view);
    for font in ui::FONT_FILES {
        application = application.font(font);
    }
    application
        .title(App::title)
        .subscription(App::subscription)
        .theme(|app: &App| app.palette().iced_theme())
        .default_font(ui::base_font())
        .antialiasing(true)
        .window(iced::window::Settings {
            size: if mini { app::MINI_SIZE } else { app::FULL_SIZE },
            min_size: Some(if mini {
                iced::Size::new(380.0, 460.0)
            } else {
                iced::Size::new(980.0, 640.0)
            }),
            icon: charts::window_icon(),
            exit_on_close_request: false,
            // Title bar and border are drawn by the application (`titlebar`).
            decorations: false,
            platform_specific: platform_window(),
            ..iced::window::Settings::default()
        })
        .run()
}

/// Windows 11 does not round undecorated windows by default: ask DWM for
/// the system rounded corners (ignored by Windows 10, and while maximized).
#[cfg(windows)]
fn platform_window() -> iced::window::settings::PlatformSpecific {
    iced::window::settings::PlatformSpecific {
        corner_preference: iced::window::settings::platform::CornerPreference::Round,
        ..Default::default()
    }
}

#[cfg(not(windows))]
fn platform_window() -> iced::window::settings::PlatformSpecific {
    iced::window::settings::PlatformSpecific::default()
}
