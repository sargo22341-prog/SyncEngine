//! Invariants du plan : construction, chargement JSON, plans altérés.

// Aides de test hors `#[test]` : `clippy.toml` n'y autorise pas `unwrap`,
// mais un échec y est bien un échec de test.
#![expect(
    clippy::unwrap_used,
    reason = "aides de test : un échec est un échec de test"
)]

use sync_core::{
    ConnectorId, EntryKind, FileMetadata, PlanId, ResourcePath, Timestamp, TimestampPrecision,
};
use sync_filter::FilterRules;
use sync_plan::{
    Action, DeletionBlock, DeletionGuard, DeletionPolicy, DeletionRefusal, EndpointSnapshot,
    EntrySnapshot, ExecutionPlan, OperationSpec, OverwritePolicy, PLAN_SCHEMA_VERSION, PlanError,
    PlanFormatError, PlanHeader, PlanPolicy, Precondition, Reason, Risk, SyncMode,
};

fn p(value: &str) -> ResourcePath {
    ResourcePath::parse(value).unwrap()
}

fn file(size: u64, secs: i64) -> EntrySnapshot {
    EntrySnapshot::new(
        EntryKind::File,
        FileMetadata::unknown()
            .with_size(size)
            .with_modified(Timestamp::new(secs, 0).unwrap(), TimestampPrecision::Second),
    )
}

fn dir() -> EntrySnapshot {
    EntrySnapshot::new(EntryKind::Directory, FileMetadata::unknown())
}

fn header(mode: SyncMode, policy: PlanPolicy, complete: (bool, bool)) -> PlanHeader {
    PlanHeader {
        id: PlanId::new("plan-test").unwrap(),
        profile: None,
        mode,
        created_at: Timestamp::new(1_800_000_000, 0).unwrap(),
        source: EndpointSnapshot::new(ConnectorId::new("mem").unwrap(), "src", complete.0),
        destination: EndpointSnapshot::new(ConnectorId::new("mem").unwrap(), "dst", complete.1),
        filter: FilterRules::new().exclude_file("*.tmp"),
        policy,
    }
}

fn mirror_policy() -> PlanPolicy {
    PlanPolicy::new(
        OverwritePolicy::SourceWins,
        DeletionPolicy::enabled(DeletionGuard::new().with_max_percent(None).unwrap()),
    )
}

fn spec(
    path: &str,
    action: Action,
    reason: Reason,
    source: Option<EntrySnapshot>,
    destination: Option<EntrySnapshot>,
) -> OperationSpec {
    OperationSpec {
        path: p(path),
        action,
        reason,
        source,
        destination,
    }
}

/// Plan miroir typique : création de dossier, copies, remplacement,
/// suppressions d'un dossier et de son contenu, conservation.
fn mirror_specs() -> Vec<OperationSpec> {
    vec![
        spec(
            "neuf",
            Action::Copy,
            Reason::MissingAtDestination,
            Some(dir()),
            None,
        ),
        spec(
            "neuf/a.txt",
            Action::Copy,
            Reason::MissingAtDestination,
            Some(file(10, 100)),
            None,
        ),
        spec(
            "commun.txt",
            Action::Keep,
            Reason::Equivalent { verified: false },
            Some(file(3, 100)),
            Some(file(3, 100)),
        ),
        spec(
            "modifie.txt",
            Action::Replace,
            Reason::SourceNewer,
            Some(file(7, 200)),
            Some(file(5, 100)),
        ),
        spec(
            "vieux",
            Action::Delete,
            Reason::AbsentFromSource,
            None,
            Some(dir()),
        ),
        spec(
            "vieux/b.txt",
            Action::Delete,
            Reason::AbsentFromSource,
            None,
            Some(file(1, 50)),
        ),
    ]
}

#[test]
fn construction_derives_ids_dependencies_and_counts() {
    let plan = ExecutionPlan::new(
        header(SyncMode::Mirror, mirror_policy(), (true, true)),
        mirror_specs(),
    )
    .unwrap();
    assert_eq!(plan.schema_version(), PLAN_SCHEMA_VERSION);
    let paths: Vec<&str> = plan
        .operations()
        .iter()
        .map(|op| op.path().as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "commun.txt",
            "modifie.txt",
            "neuf",
            "neuf/a.txt",
            "vieux",
            "vieux/b.txt"
        ]
    );
    let ids: Vec<&str> = plan
        .operations()
        .iter()
        .map(|op| op.id().as_str())
        .collect();
    assert_eq!(ids[0], "op-000001");
    assert_eq!(ids[5], "op-000006");

    let copy = plan.get(&p("neuf/a.txt")).unwrap();
    assert_eq!(
        copy.depends_on(),
        [plan.get(&p("neuf")).unwrap().id().clone()]
    );
    assert_eq!(copy.bytes(), Some(10));
    assert_eq!(copy.risk(), Risk::Additive);
    assert_eq!(
        copy.preconditions(),
        [
            Precondition::SourceUnchanged,
            Precondition::DestinationAbsent
        ]
    );

    let delete_dir = plan.get(&p("vieux")).unwrap();
    assert_eq!(delete_dir.kind(), EntryKind::Directory);
    assert_eq!(
        delete_dir.depends_on(),
        [plan.get(&p("vieux/b.txt")).unwrap().id().clone()]
    );
    assert_eq!(delete_dir.risk(), Risk::Destructive);
    assert_eq!(
        delete_dir.preconditions(),
        [
            Precondition::SourceAbsent,
            Precondition::DestinationUnchanged
        ]
    );

    assert_eq!(plan.source().entries(), 4);
    assert_eq!(plan.destination().entries(), 4);
    let summary = plan.summary();
    assert_eq!(
        (summary.copy, summary.replace, summary.keep, summary.delete),
        (2, 1, 1, 2)
    );
    assert_eq!(summary.bytes_to_transfer, 17);
    assert_eq!(summary.mutations(), 5);
    assert!(!plan.needs_attention());
}

#[test]
fn json_round_trip_preserves_the_plan() {
    let plan = ExecutionPlan::new(
        header(SyncMode::Mirror, mirror_policy(), (true, true)),
        mirror_specs(),
    )
    .unwrap();
    let json = plan.to_json().unwrap();
    assert!(json.starts_with("{\n  \"schema_version\": 1,"), "{json}");
    assert_eq!(ExecutionPlan::from_json(&json).unwrap(), plan);
    // Le désérialiseur serde générique applique la même validation.
    assert_eq!(serde_json::from_str::<ExecutionPlan>(&json).unwrap(), plan);
}

#[test]
fn backup_never_deletes() {
    let error = ExecutionPlan::new(
        header(SyncMode::Backup, mirror_policy(), (true, true)),
        mirror_specs(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        PlanError::DeletionNotAllowed {
            cause: DeletionBlock::ModeNeverDeletes,
            ..
        }
    ));
}

#[test]
fn deletion_requires_explicit_policy() {
    let policy = PlanPolicy::new(OverwritePolicy::SourceWins, DeletionPolicy::Disabled);
    let error = ExecutionPlan::new(
        header(SyncMode::Mirror, policy, (true, true)),
        mirror_specs(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        PlanError::DeletionNotAllowed {
            cause: DeletionBlock::PolicyDisabled,
            ..
        }
    ));
}

#[test]
fn incomplete_source_forbids_deletion() {
    // Given : un inventaire source incomplet.
    // When  : un plan contenant une suppression est construit.
    // Then  : il est refusé, quel que soit le reste de la politique.
    let error = ExecutionPlan::new(
        header(SyncMode::Mirror, mirror_policy(), (false, true)),
        mirror_specs(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        PlanError::DeletionNotAllowed {
            id: sync_core::OperationId::new("op-000005").unwrap(),
            cause: DeletionBlock::Refused(DeletionRefusal::SourceIncomplete),
        }
    );
}

#[test]
fn deletion_guard_is_enforced_on_the_plan() {
    let policy = PlanPolicy::new(
        OverwritePolicy::SourceWins,
        DeletionPolicy::enabled(DeletionGuard::new().with_max_count(Some(1))),
    );
    let error = ExecutionPlan::new(
        header(SyncMode::Mirror, policy, (true, true)),
        mirror_specs(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        PlanError::DeletionNotAllowed {
            cause: DeletionBlock::Refused(DeletionRefusal::TooMany { count: 2, limit: 1 }),
            ..
        }
    ));
}

#[test]
fn directory_deletion_requires_its_content_deleted() {
    let mut specs = mirror_specs();
    specs.push(spec(
        "vieux/garde.lnk",
        Action::Skip,
        Reason::UnsupportedEntryKind {
            kind: EntryKind::Symlink,
        },
        None,
        Some(EntrySnapshot::new(
            EntryKind::Symlink,
            FileMetadata::unknown(),
        )),
    ));
    let error = ExecutionPlan::new(
        header(SyncMode::Mirror, mirror_policy(), (true, true)),
        specs,
    )
    .unwrap_err();
    assert!(matches!(error, PlanError::DirectoryNotEmptied { .. }));
}

#[test]
fn interleaved_siblings_do_not_hide_directory_content() {
    // « vieux.bak » se classe entre « vieux » et « vieux/x.txt » : la
    // validation doit quand même voir « vieux/garde » conservé.
    let specs = vec![
        spec(
            "vieux",
            Action::Delete,
            Reason::AbsentFromSource,
            None,
            Some(dir()),
        ),
        spec(
            "vieux.bak",
            Action::Keep,
            Reason::Equivalent { verified: false },
            Some(file(1, 1)),
            Some(file(1, 1)),
        ),
        spec(
            "vieux/garde",
            Action::Keep,
            Reason::DeletionDisabled,
            None,
            Some(file(1, 1)),
        ),
    ];
    let error = ExecutionPlan::new(
        header(SyncMode::Mirror, mirror_policy(), (true, true)),
        specs,
    )
    .unwrap_err();
    assert!(
        matches!(error, PlanError::DirectoryNotEmptied { .. }),
        "{error}"
    );

    // Cas valide : les dépendances de « vieux » visent bien « vieux/x.txt ».
    let specs = vec![
        spec(
            "vieux",
            Action::Delete,
            Reason::AbsentFromSource,
            None,
            Some(dir()),
        ),
        spec(
            "vieux.bak",
            Action::Keep,
            Reason::Equivalent { verified: false },
            Some(file(1, 1)),
            Some(file(1, 1)),
        ),
        spec(
            "vieux/x.txt",
            Action::Delete,
            Reason::AbsentFromSource,
            None,
            Some(file(1, 1)),
        ),
    ];
    let plan = ExecutionPlan::new(
        header(SyncMode::Mirror, mirror_policy(), (true, true)),
        specs,
    )
    .unwrap();
    let vieux = plan.get(&p("vieux")).unwrap();
    assert_eq!(
        vieux.depends_on(),
        [plan.get(&p("vieux/x.txt")).unwrap().id().clone()]
    );
}

#[test]
fn copy_requires_an_available_parent() {
    let specs = vec![
        spec(
            "x",
            Action::Conflict,
            Reason::KindMismatch {
                source: EntryKind::Directory,
                destination: EntryKind::File,
            },
            Some(dir()),
            Some(file(1, 1)),
        ),
        spec(
            "x/y.txt",
            Action::Copy,
            Reason::MissingAtDestination,
            Some(file(1, 1)),
            None,
        ),
    ];
    let error = ExecutionPlan::new(
        header(SyncMode::Mirror, mirror_policy(), (true, true)),
        specs,
    )
    .unwrap_err();
    assert!(matches!(error, PlanError::ParentUnavailable { .. }));
}

#[test]
fn reasons_actions_and_observations_must_agree() {
    let wrong_reason = vec![spec(
        "a",
        Action::Delete,
        Reason::MissingAtDestination,
        None,
        Some(file(1, 1)),
    )];
    assert!(matches!(
        ExecutionPlan::new(
            header(SyncMode::Mirror, mirror_policy(), (true, true)),
            wrong_reason
        ),
        Err(PlanError::ReasonNotAllowed {
            action: Action::Delete,
            ..
        })
    ));
    // Une copie sur une destination observée présente serait un écrasement
    // implicite.
    let copy_over = vec![spec(
        "a",
        Action::Copy,
        Reason::MissingAtDestination,
        Some(file(1, 1)),
        Some(file(1, 1)),
    )];
    assert!(matches!(
        ExecutionPlan::new(
            header(SyncMode::Mirror, mirror_policy(), (true, true)),
            copy_over
        ),
        Err(PlanError::InvalidShape { .. })
    ));
    let copy_link = vec![spec(
        "a",
        Action::Copy,
        Reason::MissingAtDestination,
        Some(EntrySnapshot::new(
            EntryKind::Symlink,
            FileMetadata::unknown(),
        )),
        None,
    )];
    assert!(matches!(
        ExecutionPlan::new(
            header(SyncMode::Mirror, mirror_policy(), (true, true)),
            copy_link
        ),
        Err(PlanError::UnsupportedKind { .. })
    ));
    let duplicate = vec![
        spec(
            "a",
            Action::Copy,
            Reason::MissingAtDestination,
            Some(file(1, 1)),
            None,
        ),
        spec(
            "a",
            Action::Copy,
            Reason::MissingAtDestination,
            Some(file(1, 1)),
            None,
        ),
    ];
    assert!(matches!(
        ExecutionPlan::new(
            header(SyncMode::Mirror, mirror_policy(), (true, true)),
            duplicate
        ),
        Err(PlanError::DuplicatePath { .. })
    ));
}

#[test]
fn overwrite_policy_is_enforced() {
    let newer_dest = vec![spec(
        "a",
        Action::Replace,
        Reason::DestinationNewer,
        Some(file(1, 1)),
        Some(file(2, 2)),
    )];
    let cautious = PlanPolicy::new(OverwritePolicy::NewerSourceOnly, DeletionPolicy::Disabled);
    assert!(matches!(
        ExecutionPlan::new(
            header(SyncMode::Backup, cautious, (true, true)),
            newer_dest.clone()
        ),
        Err(PlanError::OverwriteNotAllowed { .. })
    ));
    let source_wins = PlanPolicy::new(OverwritePolicy::SourceWins, DeletionPolicy::Disabled);
    assert!(
        ExecutionPlan::new(
            header(SyncMode::Backup, source_wins, (true, true)),
            newer_dest
        )
        .is_ok()
    );
}

fn tampered(edit: impl FnOnce(&mut serde_json::Value)) -> Result<ExecutionPlan, PlanFormatError> {
    let plan = ExecutionPlan::new(
        header(SyncMode::Mirror, mirror_policy(), (true, true)),
        mirror_specs(),
    )
    .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&plan.to_json().unwrap()).unwrap();
    edit(&mut value);
    ExecutionPlan::from_json(&value.to_string())
}

#[test]
fn tampered_plans_are_rejected() {
    // Given : un plan valide sérialisé.
    // When  : on le modifie à la main pour le rendre plus destructeur.
    // Then  : le chargement le refuse.
    let error = tampered(|v| v["source"]["complete"] = false.into()).unwrap_err();
    assert!(
        matches!(
            error,
            PlanFormatError::Invalid(PlanError::DeletionNotAllowed { .. })
        ),
        "{error}"
    );

    let error = tampered(|v| v["mode"] = "backup".into()).unwrap_err();
    assert!(matches!(
        error,
        PlanFormatError::Invalid(PlanError::DeletionNotAllowed {
            cause: DeletionBlock::ModeNeverDeletes,
            ..
        })
    ));

    // Transformer une conservation en suppression : motif incompatible.
    let error = tampered(|v| v["operations"][0]["action"] = "delete".into()).unwrap_err();
    assert!(matches!(error, PlanFormatError::Invalid(_)), "{error}");

    // Supprimer une dépendance : champ dérivé incohérent.
    let error = tampered(|v| v["operations"][4]["depends_on"] = serde_json::json!([])).unwrap_err();
    assert!(
        matches!(
            error,
            PlanFormatError::Invalid(PlanError::DerivedMismatch {
                field: "depends_on",
                ..
            })
        ),
        "{error}"
    );

    // Fausser le décompte de la destination pour contourner un plafond.
    let error = tampered(|v| v["destination"]["entries"] = 1000.into()).unwrap_err();
    assert!(matches!(
        error,
        PlanFormatError::Invalid(PlanError::EntryCountMismatch { .. })
    ));

    let error = tampered(|v| v["inconnu"] = 1.into()).unwrap_err();
    assert!(matches!(error, PlanFormatError::Malformed(_)), "{error}");
}

#[test]
fn schema_version_is_checked_first() {
    let error = tampered(|v| v["schema_version"] = 3.into()).unwrap_err();
    assert!(matches!(
        error,
        PlanFormatError::UnsupportedVersion { found: 3 }
    ));
    let error = tampered(|v| {
        v.as_object_mut().unwrap().remove("schema_version");
    })
    .unwrap_err();
    assert!(matches!(error, PlanFormatError::MissingVersion));
    assert!(matches!(
        ExecutionPlan::from_json("pas du json"),
        Err(PlanFormatError::Malformed(_))
    ));
}

#[test]
fn attention_is_required_for_conflicts_and_incomplete_inventories() {
    let specs = vec![spec(
        "a",
        Action::Conflict,
        Reason::DestinationNewer,
        Some(file(1, 1)),
        Some(file(1, 2)),
    )];
    let policy = PlanPolicy::new(OverwritePolicy::NewerSourceOnly, DeletionPolicy::Disabled);
    let plan = ExecutionPlan::new(header(SyncMode::Backup, policy, (true, true)), specs).unwrap();
    assert!(plan.needs_attention());
    assert_eq!(plan.summary().mutations(), 0);

    let plan = ExecutionPlan::new(header(SyncMode::Backup, policy, (true, false)), vec![]).unwrap();
    assert!(plan.needs_attention());
}
