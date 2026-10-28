//! Profile page: identity, endpoints with the animated flow, the simulation
//! (plan inspection) and execution panels, recent results and planning.

use crate::app::{App, Job, JobKind, Message};
use crate::charts::{self, Flow, Ring};
use crate::draft::{self, Provider};
use crate::icons::{self, Glyph};
use crate::model::{self, OpView};
use crate::shell::{mode_color, mode_glyph, outcome_color};
use crate::speed;
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{Space, button, column, container, row, scrollable, text_input};
use iced::{Alignment, Color, Length, Padding};
use std::time::Instant;
use sync_plan::{Action, SyncMode};
use sync_profiles::Profile;

pub(crate) fn action_color(p: &Palette, action: Action) -> Color {
    match action {
        Action::Copy => p.success,
        Action::Replace => p.accent,
        Action::Delete => p.danger,
        Action::Conflict => p.warning,
        Action::Keep => p.faint,
        _ => p.muted,
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
        let crumbs = row![
            button(
                row![
                    icons::icon(Glyph::Grid, 14.0, p.muted),
                    ui::colored(p.muted, "Vue d’ensemble", 13.0)
                ]
                .spacing(6)
                .align_y(Alignment::Center)
            )
            .padding(Padding::from([4, 8]))
            .style(theme::btn(*p, Tone::Ghost))
            .on_press(Message::Select(None)),
            icons::icon(Glyph::ChevronRight, 12.0, p.faint),
            ui::colored(p.muted, group.to_owned(), 13.0),
            ui::fill(),
            ui::icon_button(
                p,
                Glyph::Pencil,
                "Modifier  ·  Ctrl+E",
                (!busy).then(|| Message::Edit(id.clone()))
            ),
            ui::icon_button(
                p,
                Glyph::Clock,
                "Planifier…",
                Some(Message::OpenSchedule(id.clone()))
            ),
            ui::icon_button(
                p,
                Glyph::Copy,
                "Dupliquer",
                Some(Message::Duplicate(id.clone()))
            ),
            ui::icon_button(
                p,
                Glyph::History,
                "Historique",
                Some(Message::ShowHistory(id.clone()))
            ),
            ui::icon_button(
                p,
                Glyph::Trash,
                "Retirer  ·  Suppr",
                (!busy).then(|| Message::AskRemove(id.clone()))
            ),
        ]
        .spacing(4)
        .align_y(Alignment::Center);

        let color = mode_color(p, profile.mode);
        let mut pills = row![
            ui::pill(color, model::mode_label(profile.mode)),
            ui::pill(p.muted, model::compare_label(profile.compare)),
        ]
        .spacing(8);
        match (self.schedule_of(id), self.schedule_valid(id)) {
            (Some(s), Some(true)) => {
                pills = pills.push(ui::pill(p.info, draft::trigger_label(&s.schedule.trigger)))
            }
            (Some(_), _) => pills = pills.push(ui::pill(p.danger, "Planification à renouveler")),
            _ => {}
        }
        if profile.allow_delete {
            pills = pills.push(ui::pill(p.danger, "Suppressions autorisées"));
        }
        if let Some(run) = self.last_run(id) {
            pills = pills.push(ui::pill(outcome_color(p, run.outcome), run.label.clone()));
        }
        let main_action: El<'a, Message> = match running {
            Some(job) => row![
                pause_button(p, job),
                ui::action(
                    p,
                    Tone::Secondary,
                    Some(Glyph::Stop),
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
            None => ui::action(
                p,
                Tone::Primary,
                Some(Glyph::Eye),
                "Simuler",
                (!busy).then(|| Message::Simulate(id.clone())),
            ),
        };
        let heading = row![
            ui::glyph_box(mode_glyph(profile.mode), color, 60.0),
            container(ui::title(p, profile.name.clone(), 26.0)).width(Length::Fill),
            main_action,
        ]
        .spacing(18)
        .align_y(Alignment::Center);
        let hero = column![heading, pills.wrap()].spacing(12);

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
                ui::glyph_box(provider.glyph(), p.accent, 42.0),
                // Clipped: a long unwrapped path never runs under the button.
                container(
                    column![
                        ui::eyebrow(p.faint, label),
                        ui::mono(p.text, model::ellipsize(&location, 44), 13.0),
                        ui::caption(p, model::provider_label(remote.as_ref())),
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
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fixed(96.0))
        .center_y(Length::Fixed(96.0))
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
        .style(theme::outline(*p, p.accent, 0.5))
        .into()
    }

    /// Live job card (used on the profile page and the activity page), for a
    /// job of this window or a scheduled run of the agent.
    pub(crate) fn live_panel<'a>(
        &'a self,
        p: &Palette,
        job: &'a Job,
        large: bool,
    ) -> El<'a, Message> {
        let s = &job.snapshot;
        let elapsed = self.now.saturating_duration_since(job.started);
        let turn = elapsed.as_secs_f32();
        let (title, color) = if s.pause_requested {
            ("Pause demandée — Reprendre pour continuer", p.warning)
        } else {
            match job.kind {
                JobKind::Preview if job.agent => ("Synchronisation planifiée : analyse", p.accent),
                JobKind::Execute if job.agent => {
                    ("Synchronisation planifiée : écritures en cours", p.success)
                }
                JobKind::Preview if s.hash_target > 0 => {
                    ("Simulation : comparaison du contenu", p.accent)
                }
                JobKind::Preview if s.entries_hashed > 0 => {
                    ("Simulation : lecture et empreintes", p.accent)
                }
                JobKind::Preview => ("Simulation : lecture des dossiers", p.accent),
                JobKind::Execute => ("Exécution réelle en cours", p.success),
            }
        };
        let progress = job.progress();
        let percent = progress.map(model::percent);
        // Says what the percentage measures: the volume, not the file count
        // shown next to it.
        let basis = match (job.kind, progress) {
            (JobKind::Execute, Some(_)) => Some("du volume"),
            (JobKind::Preview, Some(_)) => Some("des empreintes"),
            (_, None) => None,
        };
        let mut center = column![ui::title(
            p,
            percent.unwrap_or_else(|| format!("{:.0} s", elapsed.as_secs_f32())),
            18.0
        )]
        .align_x(Alignment::Center);
        if let Some(basis) = basis {
            center = center.push(ui::caption(p, basis));
        }
        let ring = iced::widget::stack![
            charts::ring(
                Ring {
                    progress,
                    phase: turn * 0.7,
                    color,
                    track: theme::alpha(color, 0.18),
                    width: 7.0
                },
                104.0
            ),
            container(center).center(Length::Fixed(104.0)),
        ];
        // Throughput over a 4 s sliding window (bytes/s, or entries/s while
        // scanning), as FreeFileSync: see `speed`.
        let series = |count: fn(&crate::app::Sample) -> u64| -> Vec<f32> {
            let counts: Vec<(Instant, u64)> =
                job.samples.iter().map(|s| (s.at, count(s))).collect();
            speed::rates(&counts, job.started)
        };
        let values = match job.kind {
            JobKind::Execute => series(|s| s.bytes),
            JobKind::Preview => series(|s| s.scanned),
        };
        // Many small items (folders, thumbnails) move few bytes: show the
        // operation rate next to the byte rate, as FreeFileSync does.
        let operations = match job.kind {
            JobKind::Execute => series(|s| s.operations).last().copied(),
            JobKind::Preview => None,
        };
        let rate_label = match (values.last(), job.kind) {
            // Not estimated during the first second: no misleading 0.
            (None, _) => "mesure…".to_owned(),
            (Some(rate), JobKind::Execute) => format!("{}/s", model::bytes(*rate as u64)),
            (Some(rate), JobKind::Preview) => {
                format!("{} éléments/s", model::count(*rate as u64))
            }
        };
        let stat = |label: &'static str, value: String, c: Color| -> El<'a, Message> {
            column![ui::title(p, value, 20.0), ui::colored(c, label, 12.0)]
                .spacing(2)
                .width(Length::Fill)
                .into()
        };
        let stats = match job.kind {
            JobKind::Preview => row![
                stat(
                    "entrées analysées (2 côtés)",
                    model::count(s.entries_scanned),
                    p.muted
                ),
                stat(
                    "empreintes calculées",
                    if s.hash_target > 0 {
                        format!(
                            "{} / {}",
                            model::count(s.entries_hashed),
                            model::count(s.hash_target)
                        )
                    } else {
                        model::count(s.entries_hashed)
                    },
                    p.muted
                ),
                stat(
                    "erreurs de lecture",
                    model::count(s.scan_issues),
                    if s.scan_issues > 0 {
                        p.warning
                    } else {
                        p.muted
                    }
                ),
                stat(
                    "dossiers à cohérence incertaine",
                    model::count(s.scan_consistency_warnings),
                    if s.scan_consistency_warnings > 0 {
                        p.warning
                    } else {
                        p.muted
                    },
                ),
                stat("débit d’analyse", rate_label, p.muted),
            ],
            JobKind::Execute => row![
                stat(
                    "opérations terminées",
                    format!(
                        "{} / {}",
                        model::count(s.entries_done),
                        model::count(job.planned.unwrap_or(0))
                    ),
                    p.muted
                ),
                stat(
                    "volume validé",
                    match job.planned_bytes.filter(|b| *b > 0) {
                        Some(total) =>
                            format!("{} / {}", model::bytes(s.bytes_done), model::bytes(total)),
                        None => model::bytes(s.bytes_done),
                    },
                    p.muted
                ),
                stat(
                    "refus ou échecs",
                    model::count(s.operation_errors),
                    if s.operation_errors > 0 {
                        p.danger
                    } else {
                        p.muted
                    }
                ),
                column![
                    ui::title(p, rate_label, 20.0),
                    ui::colored(
                        p.muted,
                        operations.map_or_else(
                            || "débit".to_owned(),
                            |rate| format!(
                                "débit · {} opérations/s",
                                model::count(rate.round() as u64)
                            )
                        ),
                        12.0
                    )
                ]
                .spacing(2)
                .width(Length::Fill),
            ],
        }
        .spacing(16);
        let mut body = column![
            row![
                ui::eyebrow(
                    color,
                    match (job.agent, job.kind) {
                        (true, JobKind::Preview) =>
                            "Lancée par l’agent — analyse avant écritures, rien n’est encore modifié",
                        (true, JobKind::Execute) => "Lancée par l’agent — écritures réelles",
                        (false, JobKind::Preview) => "Simulation — aucun fichier modifié",
                        (false, JobKind::Execute) => "Écritures réelles",
                    }
                ),
                ui::fill(),
                ui::caption(p, format!("{} · {:.0} s", job.name, elapsed.as_secs_f32())),
            ],
            ui::title(p, title, 19.0),
            stats,
        ]
        .spacing(10)
        .width(Length::Fill);
        if job.kind == JobKind::Execute {
            body=body.push(ui::caption(p,format!(
                "En cours : {} transférés · {} relus pour vérification · {} conservés en versions. Le volume validé avance après publication complète de chaque fichier.",
                model::bytes(s.bytes_transferred),model::bytes(s.bytes_verified),model::bytes(s.bytes_versioned)
            )));
        }
        if s.scan_consistency_warnings > 0 {
            body = body.push(ui::caption(p,
                "Le partage ne garantit pas la cohérence de ses listes : ce compteur ne signifie pas que la lecture a échoué. Les absences restent incertaines et ne permettent pas de suppression miroir."
            ));
        }
        if large {
            body = body.push(charts::spark(
                values,
                color,
                theme::alpha(p.text, 0.06),
                70.0,
            ));
        }
        body = body.push(
            row![
                // Fill: the explanation wraps instead of squeezing the button.
                container(ui::caption(p, if job.cancel_requested { "Annulation coopérative demandée : l’opération en cours se termine proprement." } else { "Vous pouvez annuler à tout moment ; l’état partiel est conservé et journalisé." }))
                    .width(Length::Fill),
                pause_button(p, job),
                ui::action(p, Tone::Secondary, Some(Glyph::Stop), "Annuler", (!job.cancel_requested).then_some(cancel_message(job))),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        );
        if job.agent {
            body = body.push(row![
                container(ui::caption(p, "Arrêter l’agent annule le transfert et suspend les jobs jusqu’à « Reprendre les planifications ». Aucun processus n’est tué de force.")).width(Length::Fill),
                ui::action(p, Tone::Secondary, Some(Glyph::Power), "Arrêter l’agent", (!self.ws.agent_stopped).then_some(Message::StopAgent)),
            ].spacing(12).align_y(Alignment::Center));
        }
        container(row![ring, body].spacing(24).align_y(Alignment::Center))
            .padding(22)
            .width(Length::Fill)
            .style(theme::outline(*p, color, 0.6))
            .into()
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
                        ui::colored(p.muted, model::action_label(a), 12.0)
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    ui::title(p, model::count(shown), 24.0),
                ]
                .spacing(4),
            )
            .padding(Padding::from([12, 14]))
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
                ui::glyph_box(Glyph::Eye, p.info, 30.0),
                // The label yields (wraps) first in a narrow window.
                container(ui::eyebrow(
                    p.info,
                    "Simulation — aucun fichier n’a été modifié"
                ))
                .width(Length::Fill),
                ui::caption(p, format!("{} à transférer", model::bytes(s.bytes))),
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
        if s.conflict > 0 {
            col = col.push(ui::callout(
                p,
                p.warning,
                Glyph::TwoWay,
                &format!("{} conflit(s) préservé(s)", model::count(s.conflict)),
                "Rien n’est écrit sur un conflit non résolu : les deux versions restent en place.",
            ));
        }

        let filter_chip = |label: &'static str, a: Option<Action>| -> El<'a, Message> {
            let on = preview.filter == a;
            button(ui::colored(if on { p.text } else { p.muted }, label, 12.0))
                .padding(Padding::from([4, 12]))
                .style(theme::chip(
                    *p,
                    a.map_or(p.accent, |a| action_color(p, a)),
                    on,
                ))
                .on_press(Message::PlanFilter(a))
                .into()
        };
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
                    .padding(Padding::from([8, 12]))
                    .size(13)
                    .style(theme::input(*p, false))
            )
            .width(Length::FillPortion(3))
            .max_width(320.0),
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
                Some(Glyph::Refresh),
                "Recalculer",
                (!self.busy()).then(|| Message::Simulate(profile.id.clone()))
            ),
            ui::action(
                p,
                execute_tone,
                Some(Glyph::Bolt),
                if s.mutations() == 0 {
                    "Rien à appliquer"
                } else {
                    "Exécuter ce plan…"
                },
                (s.mutations() > 0 && !self.busy()).then_some(Message::AskExecute)
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let filters = row![
            filter_chip("Tout", None),
            filter_chip("Copies", Some(Action::Copy)),
            filter_chip("Remplacements", Some(Action::Replace)),
            filter_chip("Suppressions", Some(Action::Delete)),
            filter_chip("Conflits", Some(Action::Conflict)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .wrap()
        .vertical_spacing(8);
        let toolbar = column![actions, filters].spacing(10);

        let mut list = column![
            row![
                container(ui::eyebrow(p.faint, "Action")).width(Length::Fixed(110.0)),
                container(ui::eyebrow(p.faint, "Chemin et motif")).width(Length::Fill),
                container(ui::eyebrow(p.faint, "Taille")).width(Length::Fixed(90.0)),
                container(ui::eyebrow(p.faint, "Cible")).width(Length::Fixed(100.0)),
            ]
            .padding(Padding::from([4, 12])),
        ]
        .spacing(2);
        for op in &preview.ops {
            list = list.push(op_row(p, op, self.ws.preferences.compact));
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

    fn profile_footer<'a>(&'a self, p: &Palette, profile: &'a Profile) -> El<'a, Message> {
        let now = model::now_secs();
        let mut runs = column![
            row![
                ui::heading(p, "Dernières exécutions"),
                ui::fill(),
                button(ui::colored(p.accent, "Tout voir", 13.0))
                    .style(theme::btn(*p, Tone::Ghost))
                    .on_press(Message::ShowHistory(profile.id.clone()))
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(8);
        let list = self
            .ws
            .runs
            .get(&profile.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for run in list.iter().take(5) {
            runs = runs.push(
                row![
                    ui::dot(outcome_color(p, run.outcome), 9.0),
                    ui::colored(p.text, run.label.clone(), 13.0),
                    ui::fill(),
                    ui::caption(
                        p,
                        run.started.map_or("—".into(), |s| model::relative(s, now))
                    ),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            );
        }
        if list.is_empty() {
            runs = runs.push(ui::muted(p, "Jamais exécuté."));
        }
        let schedule: El<'a, Message> = match self.schedule_of(&profile.id) {
            Some(s) => {
                let valid = self.schedule_valid(&profile.id) == Some(true);
                let next = draft::next_occurrence(&s.schedule.trigger, s.schedule.created_at, now);
                column![
                    ui::muted(p, draft::trigger_label(&s.schedule.trigger)),
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
        let filters = profile.filter.include_files().len()
            + profile.filter.exclude_files().len()
            + profile.filter.exclude_directories().len();
        row![
            container(runs)
                .padding(18)
                .width(Length::FillPortion(3))
                .style(theme::card(*p, 0.0, false)),
            container(
                column![
                    row![
                        ui::heading(p, "Planification"),
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
                    ui::divider(p),
                    ui::caption(
                        p,
                        format!("{filters} motif(s) de filtre · comparaison BLAKE3")
                    ),
                    ui::caption(
                        p,
                        if profile.allow_delete {
                            "Suppressions protégées autorisées"
                        } else {
                            "Aucune suppression"
                        }
                    ),
                ]
                .spacing(8),
            )
            .padding(18)
            .width(Length::FillPortion(2))
            .style(theme::card(*p, 0.0, false)),
        ]
        .spacing(14)
        .into()
    }
}

/// « 1,4 s » with a French decimal comma.
fn seconds(value: f32) -> String {
    format!("{value:.1} s").replace('.', ",")
}

fn op_row<'a>(p: &Palette, op: &'a OpView, compact: bool) -> El<'a, Message> {
    let c = action_color(p, op.action);
    let size = (op.kind != "directory")
        .then(|| {
            op.bytes
                .filter(|b| *b > 0)
                .or_else(|| op.sender.as_ref().and_then(|o| o.size))
                .or_else(|| op.receiver.as_ref().and_then(|o| o.size))
        })
        .flatten();
    let path = if op.kind == "directory" {
        format!("{}/", op.path)
    } else {
        op.path.clone()
    };
    container(
        row![
            container(ui::badge(
                theme::alpha(c, 0.16),
                c,
                model::action_label(op.action)
            ))
            .width(Length::Fixed(110.0)),
            column![
                ui::mono(p.text, model::ellipsize(&path, 90), 13.0),
                ui::caption(p, op.reason_detail.clone())
            ]
            .spacing(2)
            .width(Length::Fill),
            container(ui::caption(p, size.map_or("—".into(), model::bytes)))
                .width(Length::Fixed(90.0)),
            container(ui::caption(
                p,
                if op.to_source {
                    "côté source"
                } else {
                    "côté destination"
                }
            ))
            .width(Length::Fixed(100.0)),
        ]
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([if compact { 4 } else { 8 }, 12]))
    .width(Length::Fill)
    .style(move |_| iced::widget::container::Style {
        border: iced::Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: theme::RADIUS_SM.into(),
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
        (job.snapshot.active
            && !job.cancel_requested
            && (!job.agent || job.snapshot.control_id.is_some()))
        .then(|| Message::SetPaused {
            agent: job.agent,
            control_id: job.snapshot.control_id.clone(),
            paused: !paused,
        }),
    )
}
