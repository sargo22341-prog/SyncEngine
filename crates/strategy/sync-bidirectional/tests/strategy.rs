//! Phase 05 contract and safety tests.
#![expect(
    clippy::unwrap_used,
    reason = "isolated test fixtures fail immediately"
)]

use connector_api::{CaseSensitivity, StorageErrorKind, StorageOperation};
use connector_memory::{MemoryConnector, path, ts};
use sync_bidirectional::{BidirectionalStrategy, converged_baseline};
use sync_core::{CancellationToken, HashAlgorithm, PlanId, ProfileId};
use sync_decisions::PlanContext;
use sync_filter::{Filter, FilterRules};
use sync_plan::{
    Action, Baseline, DeletionGuard, DeletionPolicy, ExecutionPlan, OverwritePolicy, PlanPolicy,
    PlanSide,
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
