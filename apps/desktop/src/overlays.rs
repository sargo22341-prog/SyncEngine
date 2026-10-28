//! Floating layers: context menus, modal dialogs and toast notifications.

use crate::app::{App, Dialog, MenuTarget, Message, ToastKind};
use crate::icons::{self, Glyph};
use crate::model;
use crate::schedules;
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{
    Space, button, checkbox, column, container, mouse_area, opaque, pin, row, stack, text_input,
};
use iced::{Alignment, Length, Padding};

const MENU_WIDTH: f32 = 236.0;

impl App {
    pub(crate) fn menu_layer(&self, p: &Palette) -> Option<El<'_, Message>> {
        let menu = self.menu.as_ref()?;
        let t = menu.enter.at(self.now);
        let mp = p.faded(t);
        let item = |glyph: Glyph,
                    label: &str,
                    hint: Option<&str>,
                    msg: Option<Message>,
                    danger: bool|
         -> El<'_, Message> {
            let enabled = msg.is_some();
            let color = if !enabled {
                mp.faint
            } else if danger {
                mp.danger
            } else {
                mp.text
            };
            let mut r = row![
                icons::icon(glyph, 15.0, color),
                ui::colored(color, label.to_owned(), 13.0),
                ui::fill()
            ]
            .spacing(10)
            .align_y(Alignment::Center);
            if let Some(h) = hint {
                r = r.push(ui::caption(&mp, h.to_owned()));
            }
            button(r)
                .padding(Padding::from([7, 10]))
                .width(Length::Fill)
                .style(theme::menu_item(mp, danger))
                .on_press_maybe(msg.map(|m| Message::Menu(Box::new(m))))
                .into()
        };
        let sep = || container(ui::divider(&mp)).padding(Padding::from([4, 6]));
        let busy = self.busy();
        let mut col = column![].spacing(1);
        let trash_item = |id: &sync_core::ProfileId| {
            item(
                Glyph::Trash,
                "Voir la corbeille…",
                None,
                Some(Message::Trash(crate::trash::TrashMsg::Open(id.clone()))),
                false,
            )
        };
        let mut rows = 0usize;
        match &menu.target {
            MenuTarget::Profile(id) => {
                let profile = self.profile(id)?;
                col = col
                    .push(
                        container(ui::eyebrow(mp.faint, &model::ellipsize(&profile.name, 30)))
                            .padding(Padding::from([6, 10])),
                    )
                    .push(item(
                        Glyph::Eye,
                        "Simuler",
                        Some("F5"),
                        (!busy).then(|| Message::Simulate(id.clone())),
                        false,
                    ))
                    .push(item(
                        Glyph::Pencil,
                        "Modifier…",
                        Some("Ctrl+E"),
                        Some(Message::Edit(id.clone())),
                        false,
                    ))
                    .push(item(
                        Glyph::Clock,
                        "Revoir la planification…",
                        None,
                        Some(Message::OpenSchedule(id.clone())),
                        false,
                    ))
                    .push(item(
                        Glyph::History,
                        "Voir l’historique",
                        None,
                        Some(Message::ShowHistory(id.clone())),
                        false,
                    ))
                    .push(trash_item(id))
                    .push(item(
                        Glyph::Copy,
                        "Dupliquer…",
                        None,
                        Some(Message::Duplicate(id.clone())),
                        false,
                    ))
                    .push(sep());
                rows += 8;
                let others: Vec<String> = self
                    .groups()
                    .into_iter()
                    .filter(|g| *g != profile.group)
                    .take(6)
                    .collect();
                if !others.is_empty() {
                    col = col.push(
                        container(ui::eyebrow(mp.faint, "Déplacer vers"))
                            .padding(Padding::from([4, 10])),
                    );
                    for g in others {
                        let label = if g.is_empty() {
                            "Sans groupe".to_owned()
                        } else {
                            g.clone()
                        };
                        col = col.push(item(
                            Glyph::Layers,
                            &label,
                            None,
                            (!busy).then(|| Message::MoveTo(id.clone(), g.clone())),
                            false,
                        ));
                        rows += 1;
                    }
                    rows += 1;
                }
                col = col
                    .push(item(
                        Glyph::Plus,
                        "Nouveau groupe…",
                        None,
                        (!busy).then(|| Message::NewGroup(Some(id.clone()))),
                        false,
                    ))
                    .push(sep())
                    .push(item(
                        Glyph::Trash,
                        "Retirer le profil…",
                        Some("Suppr"),
                        (!busy).then(|| Message::AskRemove(id.clone())),
                        true,
                    ));
                rows += 3;
            }
            MenuTarget::Group(g) => {
                let label = if g.is_empty() {
                    "Sans groupe"
                } else {
                    g.as_str()
                };
                col = col
                    .push(container(ui::eyebrow(mp.faint, label)).padding(Padding::from([6, 10])))
                    .push(item(
                        Glyph::Plus,
                        "Nouveau profil ici",
                        None,
                        Some(Message::New(Some(g.clone()))),
                        false,
                    ))
                    .push(item(
                        Glyph::Clock,
                        "Planifier le groupe…",
                        None,
                        Some(Message::ScheduleGroup(g.clone())),
                        false,
                    ))
                    .push(item(
                        Glyph::Pencil,
                        "Renommer…",
                        None,
                        (!g.is_empty() && !busy).then(|| Message::RenameGroup(g.clone())),
                        false,
                    ))
                    .push(item(
                        if self.prefs.collapsed.contains(g) {
                            Glyph::ChevronDown
                        } else {
                            Glyph::ChevronRight
                        },
                        if self.prefs.collapsed.contains(g) {
                            "Déplier"
                        } else {
                            "Replier"
                        },
                        None,
                        Some(Message::ToggleGroup(g.clone())),
                        false,
                    ));
                rows += 5;
            }
            MenuTarget::Background => {
                col = col
                    .push(item(
                        Glyph::Plus,
                        "Nouveau profil",
                        Some("Ctrl+N"),
                        Some(Message::New(None)),
                        false,
                    ))
                    .push(item(
                        Glyph::Layers,
                        "Nouveau groupe…",
                        None,
                        Some(Message::NewGroup(None)),
                        false,
                    ))
                    .push(sep())
                    .push(item(
                        Glyph::Refresh,
                        "Actualiser",
                        Some("Ctrl+R"),
                        Some(Message::Refresh),
                        false,
                    ));
                rows += 4;
            }
        }
        let height = rows as f32 * 32.0 + 16.0;
        let x = menu.at.x.min(self.window.width - MENU_WIDTH - 8.0).max(4.0);
        let y = if menu.at.y + height > self.window.height - 8.0 {
            (menu.at.y - height).max(4.0)
        } else {
            menu.at.y
        };
        let panel = container(col)
            .padding(6)
            .width(Length::Fixed(MENU_WIDTH))
            .style(theme::menu_surface(mp));
        Some(
            stack![
                opaque(
                    mouse_area(
                        container(Space::new())
                            .width(Length::Fill)
                            .height(Length::Fill)
                    )
                    .on_press(Message::CloseMenu)
                    .on_right_press(Message::CloseMenu)
                ),
                pin(opaque(panel))
                    .x(x)
                    .y(y - 6.0 * (1.0 - t))
                    .width(Length::Fill)
                    .height(Length::Fill),
            ]
            .into(),
        )
    }

    pub(crate) fn dialog_layer(&self, p: &Palette) -> Option<El<'_, Message>> {
        let dialog = self.dialog.as_ref()?;
        let t = self.dialog_t.at(self.now);
        let dp = p.faded(t);
        let (width, body): (f32, El<'_, Message>) = match dialog {
            Dialog::Execute {
                profile,
                confirm_delete,
                confirm_permanent,
            } => (
                560.0,
                self.execute_dialog(&dp, profile, *confirm_delete, *confirm_permanent),
            ),
            Dialog::Trash(trash) => (720.0, self.trash_dialog(&dp, trash)),
            Dialog::Remove { profile } => {
                let name = self
                    .profile(profile)
                    .map_or(String::new(), |pr| pr.name.clone());
                let scheduled = self.schedule_of(profile).is_some();
                let interrupted = self.schedule_of(profile).is_some_and(|s| s.running_slot.is_some())
                    || self.last_run(profile).is_some_and(|run| run.outcome == crate::model::Outcome::Interrupted);
                (
                    480.0,
                    column![
                        dialog_head(&dp, Glyph::Trash, dp.danger, &format!("Retirer « {name} » ?")),
                        ui::muted(&dp, "La configuration et la corbeille du profil (anciennes versions des fichiers remplacés ou supprimés) sont supprimées définitivement. Vos fichiers source et destination restent intacts."),
                        if interrupted {
                            ui::callout(&dp, dp.warning, Glyph::Alert, "Diagnostic requis", "Une exécution est encore marquée en cours ou interrompue. Si elle est arrêtée, vérifiez son résultat et confirmez le diagnostic avant de retirer ce profil.")
                        } else if scheduled {
                            ui::callout(&dp, dp.warning, Glyph::Clock, "Planification associée", "Son autorisation sera retirée d’abord.")
                        } else {
                            Space::new().into()
                        },
                        row![
                            ui::fill(),
                            ui::action(&dp, Tone::Ghost, None, "Annuler", Some(Message::CloseDialog)),
                            if interrupted {
                                ui::action(&dp, Tone::Secondary, Some(Glyph::Check), "Diagnostiquer", (!self.running_on(profile)).then(|| Message::AskDiagnose(profile.clone())))
                            } else {
                                ui::action(&dp, Tone::Danger, Some(Glyph::Trash), "Retirer", Some(Message::Remove(profile.clone())))
                            },
                        ]
                        .spacing(10),
                    ]
                    .spacing(16)
                    .into(),
                )
            }
            Dialog::Diagnose { profile } => (
                560.0,
                column![
                    dialog_head(&dp, Glyph::Alert, dp.warning, "Confirmer le diagnostic de l’interruption"),
                    ui::muted(&dp, "Vérifiez que le transfert est arrêté, puis examinez les fichiers et l’historique. Des copies partielles ou des versions temporaires peuvent subsister après un arrêt forcé."),
                    ui::muted(&dp, "Votre confirmation marque l’ancienne exécution en échec et lève son blocage. Les fichiers, journaux et la corbeille sont conservés. L’ancienne échéance n’est pas rejouée ; les prochaines échéances autorisées pourront s’exécuter après une nouvelle analyse."),
                    row![
                        ui::fill(),
                        ui::action(&dp, Tone::Ghost, None, "Annuler", Some(Message::CloseDialog)),
                        ui::action(&dp, Tone::Primary, Some(Glyph::Check), "J’ai vérifié : lever le blocage", (!self.running_on(profile)).then(|| Message::Diagnose(profile.clone()))),
                    ].spacing(10),
                ].spacing(16).into(),
            ),
            Dialog::Group {
                original,
                name,
                pending,
                error,
            } => {
                let title = match (original, pending) {
                    (Some(_), _) => "Renommer le groupe",
                    (None, Some(_)) => "Déplacer dans un nouveau groupe",
                    (None, None) => "Nouveau groupe",
                };
                let help = if original.is_some() {
                    "Les profils du groupe le suivent ; leurs planifications sont renouvelées avec les mêmes réglages."
                } else {
                    "Glissez ensuite des profils dessus, ou choisissez-le à la fin de l’assistant."
                };
                (
                    460.0,
                    column![
                        dialog_head(&dp, Glyph::Layers, dp.accent, title),
                        column![
                            text_input("Nom du groupe", name)
                                .on_input(Message::GroupName)
                                .on_submit(Message::SubmitGroup)
                                .padding(Padding::from([10, 12]))
                                .size(15)
                                .style(theme::input(dp, error.is_some())),
                            match error {
                                Some(e) => ui::colored(dp.danger, e.clone(), 12.0),
                                None => ui::caption(&dp, help),
                            },
                        ]
                        .spacing(6),
                        row![
                            ui::fill(),
                            ui::action(
                                &dp,
                                Tone::Ghost,
                                None,
                                "Annuler",
                                Some(Message::CloseDialog)
                            ),
                            ui::action(
                                &dp,
                                Tone::Primary,
                                Some(Glyph::Check),
                                "Valider",
                                Some(Message::SubmitGroup)
                            ),
                        ]
                        .spacing(10),
                    ]
                    .spacing(16)
                    .into(),
                )
            }
            Dialog::Schedule {
                targets,
                title,
                draft,
                error,
                saving,
            } => {
                let deletions = targets
                    .iter()
                    .any(|id| self.profile(id).is_some_and(|p| p.allow_delete));
                (
                    760.0,
                    column![
                        dialog_head(&dp, Glyph::Clock, dp.accent, title),
                        if targets.len() > 1 {
                            ui::muted(&dp, format!("Les mêmes réglages seront appliqués aux {} profils du groupe, l’un après l’autre.", targets.len()))
                        } else {
                            Space::new().into()
                        },
                        iced::widget::scrollable(schedules::editor(&dp, draft, deletions, self.ws.preferences.compact).map(Message::ScheduleEdit))
                            .style(theme::scroll(dp))
                            .height(Length::Shrink),
                        match error {
                            Some(e) => row![icons::icon(Glyph::Alert, 15.0, dp.danger), ui::colored(dp.danger, e.clone(), 13.0)]
                                .spacing(8)
                                .align_y(Alignment::Center)
                                .into(),
                            None => El::from(Space::new()),
                        },
                        row![
                            ui::fill(),
                            ui::action(&dp, Tone::Ghost, None, "Annuler", Some(Message::CloseDialog)),
                            ui::action(
                                &dp,
                                Tone::Primary,
                                Some(Glyph::Check),
                                if *saving { "Enregistrement…" } else { "Enregistrer" },
                                (!saving).then_some(Message::SubmitSchedule)
                            ),
                        ]
                        .spacing(10),
                    ]
                    .spacing(16)
                    .into(),
                )
            }
            Dialog::Shortcuts => (520.0, shortcuts(&dp)),
        };
        let lift = 24.0 * (1.0 - t);
        let panel = container(body)
            .padding(24)
            .width(Length::Fixed(width))
            .max_height(self.window.height - 60.0)
            .style(theme::dialog(dp));
        Some(
            stack![
                opaque(
                    mouse_area(
                        container(Space::new())
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .style(theme::scrim(*p, t))
                    )
                    .on_press(Message::CloseDialog)
                ),
                container(opaque(panel))
                    .padding(Padding {
                        top: lift,
                        bottom: 0.0,
                        left: 0.0,
                        right: 0.0
                    })
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center(Length::Fill),
            ]
            .into(),
        )
    }

    fn execute_dialog(
        &self,
        p: &Palette,
        profile: &sync_core::ProfileId,
        confirm_delete: bool,
        confirm_permanent: bool,
    ) -> El<'_, Message> {
        let name = self
            .profile(profile)
            .map_or(String::new(), |pr| pr.name.clone());
        let Some(preview) = &self.preview else {
            return ui::muted(p, "Simulation expirée : relancez-la.");
        };
        let s = preview.view.summary;
        let line = |glyph: Glyph, color, label: String| -> El<'_, Message> {
            row![
                icons::icon(glyph, 16.0, color),
                ui::colored(p.text, label, 14.0)
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        };
        let mut col = column![
            dialog_head(p, Glyph::Bolt, p.success, &format!("Exécuter « {name} » ?")),
            ui::muted(p, "Le plan exact simulé va être appliqué. Les préconditions sont revérifiées juste avant chaque écriture ; une interruption peut laisser un état partiel, toujours consigné."),
            container(
                column![
                    line(Glyph::Plus, p.success, format!("{} copie(s)", model::count(s.copy))),
                    line(Glyph::Refresh, p.accent, format!("{} remplacement(s)", model::count(s.replace))),
                    line(Glyph::Trash, if s.trash.overflow { p.danger } else { p.muted }, if s.trash.enabled && !s.trash.overflow {
                        format!("Anciennes versions gardées dans la corbeille ({})", model::bytes(s.trash.needed))
                    } else {
                        "Aucune copie des fichiers remplacés ou supprimés".to_owned()
                    }),
                    line(Glyph::Trash, if s.delete > 0 { p.danger } else { p.faint }, format!("{} suppression(s)", model::count(s.delete))),
                    line(Glyph::Backup, p.muted, format!("{} à transférer", model::bytes(s.bytes))),
                ]
                .spacing(8),
            )
            .padding(14)
            .width(Length::Fill)
            .style(theme::outline(*p, p.border, 0.0)),
            column![
                ui::caption(p, format!("Source : {}", model::ellipsize(&preview.view.source, 80))),
                ui::caption(p, format!("Destination : {}", model::ellipsize(&preview.view.destination, 80))),
            ]
            .spacing(2),
        ]
        .spacing(14);
        if s.ceiling_lifted && s.delete > 0 {
            col = col.push(ui::callout(
                p,
                p.danger,
                Glyph::Alert,
                "Plafond de suppressions levé pour ce plan",
                "Ces suppressions dépassent le plafond habituel ; vous l’avez autorisé dans la simulation.",
            ));
        }
        if s.delete > 0 {
            col = col.push(
                container(
                    checkbox(confirm_delete)
                        .label(format!(
                            "Je confirme les {} suppression(s) affichées dans ce plan",
                            model::count(s.delete)
                        ))
                        .on_toggle(Message::ToggleExecuteDelete)
                        .text_size(13)
                        .style(theme::check(*p)),
                )
                .padding(12)
                .width(Length::Fill)
                .style(theme::outline(*p, p.danger, 1.0)),
            );
        }
        if s.trash.overflow {
            col = col.push(
                container(
                    column![
                        ui::colored(p.danger, s.trash.overflow_message(), 13.0),
                        checkbox(confirm_permanent)
                            .label("Je comprends, supprimer définitivement")
                            .on_toggle(Message::ToggleExecutePermanent)
                            .text_size(13)
                            .style(theme::check(*p)),
                    ]
                    .spacing(10),
                )
                .padding(12)
                .width(Length::Fill)
                .style(theme::outline(*p, p.danger, 1.0)),
            );
        }
        let ready = (s.delete == 0 || confirm_delete) && (!s.trash.overflow || confirm_permanent);
        col.push(
            row![
                ui::fill(),
                ui::action(p, Tone::Ghost, None, "Annuler", Some(Message::CloseDialog)),
                ui::action(
                    p,
                    if s.delete > 0 {
                        Tone::Danger
                    } else {
                        Tone::Success
                    },
                    Some(Glyph::Bolt),
                    "Exécuter maintenant",
                    ready.then_some(Message::Execute)
                ),
            ]
            .spacing(10),
        )
        .into()
    }

    pub(crate) fn toasts_layer(&self, p: &Palette) -> Option<El<'_, Message>> {
        if self.toasts.is_empty() {
            return None;
        }
        let mut col = column![].spacing(10).align_x(Alignment::End);
        for toast in &self.toasts {
            let t = toast.enter.at(self.now).clamp(0.0, 1.2);
            let age = self.now.saturating_duration_since(toast.born).as_secs_f32();
            let life = toast.life.as_secs_f32();
            let remaining = (1.0 - age / life).clamp(0.0, 1.0);
            let fade_out = ((life - age) / 0.35).clamp(0.0, 1.0);
            let tp = p.faded(t.min(1.0) * fade_out);
            let (glyph, color) = match toast.kind {
                ToastKind::Info => (Glyph::Info, tp.info),
                ToastKind::Success => (Glyph::Check, tp.success),
                ToastKind::Warning => (Glyph::Alert, tp.warning),
                ToastKind::Error => (Glyph::Alert, tp.danger),
            };
            let bar = container(Space::new())
                .width(Length::FillPortion(((remaining * 1000.0) as u16).max(1)))
                .height(Length::Fixed(3.0))
                .style(theme::bar(theme::alpha(color, 0.8)));
            let rest = Space::new().width(Length::FillPortion(
                (((1.0 - remaining) * 1000.0) as u16).max(1),
            ));
            let card = container(
                column![
                    row![
                        ui::glyph_box(glyph, color, 30.0),
                        column![
                            ui::strong(tp.text, toast.title.clone(), 14.0),
                            ui::colored(tp.muted, toast.body.clone(), 13.0)
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        button(icons::icon(Glyph::Close, 12.0, tp.faint))
                            .padding(4)
                            .style(theme::btn(tp, Tone::Ghost))
                            .on_press(Message::DismissToast(toast.id)),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Start),
                    row![bar, rest],
                ]
                .spacing(10),
            )
            .padding(Padding {
                top: 12.0,
                bottom: 8.0,
                left: 12.0,
                right: 10.0,
            })
            .width(Length::Fixed(360.0))
            .style(theme::card(tp, 0.6, false));
            // Slides in from the right with a slight overshoot.
            let slide = (1.0 - t) * 60.0;
            col = col.push(container(card).padding(Padding {
                right: slide.max(0.0),
                left: (-slide).max(0.0),
                ..Padding::ZERO
            }));
        }
        Some(
            container(col)
                .padding(Padding {
                    right: 20.0,
                    bottom: 20.0,
                    ..Padding::ZERO
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .align_right(Length::Fill)
                .align_bottom(Length::Fill)
                .into(),
        )
    }
}

fn dialog_head<'a>(p: &Palette, glyph: Glyph, color: iced::Color, title: &str) -> El<'a, Message> {
    row![
        ui::glyph_box(glyph, color, 40.0),
        ui::title(p, title.to_owned(), 19.0),
        ui::fill(),
        button(icons::icon(Glyph::Close, 14.0, p.faint))
            .padding(6)
            .style(theme::btn(*p, Tone::Ghost))
            .on_press(Message::CloseDialog),
    ]
    .spacing(14)
    .align_y(Alignment::Center)
    .into()
}

fn shortcuts<'a>(p: &Palette) -> El<'a, Message> {
    let keys = [
        ("Ctrl+N", "Nouveau profil"),
        ("Ctrl+E", "Modifier le profil sélectionné"),
        ("F5", "Simuler le profil sélectionné"),
        ("Entrée", "Exécuter la simulation affichée"),
        ("Suppr", "Retirer le profil sélectionné"),
        ("↑ / ↓", "Changer de profil"),
        ("Ctrl+F", "Rechercher un profil"),
        ("Ctrl+1…5", "Changer de section"),
        ("Ctrl+T", "Basculer le thème"),
        ("Ctrl+R", "Actualiser"),
        ("Ctrl+Q", "Quitter SyncEngine"),
        ("Échap", "Fermer menu, dialogue ou assistant"),
        ("F1", "Afficher cette aide"),
        ("Clic droit", "Actions contextuelles"),
        ("Glisser", "Déplacer un profil vers un groupe"),
    ];
    let rows = keys
        .into_iter()
        .fold(column![].spacing(8), |c, (k, label)| {
            c.push(
                row![
                    container(ui::kbd(p, k)).width(Length::Fixed(110.0)),
                    ui::colored(p.text, label, 14.0)
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
        });
    column![
        dialog_head(p, Glyph::Sliders, p.accent, "Raccourcis clavier"),
        rows
    ]
    .spacing(18)
    .into()
}
