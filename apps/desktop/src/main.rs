//! SyncEngine desktop: native Rust interface built with iced.
//!
//! Presentation layer only. Profiles, plans, executions and schedules are
//! owned by the `sync-orchestrator` facade, called on worker threads; this
//! crate never compares trees, decides policies or mutates synchronized files.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod anim;
mod app;
mod backend;
mod charts;
mod draft;
mod icons;
mod model;
mod native;
mod overlays;
mod pages;
mod profile;
mod schedules;
mod shell;
mod speed;
mod theme;
mod titlebar;
mod tray;
mod ui;
mod ui_prefs;
mod update;
mod wizard;

use app::App;

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(|app: &App| app.palette().iced_theme())
        .default_font(ui::base_font())
        .antialiasing(true)
        .window(iced::window::Settings {
            size: iced::Size::new(1280.0, 820.0),
            min_size: Some(iced::Size::new(980.0, 640.0)),
            icon: charts::window_icon(),
            exit_on_close_request: false,
            // Title bar and border are drawn by the application (`titlebar`).
            decorations: false,
            ..iced::window::Settings::default()
        })
        .run()
}
