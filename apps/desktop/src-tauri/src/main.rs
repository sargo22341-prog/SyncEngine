//! Desktop IPC transport. All filesystem/SQLite work runs on blocking workers.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::future::Future;
use std::sync::Arc;
use sync_core::ProfileId;
use sync_orchestrator::{AppError, Application, JobSnapshot, Run};
use sync_profiles::{Preferences, Profile};
use tauri::{Manager, State};

type App = Arc<Result<Application, AppError>>;
fn service(app: &App) -> Result<&Application, AppError> {
    app.as_ref().as_ref().map_err(Clone::clone)
}
fn transport(message: impl std::fmt::Display) -> AppError {
    AppError {
        code: "transport".into(),
        message: message.to_string(),
        profile: None,
    }
}
async fn worker<T: Send + 'static>(
    app: App,
    job: impl FnOnce(&Application) -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(move || job(service(&app)?))
        .await
        .map_err(transport)?
}
#[tauri::command(async)]
fn profiles_list(
    app: State<'_, App>,
) -> impl Future<Output = Result<Vec<Profile>, AppError>> + Send + use<> {
    worker(app.inner().clone(), |a| a.profiles_list())
}
#[tauri::command(async)]
fn profile_save(
    app: State<'_, App>,
    profile: Profile,
) -> impl Future<Output = Result<(), AppError>> + Send + use<> {
    worker(app.inner().clone(), move |a| a.profile_save(profile))
}
#[tauri::command(async)]
fn profile_remove(
    app: State<'_, App>,
    id: ProfileId,
) -> impl Future<Output = Result<(), AppError>> + Send + use<> {
    worker(app.inner().clone(), move |a| a.profile_remove(id))
}
#[tauri::command(async)]
fn preview(
    app: State<'_, App>,
    id: ProfileId,
) -> impl Future<Output = Result<serde_json::Value, AppError>> + Send + use<> {
    worker(app.inner().clone(), move |a| a.preview(id))
}
#[tauri::command(async)]
fn execute(
    app: State<'_, App>,
    id: ProfileId,
    plan_id: String,
    confirm: bool,
    confirm_delete: bool,
) -> impl Future<Output = Result<serde_json::Value, AppError>> + Send + use<> {
    worker(app.inner().clone(), move |a| {
        a.execute(id, plan_id, confirm, confirm_delete)
    })
}
#[tauri::command]
fn cancel(app: State<'_, App>) -> Result<(), AppError> {
    service(app.inner())?.cancel()
}
#[tauri::command]
fn snapshot(app: State<'_, App>) -> Result<JobSnapshot, AppError> {
    service(app.inner())?.snapshot()
}
#[tauri::command(async)]
fn history(
    app: State<'_, App>,
    id: ProfileId,
) -> impl Future<Output = Result<Vec<Run>, AppError>> + Send + use<> {
    worker(app.inner().clone(), move |a| a.history(id))
}
#[tauri::command(async)]
fn preferences(
    app: State<'_, App>,
) -> impl Future<Output = Result<Preferences, AppError>> + Send + use<> {
    worker(app.inner().clone(), |a| a.preferences())
}
#[tauri::command(async)]
fn preferences_save(
    app: State<'_, App>,
    preferences: Preferences,
) -> impl Future<Output = Result<(), AppError>> + Send + use<> {
    worker(app.inner().clone(), move |a| {
        a.preferences_save(preferences)
    })
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tauri::Builder::default()
        .setup(|app| {
            // Initialization failures are transported to the visible UI instead
            // of silently terminating a release binary with a hidden console.
            let state = app
                .path()
                .app_data_dir()
                .map_err(transport)
                .and_then(|path| {
                    std::fs::create_dir_all(&path).map_err(transport)?;
                    Application::open(path)
                });
            app.manage(Arc::new(state));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.state::<App>();
                let Ok(app) = service(app.inner()) else {
                    return;
                };
                // Keep the UI alive until cooperative cancellation and durable
                // completion finish. Never silently abandon a running worker.
                match app.snapshot() {
                    Ok(snapshot) if snapshot.active => {
                        api.prevent_close();
                        if let Err(error) = app.cancel() {
                            #[expect(
                                clippy::print_stderr,
                                reason = "last-resort cancellation diagnostic; window stays open"
                            )]
                            {
                                eprintln!("{error}");
                            }
                        }
                    }
                    Err(_) => api.prevent_close(),
                    _ => (),
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            profiles_list,
            profile_save,
            profile_remove,
            preview,
            execute,
            cancel,
            snapshot,
            history,
            preferences,
            preferences_save
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}
