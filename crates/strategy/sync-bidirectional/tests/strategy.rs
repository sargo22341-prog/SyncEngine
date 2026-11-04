//! Phase 05 contract and safety tests.
#![expect(
    clippy::unwrap_used,
    reason = "isolated test fixtures fail immediately"
)]

use connector_api::{CaseSensitivity, StorageErrorKind, StorageOperation};
use connector_memory::{MemoryConnector, path, ts};
use sync_bidirectional::{
    BidirectionalStrategy, ConflictResolution, Resolutions, converged_baseline,
};
use sync_core::{CancellationToken, HashAlgorithm, PlanId, ProfileId};
use sync_decisions::PlanContext;
use sync_filter::{Filter, FilterRules};
use sync_plan::{
    Action, Baseline, DeletionGuard, DeletionPolicy, EntrySnapshot, ExecutionPlan, OverwritePolicy,
    PlanPolicy, PlanSide, Reason,
};
use sync_scanner::{Inventory, ScanOptions, scan};

fn context() -> PlanContext {
    PlanContext {
        plan_id: PlanId::new("plan").unwrap(),
        profile: Some(ProfileId::new("profile").unwrap()),
        created_at: ts(1),
        source_location: "left-root".into(),
        destination_location: "right-root".into(),
    }
}
fn inventory(c: &MemoryConnector) -> Inventory {
    scan(
        c,
        &Filter::include_all(),
        &ScanOptions::new().with_hash(HashAlgorithm::Blake3),
        &CancellationToken::new(),
    )
    .unwrap()
}
fn policy() -> PlanPolicy {
    PlanPolicy::new(
        OverwritePolicy::SourceWins,
        DeletionPolicy::enabled(
            DeletionGuard::new()
                .with_empty_source_allowed(true)
                .with_max_percent(None)
                .unwrap(),
        ),
    )
}
fn plan(l: &MemoryConnector, r: &MemoryConnector, b: Option<&Baseline>) -> ExecutionPlan {
    BidirectionalStrategy
        .build_plan(&inventory(l), &inventory(r), &context(), b, &policy())
        .unwrap()
}
fn baseline(l: &MemoryConnector, r: &MemoryConnector) -> Baseline {
    converged_baseline(&inventory(l), &inventory(r), &context(), None).unwrap()
}

#[test]
fn first_run_copies_new_files_in_both_directions_without_deletions() {
    let l = MemoryConnector::new("l").file("left", b"L", ts(1));
    let r = MemoryConnector::new("r").file("right", b"R", ts(100));
    let p = plan(&l, &r, None);
    assert_eq!(p.summary().copy, 2);
    assert_eq!(p.summary().delete, 0);
    assert_eq!(
        p.get(&path("left")).unwrap().target(),
        PlanSide::Destination
    );
    assert_eq!(p.get(&path("right")).unwrap().target(), PlanSide::Source);
    assert_eq!(p.schema_version(), 2);
    assert_eq!(ExecutionPlan::from_json(&p.to_json().unwrap()).unwrap(), p);
}

#[test]
fn unverified_first_run_and_divergent_first_run_never_overwrite() {
    let l = MemoryConnector::new("l").file("f", b"AA", ts(10));
    let r = MemoryConnector::new("r").file("f", b"BB", ts(1));
    assert_eq!(plan(&l, &r, None).summary().conflict, 1);
    let options = ScanOptions::new();
    let cancel = CancellationToken::new();
    let p = BidirectionalStrategy
        .build_plan(
            &scan(&l, &Filter::include_all(), &options, &cancel).unwrap(),
            &scan(&r, &Filter::include_all(), &options, &cancel).unwrap(),
            &context(),
            None,
            &policy(),
        )
        .unwrap();
    assert_eq!(p.summary().mutations(), 0);
}

#[test]
fn changes_with_identical_size_and_timestamp_are_detected_on_either_side() {
    for reverse in [false, true] {
        let l = MemoryConnector::new("l").file("f", b"AA", ts(1));
        let r = MemoryConnector::new("r").file("f", b"AA", ts(1));
        let b = baseline(&l, &r);
        (if reverse { &r } else { &l }).put_file("f", b"BB", ts(1));
        let p = plan(&l, &r, Some(&b));
        assert_eq!(p.summary().replace, 1);
        assert_eq!(
            p.get(&path("f")).unwrap().target(),
            if reverse {
                PlanSide::Source
            } else {
                PlanSide::Destination
            }
        );
    }
}

#[test]
fn deletion_requires_matching_baseline_and_unchanged_survivor_in_both_directions() {
    for reverse in [false, true] {
        let l = MemoryConnector::new("l").file("f", b"old", ts(1));
        let r = MemoryConnector::new("r").file("f", b"old", ts(1));
        let b = baseline(&l, &r);
        (if reverse { &r } else { &l }).remove("f");
        assert_eq!(plan(&l, &r, None).summary().delete, 0);
        let p = plan(&l, &r, Some(&b));
        assert_eq!(p.summary().delete, 1);
        assert_eq!(
            p.get(&path("f")).unwrap().target(),
            if reverse {
                PlanSide::Source
            } else {
                PlanSide::Destination
            }
        );
        (if reverse { &l } else { &r }).put_file("f", b"new", ts(1));
        let conflict = plan(&l, &r, Some(&b));
        assert_eq!(conflict.summary().conflict, 1);
        assert_eq!(conflict.summary().mutations(), 0);
    }
}

#[test]
fn concurrent_edits_preserve_both_and_identical_edits_converge() {
    let l = MemoryConnector::new("l").file("f", b"old", ts(1));
    let r = MemoryConnector::new("r").file("f", b"old", ts(1));
    let b = baseline(&l, &r);
    l.put_file("f", b"AAA", ts(1));
    r.put_file("f", b"BBB", ts(999));
    assert_eq!(plan(&l, &r, Some(&b)).summary().conflict, 1);
    r.put_file("f", b"AAA", ts(999));
    assert_eq!(plan(&l, &r, Some(&b)).summary().keep, 1);
    assert!(converged_baseline(&inventory(&l), &inventory(&r), &context(), Some(&b)).is_ok());
}

#[test]
fn tombstone_resurrection_is_conflict_and_tombstones_do_not_expire() {
    let l = MemoryConnector::new("l").file("f", b"old", ts(1));
    let r = MemoryConnector::new("r").file("f", b"old", ts(1));
    let b = baseline(&l, &r);
    l.remove("f");
    r.remove("f");
    let deleted = converged_baseline(&inventory(&l), &inventory(&r), &context(), Some(&b)).unwrap();
    assert_eq!(deleted.entries().get(&path("f")), Some(&None));
    let again =
        converged_baseline(&inventory(&l), &inventory(&r), &context(), Some(&deleted)).unwrap();
    assert_eq!(again.entries(), deleted.entries());
    r.put_file("f", b"old", ts(1));
    assert_eq!(plan(&l, &r, Some(&again)).summary().conflict, 1);
}

#[test]
fn incomplete_scan_means_zero_mutations_and_no_baseline() {
    let l = MemoryConnector::new("l").dir("d").file("d/f", b"x", ts(1));
    let r = MemoryConnector::new("r").dir("d").file("d/f", b"x", ts(1));
    let b = baseline(&l, &r);
    let l = l.fail(
        "d",
        StorageOperation::List,
        StorageErrorKind::PermissionDenied,
    );
    let p = plan(&l, &r, Some(&b));
    assert_eq!(p.summary().mutations(), 0);
    assert!(p.needs_attention());
    assert!(converged_baseline(&inventory(&l), &inventory(&r), &context(), Some(&b)).is_err());
}

#[test]
fn partial_hash_failure_does_not_become_a_deletion() {
    let l = MemoryConnector::new("l").file("f", b"x", ts(1)).fail(
        "f",
        StorageOperation::Read,
        StorageErrorKind::PermissionDenied,
    );
    let r = MemoryConnector::new("r").file("f", b"x", ts(1));
    assert_eq!(plan(&l, &r, None).summary().mutations(), 0);
}

#[test]
fn filters_roots_profile_and_endpoint_order_cannot_reuse_reference() {
    let l = MemoryConnector::new("l");
    let r = MemoryConnector::new("r");
    let b = baseline(&l, &r);
    for change in 0..4 {
        let mut ctx = context();
        match change {
            0 => ctx.source_location = "other".into(),
            1 => ctx.profile = Some(ProfileId::new("other").unwrap()),
            2 => std::mem::swap(&mut ctx.source_location, &mut ctx.destination_location),
            _ => {}
        };
        let filter = if change == 3 {
            Filter::new(FilterRules::new().exclude_file("*.tmp")).unwrap()
        } else {
            Filter::include_all()
        };
        let opts = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
        let cancel = CancellationToken::new();
        assert!(
            BidirectionalStrategy
                .build_plan(
                    &scan(&l, &filter, &opts, &cancel).unwrap(),
                    &scan(&r, &filter, &opts, &cancel).unwrap(),
                    &ctx,
                    Some(&b),
                    &policy()
                )
                .is_err()
        );
    }
}

#[test]
fn directory_conflict_prevents_parent_deletion_even_with_interleaved_names() {
    let l = MemoryConnector::new("l")
        .dir("d")
        .file("d/f", b"old", ts(1))
        .file("d.old", b"x", ts(1));
    let r = MemoryConnector::new("r")
        .dir("d")
        .file("d/f", b"old", ts(1))
        .file("d.old", b"x", ts(1));
    let b = baseline(&l, &r);
    l.remove("d");
    r.put_file("d/f", b"new", ts(1));
    let p = plan(&l, &r, Some(&b));
    assert_eq!(p.summary().delete, 0);
    assert_eq!(p.get(&path("d/f")).unwrap().action(), Action::Conflict);
}

#[test]
fn guards_and_disabled_deletion_preserve_survivors() {
    let l = MemoryConnector::new("l").file("f", b"x", ts(1));
    let r = MemoryConnector::new("r").file("f", b"x", ts(1));
    let b = baseline(&l, &r);
    l.remove("f");
    for deletion in [
        DeletionPolicy::Disabled,
        DeletionPolicy::enabled(DeletionGuard::new()),
    ] {
        let p = BidirectionalStrategy
            .build_plan(
                &inventory(&l),
                &inventory(&r),
                &context(),
                Some(&b),
                &PlanPolicy::new(OverwritePolicy::SourceWins, deletion),
            )
            .unwrap();
        assert_eq!(p.summary().delete, 0);
    }
}

#[test]
fn name_collisions_are_never_copied() {
    let l = MemoryConnector::new("l")
        .file("A", b"x", ts(1))
        .file("a", b"y", ts(1));
    let r = MemoryConnector::new("r").with_case_sensitivity(CaseSensitivity::Insensitive);
    assert_eq!(plan(&l, &r, None).summary().mutations(), 0);
}

#[test]
fn baseline_evidence_and_direction_tampering_are_rejected() {
    let l = MemoryConnector::new("l").file("f", b"old", ts(1));
    let r = MemoryConnector::new("r").file("f", b"old", ts(1));
    let b = baseline(&l, &r);
    l.remove("f");
    let p = plan(&l, &r, Some(&b));
    let mut json = serde_json::to_value(&p).unwrap();
    json["baseline"] = serde_json::Value::Null;
    assert!(ExecutionPlan::from_json(&json.to_string()).is_err());
    let mut json = serde_json::to_value(&p).unwrap();
    json["mode"] = "mirror".into();
    json["schema_version"] = 1.into();
    assert!(ExecutionPlan::from_json(&json.to_string()).is_err());
    let mut json = serde_json::to_value(&p).unwrap();
    json["operations"][0]["target"] = "source".into();
    assert!(ExecutionPlan::from_json(&json.to_string()).is_err());
}

/// Decision of the user on the conflict of `f`, for the contents currently held.
fn decision(l: &MemoryConnector, r: &MemoryConnector, winner: PlanSide) -> Resolutions {
    decision_of("f", l, r, winner, false)
}
fn decision_of(
    name: &str,
    l: &MemoryConnector,
    r: &MemoryConnector,
    winner: PlanSide,
    keep_both: bool,
) -> Resolutions {
    let seen = |c: &MemoryConnector| EntrySnapshot::from(inventory(c).get(&path(name)).unwrap());
    Resolutions::from([(
        path(name),
        ConflictResolution {
            winner,
            keep_both,
            seen_source: seen(l),
            seen_destination: seen(r),
        },
    )])
}
fn resolved(l: &MemoryConnector, r: &MemoryConnector, d: &Resolutions) -> ExecutionPlan {
    BidirectionalStrategy
        .build_plan_resolved(&inventory(l), &inventory(r), &context(), None, &policy(), d)
        .unwrap()
}

#[test]
fn a_user_decision_turns_a_conflict_into_a_visible_replacement_in_the_chosen_direction() {
    let l = MemoryConnector::new("l").file("f", b"LEFT", ts(10));
    let r = MemoryConnector::new("r").file("f", b"RIGHT", ts(1));
    assert_eq!(plan(&l, &r, None).summary().conflict, 1);
    for (winner, target) in [
        (PlanSide::Source, PlanSide::Destination),
        (PlanSide::Destination, PlanSide::Source),
    ] {
        let p = resolved(&l, &r, &decision(&l, &r, winner));
        let op = p.get(&path("f")).unwrap();
        assert_eq!(p.summary().conflict, 0);
        assert_eq!(p.summary().replace, 1);
        assert_eq!(op.action(), Action::Replace);
        assert_eq!(*op.reason(), Reason::ConflictResolved);
        assert_eq!(op.target(), target);
        // The plan stays valid after a JSON round trip (preview = execution).
        assert_eq!(ExecutionPlan::from_json(&p.to_json().unwrap()).unwrap(), p);
    }
}

#[test]
fn a_decision_made_on_other_contents_is_ignored_and_the_conflict_is_kept() {
    let l = MemoryConnector::new("l").file("f", b"LEFT", ts(10));
    let r = MemoryConnector::new("r").file("f", b"RIGHT", ts(1));
    let d = decision(&l, &r, PlanSide::Source);
    // The right file changed after the user looked at it.
    r.put_file("f", b"NEWER-RIGHT", ts(20));
    let p = resolved(&l, &r, &d);
    assert_eq!(p.summary().conflict, 1);
    assert_eq!(p.summary().mutations(), 0);
}

#[test]
fn a_decision_never_applies_to_a_path_that_is_not_a_conflict() {
    let l = MemoryConnector::new("l").file("f", b"SAME", ts(1));
    let r = MemoryConnector::new("r").file("f", b"SAME", ts(1));
    let d = decision(&l, &r, PlanSide::Source);
    let p = resolved(&l, &r, &d);
    assert_eq!(p.summary().mutations(), 0);
    assert_eq!(p.get(&path("f")).unwrap().action(), Action::Keep);
}

fn kept_both(
    l: &MemoryConnector,
    r: &MemoryConnector,
    name: &str,
    winner: PlanSide,
) -> ExecutionPlan {
    resolved(l, r, &decision_of(name, l, r, winner, true))
}

#[test]
fn keep_both_copies_the_losing_content_first_and_replaces_after_it() {
    for (winner, copy_target, replace_target, label) in [
        (
            PlanSide::Source,
            PlanSide::Source,
            PlanSide::Destination,
            "destination",
        ),
        (
            PlanSide::Destination,
            PlanSide::Destination,
            PlanSide::Source,
            "source",
        ),
    ] {
        let l = MemoryConnector::new("l").file("d/f.txt", b"LEFT", ts(10));
        let r = MemoryConnector::new("r").file("d/f.txt", b"RIGHT", ts(1));
        let p = kept_both(&l, &r, "d/f.txt", winner);
        let name = format!("d/f (conflit {label}).txt");
        let copy = p.get(&path(&name)).unwrap();
        let replace = p.get(&path("d/f.txt")).unwrap();
        assert_eq!((p.summary().copy, p.summary().replace), (1, 1));
        assert_eq!(copy.action(), Action::Copy);
        assert_eq!(*copy.reason(), Reason::ConflictKeptBoth);
        assert_eq!(copy.target(), copy_target);
        assert_eq!(copy.source_path(), &path("d/f.txt"));
        assert_eq!(replace.target(), replace_target);
        assert_eq!(*replace.reason(), Reason::ConflictResolved);
        assert!(
            replace.depends_on().contains(copy.id()),
            "the content is copied before it is replaced"
        );
        // Plans with a physical source path use the extended schema.
        assert_eq!(p.schema_version(), 3);
        assert_eq!(ExecutionPlan::from_json(&p.to_json().unwrap()).unwrap(), p);
    }
}

#[test]
fn the_copy_name_keeps_the_extension_avoids_taken_names_and_handles_dot_files() {
    let l = MemoryConnector::new("l")
        .file("f.txt", b"LEFT", ts(10))
        // Already taken, on the other side and with another case.
        .file("f (conflit destination).txt", b"x", ts(1));
    let r = MemoryConnector::new("r")
        .file("f.txt", b"RIGHT", ts(1))
        .file("F (CONFLIT DESTINATION 2).TXT", b"y", ts(1));
    let p = kept_both(&l, &r, "f.txt", PlanSide::Source);
    assert!(p.get(&path("f (conflit destination 3).txt")).is_some());
    for (name, expected) in [
        ("Makefile", "Makefile (conflit destination)"),
        (".env", ".env (conflit destination)"),
        ("archive.tar.gz", "archive.tar (conflit destination).gz"),
    ] {
        let l = MemoryConnector::new("l").file(name, b"LEFT", ts(10));
        let r = MemoryConnector::new("r").file(name, b"RIGHT", ts(1));
        let p = kept_both(&l, &r, name, PlanSide::Source);
        assert!(p.get(&path(expected)).is_some(), "{expected}");
    }
}

#[test]
fn a_stale_keep_both_decision_changes_nothing() {
    let l = MemoryConnector::new("l").file("f", b"LEFT", ts(10));
    let r = MemoryConnector::new("r").file("f", b"RIGHT", ts(1));
    let d = decision_of("f", &l, &r, PlanSide::Source, true);
    l.put_file("f", b"CHANGED-LEFT", ts(20));
    let p = resolved(&l, &r, &d);
    assert_eq!(p.summary().conflict, 1);
    assert_eq!(p.summary().mutations(), 0);
}

#[test]
fn tampered_keep_both_plans_are_refused_when_loaded() {
    let l = MemoryConnector::new("l").file("f.txt", b"LEFT", ts(10));
    let r = MemoryConnector::new("r").file("f.txt", b"RIGHT", ts(1));
    let json = kept_both(&l, &r, "f.txt", PlanSide::Source)
        .to_json()
        .unwrap();
    let edit = |change: &dyn Fn(&mut serde_json::Value)| -> bool {
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        change(&mut value);
        ExecutionPlan::from_json(&value.to_string()).is_err()
    };
    // The replacement no longer waits for the copy.
    assert!(edit(&|v| {
        for op in v["operations"].as_array_mut().unwrap() {
            if op["reason"]["code"] == "conflict_resolved" {
                op["depends_on"] = serde_json::json!([]);
            }
        }
    }));
    // The copy reads another file than the conflict it belongs to.
    assert!(edit(&|v| {
        for op in v["operations"].as_array_mut().unwrap() {
            if op["reason"]["code"] == "conflict_kept_both" {
                op["source"]["origin_paths"] = serde_json::json!(["other.txt"]);
            }
        }
    }));
    // The copy lands in another folder.
    assert!(edit(&|v| {
        for op in v["operations"].as_array_mut().unwrap() {
            if op["reason"]["code"] == "conflict_kept_both" {
                op["path"] = serde_json::json!("sub/copy.txt");
            }
        }
    }));
    // The conflict is no longer a resolved one: the copy has no companion.
    assert!(edit(&|v| {
        for op in v["operations"].as_array_mut().unwrap() {
            if op["reason"]["code"] == "conflict_resolved" {
                op["reason"] = serde_json::json!({"code": "content_differs"});
            }
        }
    }));
    // The untouched plan loads.
    assert!(!edit(&|_| {}));
}
