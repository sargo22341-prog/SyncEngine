//! Profile page: identity, endpoints with the animated flow, the simulation
//! (plan inspection) and execution panels, recent results and planning.

use crate::app::{App, Job, JobKind, Message};
use crate::charts::{self, Flow};
use crate::draft::{self, Provider};
use crate::icons::{self, Glyph};
use crate::model::{self, OpView};
use crate::shell::{mode_color, mode_fill, outcome_color};
use crate::speed;
use crate::theme::{self, Palette, Tone};
use crate::tree::ListMode;
use crate::ui::{self, El};
use iced::widget::{Space, button, column, container, row, scrollable, text_input};
use iced::{Alignment, Color, Length, Padding};
use std::time::Instant;
use sync_orchestrator::ConflictKeep;
use sync_plan::{Action, SyncMode};
use sync_profiles::Profile;

pub(crate) fn action_color(p: &Palette, action: Action) -> Color {
    match action {
        Action::Copy => p.backup,
        Action::Replace => p.violet,
        Action::Delete => p.danger,
        Action::Conflict => p.warning,
        Action::Keep => p.faint,
        _ => p.muted,
    }
}

/// Files listed under the throughput curve: one per parallel copy.
const SHOWN_TRANSFERS: usize = 4;
/// A copy is listed only once it lasts this long (no flicker).
const SHOWN_AFTER_MS: u64 = 500;

/// Files being copied (from the live snapshot, never from the journal):
/// up to four lines with path, own progress bar, bytes, average rate and
/// percentage; shorter copies are summarized as a rate.
fn transfers<'a>(
    p: &Palette,
    s: &sync_orchestrator::JobSnapshot,
    quick_rate: Option<f32>,
    color: Color,
) -> El<'a, Message> {
    let shown: Vec<_> = s
        .transfers
        .iter()
        .filter(|f| f.elapsed_ms >= SHOWN_AFTER_MS)
        .take(SHOWN_TRANSFERS)
        .collect();
    let mut head = row![
        ui::strong(p.text, "Fichiers en cours", 14.0),
        ui::badge(
            p.surface_hi,
            p.muted,
            format!("{} copies parallèles", s.transfers.len())
        ),
        ui::fill(),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if let Some(rate) = quick_rate.filter(|r| r.round() >= 1.0) {
        head = head.push(ui::caption(
            p,
            format!("+ {} petits fichiers/s", model::count(rate.round() as u64)),
        ));
    }
    let mut col = column![container(head).padding(Padding::from([6, 16]))].spacing(4);
    for file in shown {
        let fraction = file
            .size
            .and_then(|size| model::fraction(file.bytes_done, size))
            .unwrap_or(0.0);
        let rate = file.bytes_done as f64 / (file.elapsed_ms.max(1) as f64 / 1000.0);
        let amount = match file.size {
            Some(size) => format!("{} / {}", model::bytes(file.bytes_done), model::bytes(size)),
            None => model::bytes(file.bytes_done),
        };
        col = col.push(
            container(
                row![
                    container(ui::mono(p.backup_ink, "→", 13.0)).width(Length::Fixed(24.0)),
                    column![
                        container(ui::mono(p.text, model::ellipsize(&file.path, 64), 12.0))
                            .width(Length::Fill)
                            .clip(true),
                        ui::progress(p, fraction, color, 6.0),
                    ]
                    .spacing(6)
                    .width(Length::Fill),
                    container(ui::mono(p.muted, amount, 12.0))
                        .width(Length::Fixed(140.0))
                        .align_x(iced::alignment::Horizontal::Right),
                    container(ui::mono(
                        p.muted,
                        format!("{}/s", model::bytes(rate as u64)),
                        12.0
                    ))
                    .width(Length::Fixed(100.0))
                    .align_x(iced::alignment::Horizontal::Right),
                    container(ui::mono(p.muted, model::percent(fraction), 12.0))
                        .width(Length::Fixed(60.0))
                        .align_x(iced::alignment::Horizontal::Right),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([7, 16])),
        );
    }
    container(col)
        .padding(Padding::from([10, 4]))
        .width(Length::Fill)
        .style(theme::card(*p, 0.0, false))
        .into()
}

/// Phase of a run, as shown by the chips under the page title.
#[derive(Clone, Copy)]
enum Phase {
    Done,
    Now,
    Next,
}

/// Splits `model::bytes(rate)` into a figure and its `unit/s`.
pub(crate) fn rate_parts(bytes_per_second: f32) -> (String, String) {
    let text = model::bytes(bytes_per_second.max(0.0) as u64);
    match text.rsplit_once(' ') {
        Some((value, unit)) => (value.to_owned(), format!("{unit}/s")),
        None => (text, "/s".to_owned()),
    }
}

fn provider_of(remote: Option<&sync_profiles::RemoteEndpoint>) -> Provider {
    use sync_profiles::RemoteEndpoint as R;
    match remote {
        None => Provider::Local,
        Some(R::Smb { .. }) => Provider::Smb,
        Some(R::Sftp { .. }) => Provider::Sftp,
        Some(R::Webdav { .. }) => Provider::Webdav,
        Some(R::Ftp { .. }) => Provider::Ftp,
        Some(R::Ftps { .. }) => Provider::Ftps,
        Some(R::Onedrive { .. }) => Provider::Onedrive,
    }
}

/// Series computed by [`App::job_rates`].
pub(crate) struct JobRates {
    pub(crate) forward: Vec<f32>,
    pub(crate) reverse: Option<Vec<f32>>,
    pub(crate) operations: Vec<f32>,
    pub(crate) quick: Vec<f32>,
}

impl App {
    pub(crate) fn profile_page<'a>(&'a self, p: &Palette, profile: &'a Profile) -> El<'a, Message> {
        let id = &profile.id;
        let busy = self.busy();
        let running = self.job.as_ref().filter(|j| &j.profile == id);
        let group = if profile.group.is_empty() {
            "Sans groupe"
        } else {
            profile.group.as_str()
        };
        let tool = |label: &'static str, on: Option<Message>| -> El<'a, Message> {
            ui::action_sm(p, Tone::Secondary, None, label, on)
        };
        let crumbs = row![
            button(
                row![
                    ui::colored(p.muted, "‹", 14.0),
                    ui::colored(p.muted, "Profils", 13.0),
                    ui::colored(p.faint, "/", 13.0),
                    ui::strong(p.text, group.to_owned(), 13.0),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
            )
            .padding(Padding::from([4, 8]))
            .style(theme::btn(*p, Tone::Ghost))
            .on_press(Message::Select(None)),
            ui::fill(),
            tool("Modifier", (!busy).then(|| Message::Edit(id.clone()))),
            tool("Planifier", Some(Message::OpenSchedule(id.clone()))),
            tool("Dupliquer", Some(Message::Duplicate(id.clone()))),
            tool("Historique", Some(Message::ShowHistory(id.clone()))),
            tool(
                "Corbeille",
                Some(Message::Trash(crate::trash::TrashMsg::Open(id.clone())))
            ),
            tool("Retirer", (!busy).then(|| Message::AskRemove(id.clone()))),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let color = mode_color(p, profile.mode);
        let mut pills = row![ui::tag(p, model::compare_label(profile.compare))].spacing(6);
        match (self.schedule_of(id), self.schedule_valid(id)) {
            (Some(s), Some(true)) => {
                pills = pills.push(ui::chip(
                    theme::alpha(p.violet, 0.14),
                    p.info,
                    draft::trigger_label(&s.schedule.trigger),
                ))
            }
            (Some(_), _) => {
                pills = pills.push(ui::chip(
                    theme::alpha(p.danger, 0.14),
                    p.danger,
                    "Planification à renouveler",
                ))
            }
            _ => {}
        }
        if profile.allow_delete {
            pills = pills.push(ui::chip(
                theme::alpha(p.danger, 0.14),
                p.danger,
                "Suppressions autorisées",
            ));
        }
        if profile.trash.enabled {
            pills = pills.push(ui::tag(
                p,
                format!("Corbeille {}", model::bytes(profile.trash.max_bytes)),
            ));
        }
        if let Some(run) = self.last_run(id) {
            let (wash, ink, label) = match run.outcome {
                model::Outcome::Success => (
                    theme::alpha(p.backup, if p.is_light() { 0.22 } else { 0.16 }),
                    p.backup_ink,
                    "✓ À jour".to_owned(),
                ),
                model::Outcome::Warning => {
                    (theme::alpha(p.warning, 0.22), p.warning, run.label.clone())
                }
                _ => (theme::alpha(p.danger, 0.14), p.danger, run.label.clone()),
            };
            pills = pills.push(ui::chip(wash, ink, label));
        }
        let main_action: El<'a, Message> = match running {
            Some(job) => row![
                pause_button(p, job),
                ui::action(
                    p,
                    Tone::Danger,
                    None,
                    if job.cancel_requested {
                        "Annulation…"
                    } else {
                        "Annuler"
                    },
                    (!job.cancel_requested).then_some(cancel_message(job)),
                ),
            ]
            .spacing(8)
            .into(),
            None => button(
                row![
                    ui::strong(
                        if busy { theme::alpha(p.bg, 0.5) } else { p.bg },
                        "Simuler",
                        15.0
                    ),
                    container(ui::mono(theme::alpha(p.bg, 0.75), "F5", 11.0))
                        .padding(Padding::from([1, 7]))
                        .style(theme::pill(theme::alpha(p.bg, 0.16), p.bg)),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([14, 28]))
            .style(theme::btn(*p, Tone::Primary))
            .on_press_maybe((!busy).then(|| Message::Simulate(id.clone())))
            .into(),
        };
        let heading = row![
            ui::mode_tile(p, profile.mode, 56.0),
            column![
                row![
                    ui::title(p, profile.name.clone(), 32.0),
                    ui::mode_chip(p, profile.mode),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                pills.wrap(),
            ]
            .spacing(8)
            .width(Length::Fill),
            main_action,
        ]
        .spacing(16)
        .align_y(Alignment::Center);
        let hero = heading;

        let interrupted = self
            .last_run(id)
            .is_some_and(|run| run.outcome == model::Outcome::Interrupted)
            || self
                .schedule_of(id)
                .is_some_and(|schedule| schedule.running_slot.is_some());
        let diagnosis: El<'a, Message> = if interrupted && !self.running_on(id) {
            ui::callout(
                p,
                p.warning,
                Glyph::Alert,
                "Exécution interrompue à vérifier",
                "L’agent peut être démarré, mais ce profil reste bloqué après une interruption. Vérifiez les fichiers et l’historique, puis confirmez le diagnostic pour autoriser une nouvelle simulation ou retirer le profil.",
            )
        } else {
            Space::new().into()
        };
        let diagnosis_action: El<'a, Message> = if interrupted && !self.running_on(id) {
            ui::action(
                p,
                Tone::Secondary,
                Some(Glyph::Check),
                "Diagnostiquer l’interruption",
                (!busy).then(|| Message::AskDiagnose(id.clone())),
            )
        } else {
            Space::new().into()
        };
        let turn = self
            .now
            .saturating_duration_since(self.started)
            .as_secs_f32();
        let endpoints = row![
            self.endpoint_card(
                p,
                "Source",
                provider_of(profile.remote_source.as_ref()),
                model::source_of(profile),
                &profile.remote_source
            ),
            container(charts::flow(
                Flow {
                    phase: turn,
                    color,
                    two_way: profile.mode == SyncMode::Bidirectional,
                    active: running.is_some_and(|j| j.kind == JobKind::Execute),
                },
                86.0
            ))
            .center_y(Length::Fixed(96.0)),
            self.endpoint_card(
                p,
                "Destination",
                provider_of(profile.remote_destination.as_ref()),
                model::destination_of(profile),
                &profile.remote_destination
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        let mut page = column![crumbs, hero].spacing(22);
        if interrupted && !self.running_on(id) {
            page = page.push(diagnosis).push(diagnosis_action);
        }
        let page = page
            .push(endpoints)
            .push(self.sim_panel(p, profile))
            .push(self.profile_footer(p, profile));
        scrollable(
            container(
                container(page)
                    .padding(Padding::from([22, 32]))
                    .max_width(1180.0),
            )
            .width(Length::Fill)
            .center_x(Length::Fill),
        )
        .style(theme::scroll(*p))
        .height(Length::Fill)
        .into()
    }

    fn endpoint_card<'a>(
        &'a self,
        p: &Palette,
        label: &'static str,
        provider: Provider,
        location: String,
        remote: &Option<sync_profiles::RemoteEndpoint>,
    ) -> El<'a, Message> {
        container(
            row![
                container(icons::icon(provider.glyph(), 20.0, p.text))
                    .width(Length::Fixed(44.0))
                    .height(Length::Fixed(44.0))
                    .center(Length::Fixed(44.0))
                    .style(theme::swatch(p.surface_hi)),
                // Clipped: a long unwrapped path never runs under the button.
                container(
                    column![
                        ui::eyebrow(
                            p.muted,
                            &format!(
                                "{label} · {}",
                                model::provider_label(remote.as_ref()).to_lowercase()
                            )
                        ),
                        ui::mono(p.text, model::ellipsize(&location, 48), 13.0),
                    ]
                    .spacing(3),
                )
                .width(Length::Fill)
                .clip(true),
                ui::icon_button(
                    p,
                    Glyph::Copy,
                    "Copier l’emplacement",
                    Some(Message::CopyText(location))
                ),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        )
        .padding(16)
        .width(Length::Fill)
        .style(theme::card(*p, 0.0, false))
        .into()
    }

    fn sim_panel<'a>(&'a self, p: &Palette, profile: &'a Profile) -> El<'a, Message> {
        if let Some(job) = [&self.job, &self.agent_job]
            .into_iter()
            .flatten()
            .find(|j| j.profile == profile.id)
        {
            return self.live_panel(p, job, true);
        }
        if let Some(preview) = self
            .preview
            .as_ref()
            .filter(|pr| pr.view.profile == profile.id.as_str())
        {
            return self.plan_view(p, preview, profile);
        }
        if let Some(last) = self.last_run.as_ref().filter(|l| l.profile == profile.id) {
            return self.result_card(p, last);
        }
        container(
            row![
                ui::glyph_box(Glyph::Eye, p.accent, 64.0),
                column![
                    ui::title(p, "Inspectez avant d’écrire", 19.0),
                    ui::muted(p, match profile.compare {
                        sync_profiles::CompareMethod::SizeAndDate => "La simulation lit les dossiers des deux côtés, compare tailles et dates et vous montre chaque opération prévue. Aucun fichier n’est modifié tant que vous n’avez pas confirmé ce plan précis.",
                        sync_profiles::CompareMethod::Content => "La simulation lit les deux côtés, compare le contenu des fichiers de même taille par empreinte et vous montre chaque opération prévue. Aucun fichier n’est modifié tant que vous n’avez pas confirmé ce plan précis.",
                    }),
                ]
                .spacing(6)
                .width(Length::Fill),
                ui::action(p, Tone::Primary, Some(Glyph::Eye), "Lancer la simulation", (!self.busy()).then(|| Message::Simulate(profile.id.clone()))),
            ]
            .spacing(20)
            .align_y(Alignment::Center),
        )
        .padding(24)
        .width(Length::Fill)
        .style(theme::card(*p, 0.0, false))
        .into()
    }

    /// Throughput series of a live job over a 4 s sliding window, as
    /// FreeFileSync (see `speed`): bytes/s toward the destination (or in all,
    /// when the profile is not two-way), the bytes/s toward the source when it
    /// is, and the operations/s rate.
    pub(crate) fn job_rates(&self, job: &Job) -> JobRates {
        let series = |count: fn(&crate::app::Sample) -> u64| -> Vec<f32> {
            let counts: Vec<(Instant, u64)> =
                job.samples.iter().map(|s| (s.at, count(s))).collect();
            speed::rates(&counts, job.started)
        };
        let two_ways = job.kind == JobKind::Execute
            && (job.snapshot.bytes_to_source > 0
                || self.job_mode(job) == Some(SyncMode::Bidirectional));
        let forward = match job.kind {
            JobKind::Execute if two_ways => series(|s| s.bytes.saturating_sub(s.to_source)),
            JobKind::Execute => series(|s| s.bytes),
            JobKind::Preview => series(|s| s.scanned),
        };
        JobRates {
            reverse: two_ways.then(|| series(|s| s.to_source)),
            operations: match job.kind {
                JobKind::Execute => series(|s| s.operations),
                JobKind::Preview => Vec::new(),
            },
            quick: match job.kind {
                JobKind::Execute => series(|s| s.quick),
                JobKind::Preview => Vec::new(),
            },
            forward,
        }
    }

    /// Live job view (activity page, profile page, tracker window), for a job
    /// of this window or a scheduled run of the agent. `large` is the full
    /// page; otherwise the compact tracker card.
    pub(crate) fn live_panel<'a>(
        &'a self,
        p: &Palette,
        job: &'a Job,
        large: bool,
    ) -> El<'a, Message> {
        let s = &job.snapshot;
        let elapsed = self.now.saturating_duration_since(job.started);
        let progress = job.progress();
        let rates = self.job_rates(job);
        // Unknown mode (profiles not loaded yet): no chip rather than a wrong one.
        let mode = self.job_mode(job);
        let mode_chip = |p: &Palette| -> El<'a, Message> {
            mode.map_or_else(|| Space::new().into(), |m| ui::mode_chip(p, m))
        };
        let execute = job.kind == JobKind::Execute;
        let color = mode.map_or(p.violet, |m| mode_fill(p, m));
        let remaining = job.remaining_label(self.now);
        let forward = rates.forward.last().copied();
        let total_rate = forward.map(|f| {
            f + rates
                .reverse
                .as_ref()
                .and_then(|r| r.last())
                .copied()
                .unwrap_or(0.0)
        });
        let (value, unit) = match (execute, total_rate) {
            (_, None) => ("…".to_owned(), String::new()),
            (true, Some(rate)) => rate_parts(rate),
            (false, Some(rate)) => (model::count(rate as u64), "éléments/s".to_owned()),
        };
        let controls = row![
            pause_button(p, job),
            ui::action(
                p,
                Tone::Danger,
                None,
                if job.cancel_requested {
                    "Annulation…"
                } else {
                    "Annuler"
                },
                (!job.cancel_requested).then_some(cancel_message(job)),
            ),
        ]
        .spacing(10);
        if !large {
            return container(
                column![
                    row![
                        ui::strong(p.text, model::ellipsize(&job.name, 22), 14.0),
                        mode_chip(p),
                        ui::fill(),
                        ui::mono(
                            p.muted,
                            progress.map_or_else(|| "…".to_owned(), model::percent),
                            12.0
                        ),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    ui::metric(p, value, unit, 40.0),
                    charts::spark(
                        rates.forward.clone(),
                        p.violet,
                        rates.reverse.clone().map(|r| (r, p.backup)),
                        Color::TRANSPARENT,
                        36.0
                    ),
                    ui::progress(p, progress.unwrap_or(0.0), color, 6.0),
                    ui::caption(
                        p,
                        match &remaining {
                            Some(r) => format!("écoulé {} · {r}", clock(elapsed.as_secs())),
                            None => format!("écoulé {}", clock(elapsed.as_secs())),
                        }
                    ),
                    controls,
                ]
                .spacing(10),
            )
            .padding(16)
            .width(Length::Fill)
            .style(theme::tint(
                p.violet,
                if p.is_light() { 0.12 } else { 0.17 },
                theme::RADIUS,
            ))
            .into();
        }

        let heading = if s.pause_requested {
            "Pause demandée".to_owned()
        } else {
            match (job.agent, job.kind) {
                (true, JobKind::Preview) => {
                    "Lancée par l’agent · analyse avant écritures".to_owned()
                }
                (true, JobKind::Execute) => "Lancée par l’agent · écritures en cours".to_owned(),
                (false, JobKind::Preview) => "Simulation · analyse en cours".to_owned(),
                (false, JobKind::Execute) => "Exécution réelle · écritures en cours".to_owned(),
            }
        };
        let kind_chip: El<'a, Message> = if execute {
            ui::chip(
                theme::alpha(p.danger, 0.14),
                if p.is_light() {
                    p.danger
                } else {
                    theme::mix(p.danger, p.text, 0.2)
                },
                "Réel · modifie les fichiers",
            )
        } else {
            ui::chip(
                theme::alpha(p.violet, 0.14),
                p.info,
                "Simulation · aucun fichier modifié",
            )
        };
        let header = row![
            column![
                ui::eyebrow(p.muted, &heading),
                row![
                    ui::title(p, model::ellipsize(&job.name, 40), 32.0),
                    mode_chip(p),
                    kind_chip,
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            ]
            .spacing(2)
            .width(Length::Fill),
            controls,
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        // Phases: what is finished, what runs, what comes next.
        let phase = |label: &'static str, state: Phase| -> El<'a, Message> {
            match state {
                Phase::Done => ui::chip(
                    theme::alpha(p.backup, if p.is_light() { 0.22 } else { 0.16 }),
                    p.backup_ink,
                    format!("✓ {label}"),
                ),
                Phase::Now => ui::chip(p.text, p.bg, label),
                Phase::Next => ui::tag(p, label),
            }
        };
        let hashing = s.hash_target > 0 || s.entries_hashed > 0;
        let phases: Vec<El<'a, Message>> = if execute {
            vec![
                phase("Lecture", Phase::Done),
                phase("Comparaison", Phase::Done),
                phase("Écriture", Phase::Now),
                phase("Vérification", Phase::Next),
                phase("Bilan", Phase::Next),
            ]
        } else {
            vec![
                phase("Lecture", if hashing { Phase::Done } else { Phase::Now }),
                phase(
                    "Comparaison",
                    if hashing { Phase::Now } else { Phase::Next },
                ),
                phase("Bilan", Phase::Next),
            ]
        };
        let secs = elapsed.as_secs();
        let timing = match &remaining {
            Some(r) => format!("écoulé {} · {r}", clock(secs)),
            None => format!("écoulé {}", clock(secs)),
        };
        let phase_row = row(phases)
            .spacing(6)
            .push(ui::fill())
            .push(ui::mono(p.muted, timing, 12.0))
            .align_y(Alignment::Center);

        // Throughput card.
        let two_ways = rates.reverse.is_some();
        let mut head = row![
            column![
                ui::eyebrow(
                    p.muted,
                    if execute {
                        "Débit instantané"
                    } else {
                        "Vitesse d’analyse"
                    }
                ),
                ui::metric(p, value, unit, 52.0),
            ]
            .spacing(2)
        ]
        .align_y(Alignment::Center);
        head = head.push(ui::fill());
        if let Some(reverse) = &rates.reverse {
            let rate = |v: Option<&f32>| v.map_or("—".to_owned(), |r| model::bytes(*r as u64));
            head = head.push(
                column![
                    row![
                        ui::dot(p.backup, 8.0),
                        ui::colored(p.muted, "Vers la destination", 12.0),
                        ui::mono(p.text, rate(rates.forward.last()), 12.0),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    row![
                        ui::dot(p.violet, 8.0),
                        ui::colored(p.muted, "Vers la source", 12.0),
                        ui::mono(p.text, rate(reverse.last()), 12.0),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                ]
                .spacing(4)
                .align_x(Alignment::End),
            );
        }
        let chart = container(
            column![
                head,
                charts::spark(
                    rates.forward.clone(),
                    if two_ways { p.backup } else { p.violet },
                    rates.reverse.clone().map(|r| (r, p.violet)),
                    theme::alpha(p.text, 0.07),
                    190.0,
                ),
                ui::caption(p, "Moyenne glissante sur 4 s · fenêtre de 30 s"),
            ]
            .spacing(10),
        )
        .padding(20)
        .width(Length::Fill)
        .style(theme::card(*p, 0.0, false));

        // Right column.
        let mini = |label: &'static str, value: String, unit: String| -> El<'a, Message> {
            container(
                column![
                    ui::eyebrow(p.muted, label),
                    ui::metric(p, value, unit, 20.0)
                ]
                .spacing(4),
            )
            .padding(14)
            .width(Length::FillPortion(1))
            .style(theme::card(*p, 0.0, false))
            .into()
        };
        let mut side = column![].spacing(12);
        if execute {
            let planned_bytes = job.planned_bytes.filter(|b| *b > 0);
            let amount = match planned_bytes {
                Some(total) => format!("{} / {}", model::bytes(s.bytes_done), model::bytes(total)),
                None => model::bytes(s.bytes_done),
            };
            side = side.push(
                container(
                    column![
                        ui::eyebrow(p.muted, "Progression"),
                        row![
                            ui::metric(
                                p,
                                progress.map_or("—".to_owned(), |f| format!("{:.0}", f * 100.0)),
                                "%",
                                24.0
                            ),
                            ui::fill(),
                            ui::mono(p.muted, amount, 12.0),
                        ]
                        .align_y(Alignment::Center),
                        ui::progress(p, progress.unwrap_or(0.0), color, 10.0),
                        ui::caption(
                            p,
                            format!(
                                "calculée sur le volume · {} / {} opérations",
                                model::count(s.entries_done),
                                model::count(job.planned.unwrap_or(0))
                            )
                        ),
                    ]
                    .spacing(6),
                )
                .padding(20)
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false)),
            );
            let average = if secs > 0 {
                s.bytes_transferred as f32 / secs as f32
            } else {
                0.0
            };
            let (avg_value, avg_unit) = rate_parts(average);
            let ops = rates.operations.last().copied().unwrap_or(0.0);
            side = side.push(
                row![
                    mini("Moyenne", avg_value, avg_unit),
                    mini(
                        "Opérations",
                        model::count(ops.round() as u64),
                        "/s".to_owned()
                    ),
                ]
                .spacing(12),
            );
            if s.operation_errors > 0 {
                side = side.push(ui::callout(
                    p,
                    p.danger,
                    Glyph::Alert,
                    "Refus ou échecs",
                    format!(
                        "{} opération(s) n’ont pas abouti ; le détail est dans l’historique.",
                        model::count(s.operation_errors)
                    ),
                ));
            }
        } else {
            side = side.push(
                container(
                    column![
                        ui::eyebrow(p.muted, "Analyse"),
                        row![ui::metric(
                            p,
                            model::count(s.entries_scanned),
                            "entrées",
                            24.0
                        ),],
                        ui::progress(p, progress.unwrap_or(0.0), color, 10.0),
                        ui::caption(
                            p,
                            if s.hash_target > 0 {
                                format!(
                                    "empreintes {} / {}",
                                    model::count(s.entries_hashed),
                                    model::count(s.hash_target)
                                )
                            } else {
                                "lecture des deux dossiers (2 côtés)".to_owned()
                            }
                        ),
                    ]
                    .spacing(6),
                )
                .padding(20)
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false)),
            );
            side = side.push(
                row![
                    mini(
                        "Erreurs de lecture",
                        model::count(s.scan_issues),
                        String::new()
                    ),
                    mini(
                        "Cohérence incertaine",
                        model::count(s.scan_consistency_warnings),
                        String::new()
                    ),
                ]
                .spacing(12),
            );
        }
        if s.scan_consistency_warnings > 0 {
            side = side.push(ui::callout(
                p,
                p.warning,
                Glyph::Alert,
                "Listes non garanties",
                "Le partage ne garantit pas la cohérence de ses listes : les absences restent incertaines et ne permettent aucune suppression miroir.",
            ));
        }
        let mut body = column![
            header,
            phase_row,
            row![
                container(chart).width(Length::Fill),
                container(side).width(Length::Fixed(300.0)),
            ]
            .spacing(12),
        ]
        .spacing(16);
        if execute {
            let quick = rates.quick.last().copied();
            body = body.push(transfers(p, s, quick, color));
        }
        if job.agent {
            body = body.push(
                row![
                    container(ui::caption(p, "Arrêter l’agent annule le transfert et suspend les jobs jusqu’à « Reprendre les planifications ». Aucun processus n’est tué de force.")).width(Length::Fill),
                    ui::action(p, Tone::Secondary, Some(Glyph::Power), "Arrêter l’agent", (!self.ws.agent_stopped).then_some(Message::StopAgent)),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
        } else {
            body = body.push(ui::caption(
                p,
                if job.cancel_requested {
                    "Annulation coopérative demandée : l’opération en cours se termine proprement."
                } else {
                    "Vous pouvez annuler à tout moment ; l’état partiel est conservé et journalisé."
                },
            ));
        }
        body.into()
    }

    pub(crate) fn result_card<'a>(
        &'a self,
        p: &Palette,
        last: &'a crate::app::LastRun,
    ) -> El<'a, Message> {
        let (color, glyph, title, body, totals) = match &last.result {
            Ok(done) => (
                outcome_color(p, done.outcome),
                match done.outcome {
                    model::Outcome::Success => Glyph::Check,
                    _ => Glyph::Alert,
                },
                done.label.clone(),
                format!("{} · {}", last.name, seconds(last.duration.as_secs_f32())),
                done.totals.clone(),
            ),
            Err(e) => (
                p.danger,
                Glyph::Alert,
                e.title().to_owned(),
                e.message.clone(),
                Vec::new(),
            ),
        };
        let chips = totals.into_iter().filter(|(_, v)| *v > 0).map(|(k, v)| {
            let value = if k == "bytes" {
                model::bytes(v)
            } else {
                model::count(v)
            };
            ui::badge(
                theme::alpha(p.text, 0.08),
                p.text,
                format!("{value} {}", model::total_label(&k)),
            )
        });
        let mut content = column![
            row![
                ui::glyph_box(glyph, color, 56.0),
                column![
                    ui::title(p, title, 19.0),
                    ui::muted(p, body),
                    row(chips).spacing(6).wrap()
                ]
                .spacing(6)
                .width(Length::Fill),
                column![
                    ui::action(
                        p,
                        Tone::Secondary,
                        Some(Glyph::History),
                        "Historique",
                        Some(Message::ShowHistory(last.profile.clone()))
                    ),
                    ui::action(
                        p,
                        Tone::Ghost,
                        Some(Glyph::Eye),
                        "Nouvelle simulation",
                        (!self.busy()).then(|| Message::Simulate(last.profile.clone()))
                    ),
                ]
                .spacing(8)
                .align_x(Alignment::End),
            ]
            .spacing(18)
            .align_y(Alignment::Center)
        ]
        .spacing(16);
        if let Ok(done) = &last.result {
            if let Some(message) = &done.diagnostics_error {
                content = content.push(ui::callout(
                    p,
                    p.warning,
                    Glyph::Alert,
                    "Diagnostics non chargés",
                    message.clone(),
                ));
            }
            if done.problems_total > 0 {
                let mut problems = column![ui::strong(p.warning, format!("{} opérations à examiner", model::count(done.problems_total as u64)), 14.0),
                    ui::caption(p, "Les refus de sécurité, échecs et opérations ignorées sont détaillés ci-dessous. Un refus n’est pas une copie réussie.")].spacing(8);
                for problem in &done.problems {
                    let path = problem
                        .op
                        .as_ref()
                        .map_or("Chemin inconnu", |op| op.path.as_str());
                    problems = problems.push(
                        column![
                            ui::mono(p.text, model::ellipsize(path, 90), 12.0),
                            ui::caption(p, problem.detail.clone())
                        ]
                        .spacing(2),
                    );
                }
                problems = problems.push(ui::action(
                    p,
                    Tone::Secondary,
                    Some(Glyph::History),
                    "Voir tous les problèmes dans l’historique",
                    Some(Message::ShowRunProblems(
                        last.profile.clone(),
                        done.execution.clone(),
                    )),
                ));
                content = content.push(
                    container(problems)
                        .padding(14)
                        .style(theme::outline(*p, p.warning, 0.5)),
                );
            }
        }
        container(content)
            .padding(22)
            .width(Length::Fill)
            .style(theme::outline(*p, color, 0.7))
            .into()
    }

    fn plan_view<'a>(
        &'a self,
        p: &Palette,
        preview: &'a crate::app::PreviewState,
        profile: &'a Profile,
    ) -> El<'a, Message> {
        let s = preview.view.summary;
        let reveal = preview.reveal.at(self.now).clamp(0.0, 1.0);
        let tiles = [
            Action::Copy,
            Action::Replace,
            Action::Delete,
            Action::Conflict,
            Action::Skip,
            Action::Keep,
        ]
        .into_iter()
        .map(|a| {
            let n = s.count(a);
            let shown = ((n as f32) * reveal).round() as u64;
            let c = action_color(p, a);
            let selected = preview.filter == Some(a);
            button(
                column![
                    row![
                        ui::dot(c, 8.0),
                        ui::colored(
                            if a == Action::Delete && n > 0 {
                                p.danger
                            } else {
                                p.muted
                            },
                            model::action_label(a),
                            12.0
                        )
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    ui::metric(p, model::count(shown), "", 24.0),
                ]
                .spacing(4),
            )
            .padding(Padding::from([12, 16]))
            .width(Length::Fill)
            .style(theme::tile(*p, selected, true))
            .on_press(Message::PlanFilter(if selected { None } else { Some(a) }))
            .into()
        });
        // Proportion bar of all planned operations.
        let total = s.total().max(1);
        let segments = [
            Action::Copy,
            Action::Replace,
            Action::Delete,
            Action::Conflict,
            Action::Skip,
            Action::Keep,
        ]
        .into_iter()
        .filter(|a| s.count(*a) > 0)
        .map(|a| {
            let share = ((s.count(a) as f32 / total as f32) * 1000.0 * reveal).max(1.0) as u16;
            container(Space::new())
                .width(Length::FillPortion(share))
                .height(Length::Fixed(8.0))
                .style(theme::bar(action_color(p, a)))
                .into()
        });
        let rest = Space::new()
            .width(Length::FillPortion(
                ((1.0 - reveal) * 1000.0).max(1.0) as u16
            ))
            .into();
        let bar = row(segments.chain(std::iter::once(rest))).spacing(3);

        let mut col = column![
            row![
                ui::chip(
                    theme::alpha(p.violet, 0.14),
                    p.info,
                    "SIMULATION · AUCUN FICHIER MODIFIÉ"
                ),
                ui::fill(),
                ui::colored(
                    p.muted,
                    format!("{} à transférer", model::bytes(s.bytes)),
                    13.0
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row(tiles).spacing(10),
            bar,
        ]
        .spacing(14);

        if profile.mode == sync_plan::SyncMode::Mirror {
            col=col.push(ui::caption(p,"Miroir : les fichiers déjà prévus à la copie sont relus dans leur état actuel au démarrage de leur transfert. Un fichier modifié pendant sa lecture reste refusé ; les suppressions et la destination gardent leurs contrôles."));
        }
        if !preview.view.source_complete || !preview.view.destination_complete {
            col = col.push(ui::callout(
                p,
                p.warning,
                Glyph::Alert,
                "Inventaire incomplet",
                "Certains chemins n’ont pas pu être lus : ils sont ignorés et aucune suppression n’est déduite de leur absence.",
            ));
        }
        if s.trash.overflow {
            col = col.push(ui::callout(
                p,
                p.danger,
                Glyph::Alert,
                "Corbeille insuffisante",
                format!(
                    "{} L’exécution demandera une confirmation explicite ; une synchronisation planifiée sera bloquée.",
                    s.trash.overflow_message()
                ),
            ));
        }
        if s.ceiling_lifted && s.delete > 0 {
            col = col.push(ui::callout(
                p,
                p.danger,
                Glyph::Trash,
                &format!(
                    "Plafond de suppressions levé : {} suppression(s) prévue(s)",
                    model::count(s.delete)
                ),
                "Vous avez autorisé ce volume pour cette simulation uniquement. Vérifiez la liste « Suppressions » ; l’exécution demandera encore votre confirmation. « Recalculer » rétablit le plafond.",
            ));
        }
        if s.deletions_refused > 0 {
            let title = format!(
                "{} suppression(s) refusée(s)",
                model::count(s.deletions_refused)
            );
            let (message, liftable) = match (profile.allow_delete, s.deletion_refusal) {
                (false, _) => ("Ce profil interdit les suppressions : la destination conserve des fichiers absents de la source. Autorisez-les dans « Sécurité » puis recalculez.".to_owned(), false),
                // The label already says what happens to the deletions.
                (true, Some(refusal)) => (
                    model::refusal_label(refusal),
                    model::ceiling_liftable(refusal),
                ),
                (true, None) => ("Certaines suppressions sont interdites ou incertaines : la destination les conserve.".to_owned(), false),
            };
            col = col.push(ui::callout(p, p.warning, Glyph::Shield, &title, message));
            if liftable {
                col = col.push(
                    row![
                        ui::fill(),
                        ui::action(
                            p,
                            Tone::Danger,
                            Some(Glyph::Trash),
                            format!(
                                "Autoriser ces {} suppressions pour cette simulation",
                                model::count(s.deletions_refused)
                            ),
                            (!self.busy())
                                .then(|| Message::LiftDeletionCeiling(profile.id.clone()))
                        ),
                    ]
                    .align_y(Alignment::Center),
                );
            }
        }
        if s.delete > 0 {
            let complete = preview.view.source_complete && preview.view.destination_complete;
            let guard = |ok: bool, label: String| -> El<'a, Message> {
                row![
                    container(if ok {
                        ui::chip(
                            theme::alpha(p.backup, if p.is_light() { 0.22 } else { 0.16 }),
                            p.backup_ink,
                            "✓",
                        )
                    } else {
                        ui::chip(theme::alpha(p.warning, 0.22), p.warning, "!")
                    })
                    .width(Length::Fixed(34.0)),
                    ui::colored(p.text, label, 13.0)
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
            };
            let trash_ok = s.trash.enabled && !s.trash.overflow;
            col = col.push(
                container(
                    column![
                        row![
                            icons::icon(Glyph::Trash, 18.0, p.danger),
                            column![
                                ui::strong(
                                    p.danger,
                                    format!(
                                        "{} suppression(s) à revoir avant d’exécuter",
                                        model::count(s.delete)
                                    ),
                                    15.0
                                ),
                                ui::caption(
                                    p,
                                    "Elles ne seront appliquées qu’après votre confirmation séparée."
                                ),
                            ]
                            .spacing(2)
                            .width(Length::Fill),
                            ui::action(
                                p,
                                Tone::Secondary,
                                Some(Glyph::Eye),
                                "Voir les suppressions",
                                Some(Message::PlanFilter(Some(Action::Delete))),
                            ),
                        ]
                        .spacing(12)
                        .align_y(Alignment::Center),
                        guard(
                            complete,
                            if complete {
                                "Inventaire complet des deux côtés".to_owned()
                            } else {
                                "Inventaire incomplet : aucune suppression n’est déduite des chemins illisibles".to_owned()
                            }
                        ),
                        guard(
                            trash_ok,
                            if trash_ok {
                                "Les fichiers supprimés iront dans la corbeille du profil".to_owned()
                            } else if s.trash.enabled {
                                "Corbeille trop petite : suppression définitive".to_owned()
                            } else {
                                "Corbeille désactivée : suppression définitive".to_owned()
                            }
                        ),
                        guard(
                            !s.ceiling_lifted,
                            if s.ceiling_lifted {
                                "Plafond de suppressions levé pour cette simulation".to_owned()
                            } else {
                                "Sous le plafond de sécurité (50 % de la destination, 10 minimum)".to_owned()
                            }
                        ),
                    ]
                    .spacing(8),
                )
                .padding(Padding::from([14, 16]))
                .width(Length::Fill)
                .style(theme::outline(*p, p.danger, 0.8)),
            );
        }
        if s.conflict > 0 {
            col = col.push(ui::callout(
                p,
                p.warning,
                Glyph::TwoWay,
                &format!("{} conflit(s) préservé(s)", model::count(s.conflict)),
                "Rien n’est écrit sur un conflit non résolu : les deux versions restent en place.",
            ));
            if profile.mode == SyncMode::Bidirectional {
                col = col.push(self.conflict_bar(p, profile));
            }
        }

        let execute_tone = if s.delete > 0 {
            Tone::Danger
        } else {
            Tone::Success
        };
        // Two lines so nothing is squeezed to a sliver in a narrow window:
        // search + actions (the field yields its width first), then the
        // action filters, which wrap when they no longer fit.
        let actions = row![
            container(
                text_input("Filtrer par chemin…", &preview.query)
                    .on_input(Message::PlanQuery)
                    .padding(Padding::from([9, 0]))
                    .size(13)
                    .style(theme::search(*p))
            )
            .padding(Padding::from([0, 16]))
            .width(Length::FillPortion(3))
            .max_width(320.0)
            .style(theme::field_box(*p, 999.0)),
            Space::new().width(Length::FillPortion(1)),
            ui::icon_button(
                p,
                Glyph::Close,
                "Fermer la simulation",
                Some(Message::DiscardPreview)
            ),
            ui::action(
                p,
                Tone::Secondary,
                None,
                "Recalculer",
                (!self.busy()).then(|| Message::Simulate(profile.id.clone()))
            ),
            ui::action(
                p,
                execute_tone,
                None,
                if s.mutations() == 0 {
                    "Rien à appliquer".to_owned()
                } else {
                    format!("Exécuter {} opérations…", model::count(s.mutations()))
                },
                (s.mutations() > 0 && !self.busy()).then_some(Message::AskExecute)
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        let filters = row![
            ui::seg(
                p,
                &[
                    (None, "Tout"),
                    (Some(Action::Copy), "Copies"),
                    (Some(Action::Replace), "Remplacements"),
                    (Some(Action::Delete), "Suppressions"),
                    (Some(Action::Conflict), "Conflits"),
                ],
                preview.filter,
                Message::PlanFilter,
            ),
            Space::new().width(Length::Fixed(12.0)),
            mode_switch(p, self.list_mode),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let toolbar = column![actions, filters].spacing(10);
        let compact = self.ws.preferences.compact;
        let arbitrating =
            preview.filter == Some(Action::Conflict) && profile.mode == SyncMode::Bidirectional;
        if self.list_mode == ListMode::Tree && !arbitrating {
            let pal = *p;
            let tree = crate::tree::view(
                p,
                &preview.tree,
                Message::PlanTree,
                &|op, indent| op_row(&pal, op, compact, Some(indent)),
                &|folder| crate::tree::action_summary(&pal, folder),
                &|folder| {
                    Some(ui::action(
                        &pal,
                        Tone::Ghost,
                        Some(Glyph::Close),
                        "Exclure",
                        (!self.busy()).then(|| Message::ExcludeFolder(folder.path.clone())),
                    ))
                },
                "Aucune opération pour ce filtre.",
            );
            col = col.push(toolbar).push(
                container(tree)
                    .padding(8)
                    .width(Length::Fill)
                    .style(theme::card(*p, 0.0, false)),
            );
            return container(col).width(Length::Fill).into();
        }

        let mut list = column![
            row![
                container(ui::eyebrow(p.faint, "Action")).width(Length::Fixed(84.0)),
                container(ui::eyebrow(p.faint, "Chemin et motif")).width(Length::Fill),
                container(ui::eyebrow(p.faint, "Taille")).width(Length::Fixed(90.0)),
                container(ui::eyebrow(p.faint, "Cible")).width(Length::Fixed(130.0)),
            ]
            .padding(Padding::from([4, 12])),
        ]
        .spacing(2);
        for op in &preview.ops {
            list = list.push(if arbitrating {
                self.conflict_row(p, op)
            } else {
                op_row(p, op, compact, None)
            });
        }
        if preview.ops.is_empty() && !preview.loading {
            list = list.push(
                container(ui::muted(p, "Aucune opération pour ce filtre."))
                    .padding(18)
                    .center_x(Length::Fill),
            );
        }
        let more = (preview.ops.len() < preview.total).then(|| {
            row![
                ui::caption(
                    p,
                    format!(
                        "{} sur {} affichées",
                        model::count(preview.ops.len() as u64),
                        model::count(preview.total as u64)
                    )
                ),
                ui::fill(),
                ui::action(
                    p,
                    Tone::Secondary,
                    None,
                    if preview.loading {
                        "Chargement…"
                    } else {
                        "Afficher 100 de plus"
                    },
                    (!preview.loading).then_some(Message::PlanMore)
                ),
            ]
            .align_y(Alignment::Center)
        });
        col = col.push(toolbar).push(
            container(list.push(more))
                .padding(8)
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false)),
        );
        container(col).width(Length::Fill).into()
    }

    /// Entry point and « apply » bar of the conflict arbitration.
    fn conflict_bar<'a>(&'a self, p: &Palette, profile: &'a Profile) -> El<'a, Message> {
        let chosen = self.conflict_choices.len();
        // Replacing for good needs the trash; « Garder les deux » does not.
        let needs_trash = self
            .conflict_choices
            .values()
            .any(|keep| *keep != ConflictKeep::Both);
        let trash_ok = profile.trash.enabled || !needs_trash;
        let mut line = row![
            ui::muted(
                p,
                if trash_ok {
                    "Choisissez ce qu’il faut garder pour chaque fichier. Une version remplacée va dans la corbeille du profil ; « Garder les deux » la conserve à côté sous un autre nom."
                } else {
                    "Activez la corbeille du profil (Modifier › Sécurité) pour remplacer une version, ou choisissez « Garder les deux »."
                }
            ),
            ui::fill(),
            ui::action(
                p,
                Tone::Secondary,
                Some(Glyph::Eye),
                "Voir les conflits",
                Some(Message::PlanFilter(Some(Action::Conflict))),
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        if chosen > 0 {
            line = line.push(ui::action(
                p,
                Tone::Primary,
                Some(Glyph::Check),
                format!("Appliquer {chosen} choix (nouvelle simulation)"),
                (trash_ok && !self.busy()).then_some(Message::ResolveConflicts),
            ));
        }
        container(line)
            .padding(Padding::from([4, 4]))
            .width(Length::Fill)
            .into()
    }

    /// One conflicting file: both contents side by side and the two choices.
    fn conflict_row<'a>(&'a self, p: &Palette, op: &'a OpView) -> El<'a, Message> {
        let choice = self.conflict_choices.get(&op.path).copied();
        let side = |label: &'static str,
                    badge: &'static str,
                    tint: Color,
                    obs: &Option<crate::model::Observation>|
         -> El<'a, Message> {
            let detail = match obs {
                Some(o) => format!(
                    "{} · {}",
                    o.size.map_or("—".into(), model::bytes),
                    o.modified.map_or("date inconnue".into(), model::utc_label)
                ),
                None => "absent".into(),
            };
            column![
                row![
                    ui::chip(theme::alpha(tint, 0.16), tint, badge),
                    ui::strong(p.text, label, 14.0),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                ui::mono(p.muted, detail, 12.0),
            ]
            .spacing(6)
            .width(Length::Fill)
            .into()
        };
        let pick = |label: &'static str, keep: ConflictKeep| -> El<'a, Message> {
            let on = choice == Some(keep);
            button(ui::colored(if on { p.bg } else { p.muted }, label, 12.0))
                .padding(Padding::from([5, 14]))
                .style(theme::pick_chip(*p, on, true))
                .on_press(Message::ConflictChoose(op.path.clone(), keep))
                .into()
        };
        container(
            column![
                row![
                    ui::mono(p.text, model::ellipsize(&op.path, 90), 13.0),
                    ui::fill(),
                    match choice {
                        Some(ConflictKeep::Source) => {
                            ui::chip(theme::alpha(p.backup, 0.22), p.backup_ink, "A gardé")
                        }
                        Some(ConflictKeep::Destination) => {
                            ui::chip(theme::alpha(p.mirror, 0.2), p.mirror_ink, "B gardé")
                        }
                        Some(ConflictKeep::Both) => {
                            ui::chip(theme::alpha(p.violet, 0.16), p.info, "Les deux gardés")
                        }
                        None => ui::chip(theme::alpha(p.warning, 0.22), p.warning, "à décider"),
                    },
                ]
                .align_y(Alignment::Center),
                row![
                    side("Source", "A", p.violet, &op.sender),
                    side("Destination", "B", p.mirror, &op.receiver)
                ]
                .spacing(16),
                row![
                    pick("Garder la source", ConflictKeep::Source),
                    pick("Garder la destination", ConflictKeep::Destination),
                    pick("Garder les deux", ConflictKeep::Both),
                ]
                .spacing(8),
            ]
            .spacing(8),
        )
        .padding(Padding::from([10, 12]))
        .width(Length::Fill)
        .style(theme::row_card(*p, 0.0, choice.is_some()))
        .into()
    }

    fn profile_footer<'a>(&'a self, p: &Palette, profile: &'a Profile) -> El<'a, Message> {
        let now = model::now_secs();
        let mut runs = column![
            row![
                ui::heading(p, "Dernières exécutions"),
                ui::fill(),
                button(ui::strong(p.info, "Tout voir ›", 13.0))
                    .padding(Padding::from([4, 8]))
                    .style(theme::btn(*p, Tone::Ghost))
                    .on_press(Message::ShowHistory(profile.id.clone()))
            ]
            .padding(Padding::from([6, 10]))
            .align_y(Alignment::Center)
        ]
        .spacing(2);
        let list = self
            .ws
            .runs
            .get(&profile.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for run in list.iter().take(5) {
            let dot = match run.outcome {
                model::Outcome::Success => p.backup,
                model::Outcome::Warning => p.warning,
                _ => p.danger,
            };
            runs = runs.push(
                container(
                    row![
                        ui::dot(dot, 10.0),
                        column![
                            ui::strong(p.text, model::ellipsize(&run.label, 48), 14.0),
                            ui::colored(
                                p.muted,
                                run.started.map_or("—".into(), |s| format!(
                                    "{} · {}",
                                    model::relative(s, now),
                                    model::utc_label(s)
                                )),
                                12.0
                            ),
                        ]
                        .spacing(1)
                        .width(Length::Fill),
                        crate::pages::run_badges(p, run),
                    ]
                    .spacing(14)
                    .align_y(Alignment::Center),
                )
                .padding(10),
            );
        }
        if list.is_empty() {
            runs = runs.push(container(ui::muted(p, "Jamais exécuté.")).padding(10));
        }
        let schedule: El<'a, Message> = match self.schedule_of(&profile.id) {
            Some(s) => {
                let valid = self.schedule_valid(&profile.id) == Some(true);
                let next = draft::next_occurrence(&s.schedule.trigger, s.schedule.created_at, now);
                column![
                    ui::strong(p.text, draft::trigger_label(&s.schedule.trigger), 14.0),
                    if valid {
                        ui::caption(
                            p,
                            next.map_or("Déclenché par un événement de session".into(), |n| {
                                format!("Prochaine échéance {}", model::relative(n, now))
                            }),
                        )
                    } else {
                        ui::colored(
                            p.danger,
                            "La configuration a changé : renouvelez l’autorisation.",
                            12.0,
                        )
                    },
                ]
                .spacing(4)
                .into()
            }
            None => ui::muted(p, "Lancement manuel uniquement."),
        };
        let ok = |label: String| -> El<'a, Message> {
            row![
                container(ui::chip(
                    theme::alpha(p.backup, if p.is_light() { 0.22 } else { 0.16 }),
                    p.backup_ink,
                    "✓"
                ))
                .width(Length::Fixed(34.0)),
                ui::colored(p.text, label, 13.0),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        };
        let notice = |label: String| -> El<'a, Message> {
            row![
                container(ui::chip(theme::alpha(p.warning, 0.22), p.warning, "!"))
                    .width(Length::Fixed(34.0)),
                ui::colored(p.text, label, 13.0),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        };
        let filters = profile.filter.include_files().len()
            + profile.filter.exclude_files().len()
            + profile.filter.exclude_directories().len();
        let mut guards = column![ui::eyebrow(p.muted, "Garde-fous du profil")].spacing(10);
        guards = guards.push(if profile.mode == SyncMode::Backup {
            ok("Jamais de suppression (sauvegarde)".to_owned())
        } else if profile.allow_delete {
            ok("Suppressions protégées, confirmées à chaque exécution".to_owned())
        } else {
            ok("Aucune suppression autorisée".to_owned())
        });
        guards = guards.push(if profile.trash.enabled {
            ok("Fichiers remplacés conservés en corbeille".to_owned())
        } else {
            notice("Corbeille désactivée : les versions remplacées sont perdues".to_owned())
        });
        guards = guards.push(
            row![
                container(ui::tag(p, filters.to_string())).width(Length::Fixed(34.0)),
                ui::colored(p.text, "motif(s) d’exclusion et d’inclusion", 13.0),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
        row![
            container(runs)
                .padding(12)
                .width(Length::FillPortion(3))
                .style(theme::card(*p, 0.0, false)),
            column![
                container(
                    column![
                        row![
                            ui::eyebrow(p.muted, "Planification"),
                            ui::fill(),
                            ui::icon_button(
                                p,
                                Glyph::Pencil,
                                "Revoir la planification",
                                Some(Message::OpenSchedule(profile.id.clone()))
                            )
                        ]
                        .align_y(Alignment::Center),
                        schedule,
                    ]
                    .spacing(8),
                )
                .padding(20)
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false)),
                container(guards)
                    .padding(20)
                    .width(Length::Fill)
                    .style(theme::card(*p, 0.0, false)),
            ]
            .spacing(12)
            .width(Length::FillPortion(2)),
        ]
        .spacing(14)
        .into()
    }
}

/// « 1,4 s » with a French decimal comma.
fn seconds(value: f32) -> String {
    format!("{value:.1} s").replace('.', ",")
}

/// « Arbre » / « À plat » switch of the long lists (simulation, history).
pub(crate) fn mode_switch<'a>(p: &Palette, mode: ListMode) -> El<'a, Message> {
    ui::seg(
        p,
        &[(ListMode::Tree, "Arbre"), (ListMode::Flat, "À plat")],
        mode,
        Message::ListMode,
    )
}

/// One planned operation. In the folder view (`indent`), only its name is
/// shown, below its folder.
fn op_row<'a>(p: &Palette, op: &'a OpView, compact: bool, indent: Option<f32>) -> El<'a, Message> {
    let c = action_color(p, op.action);
    let size = (op.kind != "directory")
        .then(|| {
            op.bytes
                .filter(|b| *b > 0)
                .or_else(|| op.sender.as_ref().and_then(|o| o.size))
                .or_else(|| op.receiver.as_ref().and_then(|o| o.size))
        })
        .flatten();
    let shown = match indent {
        Some(_) => op.path.rsplit('/').next().unwrap_or(&op.path),
        None => op.path.as_str(),
    };
    let path = if op.kind == "directory" {
        format!("{shown}/")
    } else {
        shown.to_owned()
    };
    let (tag, ink) = match op.action {
        Action::Copy => ("+ AJOUT", p.backup_ink),
        Action::Replace => ("~ MODIF.", p.info),
        Action::Delete => ("− SUPPR.", p.danger),
        Action::Conflict => ("! CONFLIT", p.warning),
        Action::Keep => ("GARDÉ", p.muted),
        _ => ("IGNORÉ", p.muted),
    };
    let wash = match op.action {
        Action::Conflict => Some(theme::alpha(
            p.warning,
            if p.is_light() { 0.2 } else { 0.14 },
        )),
        Action::Delete => Some(theme::alpha(p.danger, 0.1)),
        _ => None,
    };
    let _ = c;
    container(
        row![
            container(ui::mono(ink, tag, 11.0)).width(Length::Fixed(84.0)),
            column![
                container(ui::mono(
                    if op.action == Action::Delete {
                        p.danger
                    } else if matches!(op.action, Action::Keep | Action::Skip) {
                        p.muted
                    } else {
                        p.text
                    },
                    model::ellipsize(&path, 90),
                    13.0
                ))
                .width(Length::Fill)
                .clip(true),
                ui::caption(p, op.reason_detail.clone())
            ]
            .spacing(1)
            .width(Length::Fill),
            container(ui::mono(
                p.muted,
                size.map_or("—".into(), model::bytes),
                12.0
            ))
            .width(Length::Fixed(90.0))
            .align_x(iced::alignment::Horizontal::Right),
            container(ui::mono(
                p.muted,
                if op.to_source {
                    "côté source"
                } else {
                    "côté destination"
                },
                12.0
            ))
            .width(Length::Fixed(130.0))
            .align_x(iced::alignment::Horizontal::Right),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: if compact { 4.0 } else { 7.0 },
        bottom: if compact { 4.0 } else { 7.0 },
        left: 14.0 + indent.unwrap_or(0.0),
        right: 14.0,
    })
    .width(Length::Fill)
    .style(move |_| iced::widget::container::Style {
        background: wash.map(Into::into),
        border: iced::Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        ..iced::widget::container::Style::default()
    })
    .into()
}

/// Cancels a job of this window, or asks the agent to stop its scheduled run.
pub(crate) fn cancel_message(job: &Job) -> Message {
    if job.agent {
        Message::CancelAgent
    } else {
        Message::Cancel
    }
}

/// Shared control for full, compact and agent views.
pub(crate) fn pause_button<'a>(p: &Palette, job: &Job) -> El<'a, Message> {
    let paused = job.snapshot.pause_requested;
    ui::action(
        p,
        Tone::Secondary,
        Some(if paused { Glyph::Play } else { Glyph::Pause }),
        if paused { "Reprendre" } else { "Pause" },
        pause_message(job),
    )
}

/// Pause or resume request, when the job can take it.
fn pause_message(job: &Job) -> Option<Message> {
    (job.snapshot.active
        && !job.cancel_requested
        && (!job.agent || job.snapshot.control_id.is_some()))
    .then(|| Message::SetPaused {
        agent: job.agent,
        control_id: job.snapshot.control_id.clone(),
        paused: !job.snapshot.pause_requested,
    })
}

/// Icon-only pause/resume for the narrow sidebar tracker.
pub(crate) fn pause_icon<'a>(p: &Palette, job: &Job) -> El<'a, Message> {
    let paused = job.snapshot.pause_requested;
    ui::icon_button(
        p,
        if paused { Glyph::Play } else { Glyph::Pause },
        if paused {
            "Reprendre"
        } else {
            "Mettre en pause"
        },
        pause_message(job),
    )
}

/// « 08:29 » (minutes and seconds elapsed, hours folded into minutes).
fn clock(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}
