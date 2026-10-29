//! Application updates: Réglages › « Mises à jour » and the « Nouvelle mise à
//! jour disponible » dialog shown at start-up.
//!
//! Presentation only (not to be confused with `update.rs`, the iced message
//! loop). Lookup, download, checksum, extraction and replacement live in the
//! `app-update` crate, on worker threads. This module checks the window's own
//! preconditions, stops the resident agent around the replacement and starts
//! it again, then starts the new interface and leaves. Nothing is installed
//! without the user's explicit « Mettre à jour maintenant ».

use crate::app::{App, Dialog, Message};
use crate::backend::{self, Failure};
use crate::icons::{self, Glyph};
use crate::model;
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use app_update::{Check, Installed, Progress, Staged, Target, Update, UpdateError, UpdateSource};
use iced::widget::{Space, column, container, row, scrollable};
use iced::{Alignment, Length, Padding, Task};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Longest wait for the resident agent to stop before replacing the files.
const AGENT_STOP_WAIT: Duration = Duration::from_secs(20);
/// Notes beyond these bounds are folded behind « Afficher plus ».
const FOLDED_LINES: usize = 8;
const FOLDED_CHARS: usize = 600;

/// Last update lookup.
#[derive(Debug, Clone, Default)]
pub(crate) enum CheckState {
    #[default]
    Idle,
    Checking,
    UpToDate(String),
    /// Newer version published without an archive for this target.
    NoAsset(String),
    Available(Box<Update>),
    Failed(Failure),
    /// This build's target has no published archive.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Download, checksum and extraction (cancellable).
    Downloading,
    /// Agent stop, replacement, agent restart (not cancellable).
    Installing,
    /// New interface being started.
    Restarting,
}

#[derive(Debug)]
pub(crate) struct Install {
    pub(crate) phase: Phase,
    pub(crate) progress: Arc<Progress>,
}

/// Update state of the window, kept for the whole session (nothing persisted).
#[derive(Debug, Default)]
pub(crate) struct Updates {
    pub(crate) check: CheckState,
    pub(crate) notes_open: bool,
    pub(crate) install: Option<Install>,
    /// Failure of the last installation attempt.
    pub(crate) error: Option<Failure>,
    /// Start-up check found an update while the window could not show it.
    pub(crate) popup_pending: bool,
    pub(crate) cleaned: bool,
    /// Leftovers of a previous update that could not be removed.
    pub(crate) cleanup_failed: Vec<String>,
}

/// Single-use value carried by a cloneable message.
pub(crate) struct Handoff<T>(Arc<Mutex<Option<T>>>);

// Manual: a derive would require `T: Clone`, only the handle is cloned.
impl<T> Clone for Handoff<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Handoff<T> {
    fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }
    fn take(&self) -> Option<T> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

impl<T> std::fmt::Debug for Handoff<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Handoff")
    }
}

#[derive(Debug, Clone)]
pub(crate) enum UpdateMsg {
    /// « Vérifier les mises à jour ».
    Check,
    Checked {
        manual: bool,
        result: Result<Check, Failure>,
    },
    Cleaned(Vec<String>),
    ToggleNotes,
    /// « Plus tard »: closes the dialog; it comes back at the next opening.
    Later,
    /// « Mettre à jour maintenant ».
    Install,
    Staged(Result<Handoff<Staged>, Failure>),
    Installed(Result<Installed, Failure>),
    Cancel,
    /// Redraws the download progress.
    Tick,
    OpenPage,
}

fn wrap(msg: UpdateMsg) -> Message {
    Message::AppUpdate(msg)
}

fn failure(error: &UpdateError) -> Failure {
    Failure::new(error.code(), error.message())
}

/// Plain-language headline of an update error code.
pub(crate) fn headline(code: &str) -> &'static str {
    match code {
        "offline" => "Hors ligne : vérification impossible",
        "rate_limited" => "Limite de requêtes GitHub atteinte",
        "no_release" => "Aucune version publiée",
        "http" => "Échange avec GitHub impossible",
        "invalid_release" => "Réponse de GitHub inutilisable",
        "no_asset" => "Mise à jour non disponible pour cette plateforme",
        "checksum_missing" => "Empreinte SHA-256 absente : installation refusée",
        "checksum_mismatch" => "Empreinte SHA-256 différente : archive refusée",
        "invalid_archive" => "Archive refusée",
        "cancelled" => "Mise à jour annulée",
        "busy" => "Mise à jour impossible pour l’instant",
        "not_writable" => "Dossier du programme non modifiable",
        "replace_failed" => "Remplacement impossible : version actuelle restaurée",
        "rollback_incomplete" => "Restauration incomplète : vérifiez le dossier du programme",
        "unsupported" => "Mise à jour automatique non disponible",
        "restart" => "Mise à jour installée : relancez SyncEngine",
        _ => "Mise à jour impossible",
    }
}

/// Errors after which a manual download is the way forward.
fn manual_download(code: &str) -> bool {
    matches!(
        code,
        "not_writable" | "unsupported" | "rollback_incomplete" | "checksum_missing"
    )
}

/// Looks up the latest release on a worker thread.
pub(crate) fn lookup(manual: bool) -> Task<Message> {
    let Some(target) = Target::current() else {
        return Task::none();
    };
    Task::perform(
        backend::blocking(move || {
            let source = UpdateSource::github().map_err(|e| failure(&e))?;
            app_update::check(&source, env!("CARGO_PKG_VERSION"), target).map_err(|e| failure(&e))
        }),
        move |result| wrap(UpdateMsg::Checked { manual, result }),
    )
}

/// Program folder of this interface.
fn program_dir(target: Target) -> Result<PathBuf, Failure> {
    let exe = std::env::current_exe().map_err(|e| failure(&app_update::io_error(e)))?;
    app_update::program_dir(&exe, target).map_err(|e| failure(&e))
}

/// Download, checksum and strict extraction, in the program folder.
fn prepare(update: Update, progress: Arc<Progress>) -> Result<Staged, Failure> {
    let target = Target::current().ok_or_else(|| failure(&app_update::unsupported_platform()))?;
    let dir = program_dir(target)?;
    let source = UpdateSource::github().map_err(|e| failure(&e))?;
    let downloaded =
        app_update::download(&source, &update, &dir, &progress).map_err(|e| failure(&e))?;
    app_update::stage(downloaded, &progress).map_err(|e| failure(&e))
}

fn discard(staged: Staged, error: Failure) -> Failure {
    match staged.discard() {
        Ok(()) => error,
        Err(e) => Failure::new(
            &error.code,
            format!(
                "{} ; fichiers extraits non retirés : {}",
                error.message,
                e.message()
            ),
        ),
    }
}

/// Stops the resident agent, replaces the files, restarts the agent.
fn finish(staged: Staged, state: &Path) -> Result<Installed, Failure> {
    let mut slot = Some(staged);
    let result = around_agent(state, AGENT_STOP_WAIT, crate::native::prepare_agent, || {
        let staged = slot
            .take()
            .ok_or_else(|| Failure::new("io", "fichiers extraits indisponibles"))?;
        app_update::install(staged).map_err(|e| failure(&e))
    });
    // Refused before replacing: the extracted files are removed.
    match (slot.take(), result) {
        (Some(staged), Err(error)) => Err(discard(staged, error)),
        (_, result) => result,
    }
}

fn joined(error: Failure, notes: Vec<String>) -> Failure {
    if notes.is_empty() {
        return error;
    }
    let message = [error.message].into_iter().chain(notes).collect::<Vec<_>>();
    Failure::new(&error.code, message.join(" ; "))
}

/// Runs `work` with the resident agent stopped, then restarts it.
///
/// Refused while the agent dispatches a profile. A stop request is only left
/// (then lifted, and `restart` asked) when the agent was running and no
/// explicit stop was pending: an agent stopped by the user stays stopped.
/// A scheduled run that starts between the check and the stop request is
/// cancelled cooperatively, as by `syncengine-agent stop`.
fn around_agent(
    state: &Path,
    wait: Duration,
    restart: impl FnOnce(&Path) -> Result<(), Failure>,
    work: impl FnOnce() -> Result<Installed, Failure>,
) -> Result<Installed, Failure> {
    let status = sync_orchestrator::resident_status(state).map_err(Failure::from)?;
    if status.available && status.phase == sync_orchestrator::ResidentPhase::Dispatching {
        return Err(Failure::new(
            "busy",
            "Une synchronisation planifiée est en cours, réessayez après.",
        ));
    }
    let we_stopped = status.available && !sync_orchestrator::agent_stop_pending(state);
    if we_stopped {
        sync_orchestrator::request_agent_stop(state)
            .map_err(|e| Failure::new("unavailable", e.to_string()))?;
    }
    let mut notes = Vec::new();
    let result = match status.available {
        true => wait_agent_exit(state, wait),
        false => Ok(()),
    }
    .and_then(|()| work());
    if we_stopped {
        match sync_orchestrator::clear_agent_stop(state) {
            Err(e) => notes.push(format!(
                "demande d’arrêt de l’agent non levée (Planification › Reprendre les planifications) : {e}"
            )),
            Ok(()) => {
                if let Err(e) = restart(state) {
                    notes.push(format!("relance de l’agent : {}", e.message));
                }
            }
        }
    }
    match result {
        Ok(mut installed) => {
            installed.warnings.extend(notes);
            Ok(installed)
        }
        Err(error) => Err(joined(error, notes)),
    }
}

/// Waits for the agent's presence lock to be released.
fn wait_agent_exit(state: &Path, wait: Duration) -> Result<(), Failure> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if !sync_orchestrator::resident_status(state)
            .map_err(Failure::from)?
            .available
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(Failure::new(
                "busy",
                "l’agent ne s’est pas arrêté : rien n’a été remplacé, réessayez",
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Removes what a previous update left in the program folder.
fn cleanup() -> Vec<String> {
    let Some(target) = Target::current() else {
        return Vec::new();
    };
    match program_dir(target) {
        Ok(dir) => app_update::cleanup(&dir, target)
            .failed
            .into_iter()
            .map(|(path, cause)| format!("{} : {cause}", path.display()))
            .collect(),
        // A renamed executable is reported when updating, not at each start.
        Err(_) => Vec::new(),
    }
}

/// `2026-10-20T08:30:00Z` → `20/10/2026` (UTC date as published).
fn published(at: Option<&str>) -> Option<String> {
    let date = at?.get(..10)?;
    let mut parts = date.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    let digits = |v: &str, n: usize| v.len() == n && v.bytes().all(|b| b.is_ascii_digit());
    (digits(y, 4) && digits(m, 2) && digits(d, 2)).then(|| format!("{d}/{m}/{y}"))
}

fn long_notes(notes: &str) -> bool {
    notes.lines().count() > FOLDED_LINES || notes.chars().count() > FOLDED_CHARS
}

impl App {
    /// Why installing cannot start right now, if so.
    fn update_blocker(&self) -> Option<&'static str> {
        if self.backend.is_none() || self.purging() {
            Some("L’état local n’est pas ouvert.")
        } else if self.busy() {
            Some("Une synchronisation est en cours dans cette fenêtre : attendez sa fin.")
        } else if self.agent_job.is_some() {
            Some("Une synchronisation planifiée est en cours, réessayez après.")
        } else {
            None
        }
    }

    pub(crate) fn update_installing(&self) -> bool {
        self.updates.install.is_some()
    }

    /// The window owns the state: removes leftovers of a previous update and
    /// shows the dialog of a start-up check that arrived before.
    pub(crate) fn update_after_boot(&mut self) -> Task<Message> {
        self.show_update_popup();
        if self.updates.cleaned {
            return Task::none();
        }
        self.updates.cleaned = true;
        Task::perform(backend::blocking(|| Ok(cleanup())), |r| {
            wrap(UpdateMsg::Cleaned(r.unwrap_or_default()))
        })
    }

    fn show_update_popup(&mut self) {
        if !self.updates.popup_pending || self.backend.is_none() {
            return;
        }
        if !matches!(self.updates.check, CheckState::Available(_)) {
            self.updates.popup_pending = false;
            return;
        }
        if self.dialog.is_none() {
            self.updates.popup_pending = false;
            self.open_dialog(Dialog::Update);
        }
    }

    /// Shows a dialog that was waiting for another one to close.
    pub(crate) fn update_dialog_closed(&mut self) {
        self.show_update_popup();
    }

    pub(crate) fn app_update(&mut self, msg: UpdateMsg) -> Task<Message> {
        match msg {
            UpdateMsg::Check => {
                if matches!(self.updates.check, CheckState::Checking) || self.update_installing() {
                    return Task::none();
                }
                if Target::current().is_none() {
                    self.updates.check = CheckState::Unsupported;
                    return Task::none();
                }
                self.updates.check = CheckState::Checking;
                lookup(true)
            }
            UpdateMsg::Checked { manual, result } => {
                if self.update_installing() {
                    return Task::none();
                }
                self.updates.check = match result {
                    Ok(Check::UpToDate { latest }) => CheckState::UpToDate(latest),
                    Ok(Check::NoAsset { latest }) => CheckState::NoAsset(latest),
                    Ok(Check::Available(update)) => {
                        self.updates.notes_open = false;
                        CheckState::Available(update)
                    }
                    // Silent at start-up: only Réglages shows the failure.
                    Err(e) => CheckState::Failed(e),
                };
                if !manual {
                    self.updates.popup_pending = true;
                    self.show_update_popup();
                }
                Task::none()
            }
            UpdateMsg::Cleaned(failed) => {
                self.updates.cleanup_failed = failed;
                Task::none()
            }
            UpdateMsg::ToggleNotes => {
                self.updates.notes_open = !self.updates.notes_open;
                Task::none()
            }
            UpdateMsg::Later => {
                self.close_dialog();
                Task::none()
            }
            UpdateMsg::Install => self.start_install(),
            UpdateMsg::Staged(result) => self.staged(result),
            UpdateMsg::Installed(result) => self.installed(result),
            UpdateMsg::Cancel => {
                if let Some(install) = &self.updates.install
                    && install.phase == Phase::Downloading
                {
                    install.progress.cancel();
                }
                Task::none()
            }
            UpdateMsg::Tick => Task::none(),
            UpdateMsg::OpenPage => {
                let url = match &self.updates.check {
                    CheckState::Available(update) => update.page_url.clone(),
                    _ => "https://github.com/sargo22341-prog/SyncEngine/releases/latest".to_owned(),
                };
                if let Err(e) = crate::native::open_url(&url) {
                    self.fail(&e);
                }
                Task::none()
            }
        }
    }

    fn start_install(&mut self) -> Task<Message> {
        if self.update_installing() {
            return Task::none();
        }
        let CheckState::Available(update) = &self.updates.check else {
            return Task::none();
        };
        let update = (**update).clone();
        if !matches!(self.dialog, Some(Dialog::Update)) {
            self.open_dialog(Dialog::Update);
        }
        if let Some(reason) = self.update_blocker() {
            self.updates.error = Some(Failure::new("busy", reason));
            return Task::none();
        }
        self.updates.error = None;
        let progress = Arc::new(Progress::default());
        self.updates.install = Some(Install {
            phase: Phase::Downloading,
            progress: Arc::clone(&progress),
        });
        Task::perform(
            backend::blocking(move || prepare(update, progress).map(Handoff::new)),
            |r| wrap(UpdateMsg::Staged(r)),
        )
    }

    fn update_failed(&mut self, error: Failure) {
        self.updates.install = None;
        self.updates.error = Some(error);
        if !matches!(self.dialog, Some(Dialog::Update)) {
            self.open_dialog(Dialog::Update);
        }
    }

    fn staged(&mut self, result: Result<Handoff<Staged>, Failure>) -> Task<Message> {
        let staged = match result.and_then(|h| {
            h.take()
                .ok_or_else(|| Failure::new("io", "fichiers extraits indisponibles"))
        }) {
            Ok(staged) => staged,
            Err(e) => {
                self.update_failed(e);
                return Task::none();
            }
        };
        // Re-checked just before acting: a run may have started meanwhile.
        let state = self
            .backend
            .as_ref()
            .map(|a| a.state_directory().to_path_buf());
        let (Some(state), None) = (state, self.update_blocker()) else {
            let reason = self
                .update_blocker()
                .unwrap_or("L’état local n’est pas ouvert.");
            let error = discard(staged, Failure::new("busy", reason));
            self.update_failed(error);
            return Task::none();
        };
        if let Some(install) = &mut self.updates.install {
            install.phase = Phase::Installing;
        }
        Task::perform(backend::blocking(move || finish(staged, &state)), |r| {
            wrap(UpdateMsg::Installed(r))
        })
    }

    fn installed(&mut self, result: Result<Installed, Failure>) -> Task<Message> {
        let installed = match result {
            Ok(installed) => installed,
            Err(e) => {
                self.update_failed(e);
                return Task::none();
            }
        };
        if let Some(install) = &mut self.updates.install {
            install.phase = Phase::Restarting;
        }
        match std::process::Command::new(&installed.desktop).spawn() {
            Ok(_child) => {
                // The new window waits for this one's state lock, released at exit.
                self.presence = None;
                iced::exit()
            }
            Err(e) => {
                let mut message = format!(
                    "SyncEngine {} est installé mais n’a pas pu être relancé ({e}) : fermez puis rouvrez SyncEngine.",
                    installed.version
                );
                for warning in &installed.warnings {
                    message.push_str(" ; ");
                    message.push_str(warning);
                }
                self.updates.check = CheckState::Idle;
                self.update_failed(Failure::new("restart", message));
                Task::none()
            }
        }
    }

    fn notes_view<'a>(&self, p: &Palette, update: &'a Update) -> El<'a, Message> {
        if update.notes.is_empty() {
            return ui::caption(p, "Aucune note de version.");
        }
        let long = long_notes(&update.notes);
        let folded = long && !self.updates.notes_open;
        let mut list = column![].spacing(4);
        let mut chars = 0;
        for line in update
            .notes
            .lines()
            .take(if folded { FOLDED_LINES } else { usize::MAX })
        {
            let shown = match line.trim_start().strip_prefix("- ") {
                Some(rest) => format!("• {rest}"),
                None => line.to_owned(),
            };
            chars += shown.chars().count();
            if folded && chars > FOLDED_CHARS {
                list = list.push(ui::colored(p.text, model::ellipsize(&shown, 80), 13.0));
                break;
            }
            list = list.push(ui::colored(p.text, shown, 13.0));
        }
        let mut col = column![
            container(scrollable(list).style(theme::scroll(*p)))
                .padding(Padding::from([10, 12]))
                .max_height(if folded { 220.0 } else { 320.0 })
                .width(Length::Fill)
                .style(theme::outline(*p, p.border, 0.0)),
        ]
        .spacing(6);
        if long {
            col = col.push(ui::action(
                p,
                Tone::Ghost,
                Some(if folded {
                    Glyph::ChevronDown
                } else {
                    Glyph::ChevronUp
                }),
                if folded {
                    "Afficher plus"
                } else {
                    "Afficher moins"
                },
                Some(wrap(UpdateMsg::ToggleNotes)),
            ));
        }
        col.into()
    }

    fn progress_view<'a>(&self, p: &Palette, install: &Install) -> El<'a, Message> {
        let (label, detail): (&str, String) = match install.phase {
            Phase::Downloading => {
                let (done, total) = (install.progress.done(), install.progress.total());
                (
                    "Téléchargement et vérification…",
                    format!("{} / {}", model::bytes(done), model::bytes(total)),
                )
            }
            Phase::Installing => (
                "Installation…",
                "Arrêt de l’agent, remplacement des fichiers, relance de l’agent.".to_owned(),
            ),
            Phase::Restarting => ("Redémarrage de SyncEngine…", String::new()),
        };
        let fraction = match install.phase {
            Phase::Downloading => {
                model::fraction(install.progress.done(), install.progress.total())
            }
            _ => Some(1.0),
        }
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
        let filled = (fraction * 1000.0).round() as u16;
        let bar = container(
            row![
                container(Space::new())
                    .width(Length::FillPortion(filled.max(1)))
                    .height(Length::Fixed(8.0))
                    .style(theme::bar(p.accent)),
                Space::new().width(Length::FillPortion(1000_u16.saturating_sub(filled).max(1))),
            ]
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .style(theme::bar(theme::alpha(p.accent, 0.18)));
        let cancel = (install.phase == Phase::Downloading && !install.progress.cancelled())
            .then(|| wrap(UpdateMsg::Cancel));
        column![
            row![
                ui::strong(p.text, label, 14.0),
                ui::fill(),
                ui::caption(p, detail),
            ]
            .align_y(Alignment::Center),
            bar,
            row![
                ui::caption(
                    p,
                    "Ne fermez pas SyncEngine : il se relancera tout seul une fois la mise à jour installée."
                ),
                ui::fill(),
                ui::action(p, Tone::Ghost, Some(Glyph::Stop), "Annuler", cancel),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(10)
        .into()
    }

    fn error_view<'a>(&self, p: &Palette, error: &Failure) -> El<'a, Message> {
        let mut col = column![ui::callout(
            p,
            if error.code == "cancelled" {
                p.warning
            } else {
                p.danger
            },
            Glyph::Alert,
            headline(&error.code),
            error.message.clone(),
        )]
        .spacing(8);
        if manual_download(&error.code) {
            col = col.push(ui::action(
                p,
                Tone::Secondary,
                Some(Glyph::Globe),
                "Télécharger manuellement (page de la release)",
                Some(wrap(UpdateMsg::OpenPage)),
            ));
        }
        col.into()
    }

    /// Body of an available update: versions, date, notes, actions.
    fn available_view<'a>(&self, p: &Palette, update: &'a Update, dialog: bool) -> El<'a, Message> {
        let mut facts = format!(
            "Version actuelle : {} · nouvelle version : {}",
            env!("CARGO_PKG_VERSION"),
            update.version
        );
        if let Some(date) = published(update.published_at.as_deref()) {
            facts.push_str(&format!(" · publiée le {date}"));
        }
        let mut col =
            column![ui::strong(p.text, facts, 14.0), self.notes_view(p, update),].spacing(12);
        if let Some(problem) = update.checksum_problem() {
            col = col.push(self.error_view(p, &failure(problem)));
        }
        if let Some(error) = &self.updates.error {
            col = col.push(self.error_view(p, error));
        }
        if let Some(install) = &self.updates.install {
            return col.push(self.progress_view(p, install)).into();
        }
        if let Some(reason) = self.update_blocker() {
            col = col.push(ui::callout(
                p,
                p.warning,
                Glyph::Info,
                "Mise à jour possible plus tard",
                reason,
            ));
        }
        let ready = self.update_blocker().is_none() && update.checksum_problem().is_none();
        let mut actions = row![].spacing(10).align_y(Alignment::Center);
        if dialog {
            actions = actions.push(ui::caption(
                p,
                "Rien n’est installé sans votre accord. Profils et historique sont conservés.",
            ));
            actions = actions.push(ui::fill());
            actions = actions.push(ui::action(
                p,
                Tone::Ghost,
                None,
                "Plus tard",
                Some(wrap(UpdateMsg::Later)),
            ));
        } else {
            actions = actions.push(ui::action(
                p,
                Tone::Ghost,
                Some(Glyph::Globe),
                "Page de la release",
                Some(wrap(UpdateMsg::OpenPage)),
            ));
            actions = actions.push(ui::fill());
        }
        actions = actions.push(ui::action(
            p,
            Tone::Primary,
            Some(Glyph::Refresh),
            "Mettre à jour maintenant",
            ready.then(|| wrap(UpdateMsg::Install)),
        ));
        col.push(actions).into()
    }

    /// « Nouvelle mise à jour disponible » dialog.
    pub(crate) fn update_dialog<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let running = self.update_installing();
        let head = row![
            ui::glyph_box(Glyph::Refresh, p.accent, 40.0),
            ui::title(p, "Nouvelle mise à jour disponible", 19.0),
            ui::fill(),
            iced::widget::button(icons::icon(Glyph::Close, 14.0, p.faint))
                .padding(6)
                .style(theme::btn(*p, Tone::Ghost))
                .on_press_maybe((!running).then_some(Message::CloseDialog)),
        ]
        .spacing(14)
        .align_y(Alignment::Center);
        let body: El<'a, Message> = match &self.updates.check {
            CheckState::Available(update) => self.available_view(p, update, true),
            _ => match &self.updates.error {
                Some(error) => self.error_view(p, error),
                None => ui::muted(p, "Aucune mise à jour à installer."),
            },
        };
        column![head, body].spacing(16).into()
    }

    /// Réglages › « Mises à jour ».
    pub(crate) fn updates_section<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let target = Target::current().map_or("cible sans archive publiée", Target::triple);
        let checking = matches!(self.updates.check, CheckState::Checking);
        let header = row![
            column![
                ui::strong(
                    p.text,
                    format!("Version installée : {}", env!("CARGO_PKG_VERSION")),
                    14.0
                ),
                ui::caption(p, format!("Cible compilée : {target}")),
            ]
            .spacing(2)
            .width(Length::Fill),
            ui::action(
                p,
                Tone::Secondary,
                Some(Glyph::Refresh),
                if checking {
                    "Vérification…"
                } else {
                    "Vérifier les mises à jour"
                },
                (!checking && !self.update_installing() && Target::current().is_some())
                    .then(|| wrap(UpdateMsg::Check)),
            ),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let line = |glyph: Glyph, color: iced::Color, text: String| -> El<'a, Message> {
            row![
                icons::icon(glyph, 15.0, color),
                ui::colored(p.text, text, 13.0)
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        };
        let status: El<'a, Message> = match &self.updates.check {
            CheckState::Idle => ui::caption(
                p,
                "Vérification automatique à chaque ouverture, sans compte ni jeton. Rien n’est installé sans votre accord.",
            ),
            CheckState::Checking => ui::caption(p, "Interrogation de GitHub…"),
            CheckState::UpToDate(latest) => line(
                Glyph::Check,
                p.success,
                format!("SyncEngine est à jour (dernière version publiée : {latest})."),
            ),
            CheckState::NoAsset(latest) => line(
                Glyph::Info,
                p.warning,
                format!(
                    "La version {latest} est publiée mais n’est pas disponible pour cette plateforme."
                ),
            ),
            CheckState::Unsupported => line(
                Glyph::Info,
                p.warning,
                "Mise à jour non disponible pour cette plateforme.".to_owned(),
            ),
            CheckState::Failed(error) => column![
                ui::callout(
                    p,
                    p.warning,
                    Glyph::Alert,
                    headline(&error.code),
                    error.message.clone(),
                ),
                ui::caption(
                    p,
                    "L’état des mises à jour est inconnu : ce n’est pas « à jour »."
                ),
            ]
            .spacing(6)
            .into(),
            CheckState::Available(update) => container(
                column![
                    line(
                        Glyph::Bolt,
                        p.accent,
                        "Une nouvelle mise à jour est disponible".to_owned()
                    ),
                    self.available_view(p, update, false),
                ]
                .spacing(12),
            )
            .padding(14)
            .width(Length::Fill)
            .style(theme::outline(*p, p.accent, 1.0))
            .into(),
        };
        let mut col = column![header, status].spacing(14);
        if !matches!(self.updates.check, CheckState::Available(_))
            && let Some(error) = &self.updates.error
        {
            col = col.push(self.error_view(p, error));
        }
        if !self.updates.cleanup_failed.is_empty() {
            col = col.push(ui::caption(
                p,
                format!(
                    "Restes d’une mise à jour précédente non retirés (nouvel essai au prochain démarrage) : {}",
                    self.updates.cleanup_failed.join(" ; ")
                ),
            ));
        }
        col.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn installed() -> Result<Installed, Failure> {
        Ok(Installed {
            desktop: PathBuf::from("syncengine-desktop"),
            version: "0.2.0".into(),
            warnings: Vec::new(),
        })
    }

    fn state() -> Result<tempfile::TempDir, std::io::Error> {
        tempfile::tempdir()
    }

    #[test]
    fn dates_and_folding() {
        assert_eq!(
            published(Some("2026-10-20T08:30:00Z")).as_deref(),
            Some("20/10/2026")
        );
        assert_eq!(published(Some("bientôt")), None);
        assert_eq!(published(None), None);
        assert!(!long_notes(
            "- a
- b"
        ));
        assert!(long_notes(
            &"- ligne
"
            .repeat(FOLDED_LINES + 1)
        ));
        assert!(long_notes(&"x".repeat(FOLDED_CHARS + 1)));
        assert!(manual_download("not_writable"));
        assert!(!manual_download("offline"));
        assert_ne!(headline("offline"), headline("checksum_mismatch"));
    }

    #[test]
    fn without_agent_nothing_is_stopped_or_restarted() -> TestResult {
        let dir = state()?;
        let restarts = AtomicUsize::new(0);
        let result = around_agent(
            dir.path(),
            Duration::from_secs(1),
            |_| {
                restarts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            installed,
        );
        assert!(result.is_ok());
        assert_eq!(restarts.load(Ordering::SeqCst), 0);
        assert!(!sync_orchestrator::agent_stop_pending(dir.path()));
        Ok(())
    }

    #[test]
    fn dispatching_agent_refuses_without_touching_anything() -> TestResult {
        let dir = state()?;
        let _presence = sync_orchestrator::hold_agent_presence(dir.path())?;
        sync_orchestrator::publish_resident_status(
            dir.path(),
            sync_orchestrator::ResidentPhase::Dispatching,
        )?;
        let worked = AtomicUsize::new(0);
        let result = around_agent(
            dir.path(),
            Duration::from_secs(1),
            |_| Ok(()),
            || {
                worked.fetch_add(1, Ordering::SeqCst);
                installed()
            },
        );
        let error = result.err().ok_or("refus attendu")?;
        assert_eq!(error.code, "busy");
        assert!(error.message.contains("synchronisation planifiée"));
        assert_eq!(worked.load(Ordering::SeqCst), 0);
        assert!(!sync_orchestrator::agent_stop_pending(dir.path()));
        Ok(())
    }

    #[test]
    fn idle_agent_is_stopped_then_restarted() -> TestResult {
        let dir = state()?;
        let path = dir.path().to_path_buf();
        let presence = sync_orchestrator::hold_agent_presence(&path)?;
        sync_orchestrator::publish_resident_status(&path, sync_orchestrator::ResidentPhase::Idle)?;
        // Simulated agent: leaves as soon as the stop request appears.
        let watcher = std::thread::spawn({
            let path = path.clone();
            move || {
                while !sync_orchestrator::agent_stop_pending(&path) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                drop(presence);
            }
        });
        let restarts = AtomicUsize::new(0);
        let result = around_agent(
            &path,
            Duration::from_secs(10),
            |_| {
                restarts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            || {
                // The agent is gone while files are replaced.
                assert!(!sync_orchestrator::resident_status(&path)?.available);
                installed()
            },
        );
        watcher.join().map_err(|_| "agent simulé interrompu")?;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(restarts.load(Ordering::SeqCst), 1);
        assert!(!sync_orchestrator::agent_stop_pending(&path));
        Ok(())
    }

    #[test]
    fn agent_that_does_not_stop_blocks_the_replacement() -> TestResult {
        let dir = state()?;
        let _presence = sync_orchestrator::hold_agent_presence(dir.path())?;
        sync_orchestrator::publish_resident_status(
            dir.path(),
            sync_orchestrator::ResidentPhase::Idle,
        )?;
        let worked = AtomicUsize::new(0);
        let result = around_agent(
            dir.path(),
            Duration::from_millis(300),
            |_| Ok(()),
            || {
                worked.fetch_add(1, Ordering::SeqCst);
                installed()
            },
        );
        assert_eq!(result.err().map(|e| e.code), Some("busy".to_owned()));
        assert_eq!(worked.load(Ordering::SeqCst), 0);
        // The request left for this update is lifted: schedules continue.
        assert!(!sync_orchestrator::agent_stop_pending(dir.path()));
        Ok(())
    }

    #[test]
    fn explicit_user_stop_is_kept() -> TestResult {
        let dir = state()?;
        sync_orchestrator::request_agent_stop(dir.path())?;
        let restarts = AtomicUsize::new(0);
        let result = around_agent(
            dir.path(),
            Duration::from_secs(1),
            |_| {
                restarts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            installed,
        );
        assert!(result.is_ok());
        assert_eq!(restarts.load(Ordering::SeqCst), 0);
        assert!(sync_orchestrator::agent_stop_pending(dir.path()));
        Ok(())
    }
}
