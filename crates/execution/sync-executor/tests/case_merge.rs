//! Case-sensitive sender -> insensitive receiver: real plans and execution.
#![expect(
    clippy::unwrap_used,
    reason = "integration-test assertions and fixtures"
)]
mod common;
use common::*;
use connector_api::CaseSensitivity;
use connector_memory::{MemoryConnector, ts};
use sync_core::{CancellationToken, PlanId};
use sync_decisions::{PlanContext, PlanStrategy, PlanningInput};
use sync_filter::Filter;
use sync_journal::MemoryJournal;
use sync_plan::{Action, ExecutionPlan};

fn merged(source: &MemoryConnector, target: &MemoryConnector, filter: &Filter) -> ExecutionPlan {
    let cancel = CancellationToken::new();
    let options = sync_scanner::ScanOptions::new();
    let left = sync_scanner::scan(source, filter, &options, &cancel).unwrap();
    let right = sync_scanner::scan(target, filter, &options, &cancel).unwrap();
    let report = sync_compare::compare(
        &left,
        &right,
        &sync_compare::CompareOptions::new().with_directory_merge(true),
    )
    .unwrap();
    let context = PlanContext {
        plan_id: PlanId::new("merge-test").unwrap(),
        profile: None,
        created_at: now(),
        source_location: "source".into(),
        destination_location: "destination".into(),
    };
    sync_mirror::MirrorStrategy::new()
        .build_plan(&PlanningInput::new(&report, &context), &mirror_policy())
        .unwrap()
}

#[test]
fn merge_copies_both_branches_to_empty_or_existing_destination_and_is_repeatable() {
    for existing in [false, true] {
        let source = MemoryConnector::new("source")
            .file("People/B/Artist/photo.jpg", b"uppercase branch", ts(20))
            .file("People/b/bbno$/folder.jpg", b"lowercase branch", ts(20))
            .dir("People/c");
        let target = MemoryConnector::new("destination")
            .writable()
            .with_case_sensitivity(CaseSensitivity::InsensitivePreserving);
        if existing {
            target.put_file("People/B/bbno$/folder.jpg", b"old", ts(1));
        }
        let plan = merged(&source, &target, &Filter::include_all());
        assert_eq!(plan.schema_version(), 3);
        assert!(
            plan.operations()
                .iter()
                .all(|op| !matches!(op.action(), Action::Skip | Action::Conflict))
        );
        let plan = ExecutionPlan::from_json(&plan.to_json().unwrap()).unwrap();
        let report = execute(
            &plan,
            &source,
            &target,
            confirmed(),
            &mut MemoryJournal::new(),
        )
        .unwrap();
        assert_eq!(report.totals().refused, 0);
        assert_eq!(
            target.content("People/B/Artist/photo.jpg"),
            Some(b"uppercase branch".to_vec())
        );
        assert_eq!(
            target.content("People/B/bbno$/folder.jpg"),
            Some(b"lowercase branch".to_vec())
        );
        assert_eq!(
            merged(&source, &target, &Filter::include_all())
                .summary()
                .mutations(),
            0
        );
        assert_eq!(source.mutations(), 0);
    }
}

#[test]
fn real_file_collision_does_not_block_unrelated_children_or_overwrite_the_target() {
    let source = MemoryConnector::new("source")
        .file("B/same", b"first", ts(2))
        .file("b/same", b"second", ts(2))
        .file("b/unique", b"safe", ts(2));
    let target = MemoryConnector::new("destination")
        .writable()
        .with_case_sensitivity(CaseSensitivity::InsensitivePreserving)
        .file("B/same", b"preserve", ts(1));
    let plan = merged(&source, &target, &Filter::include_all());
    assert_eq!(
        plan.operations()
            .iter()
            .filter(|op| op.action() == Action::Skip)
            .count(),
        1
    );
    execute(
        &plan,
        &source,
        &target,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert_eq!(target.content("B/same"), Some(b"preserve".to_vec()));
    assert_eq!(target.content("B/unique"), Some(b"safe".to_vec()));
}

#[test]
fn mirror_checks_every_source_spelling_before_deletion() {
    let source = MemoryConnector::new("source").dir("B").dir("b");
    let target = MemoryConnector::new("destination")
        .writable()
        .with_case_sensitivity(CaseSensitivity::InsensitivePreserving)
        .file("B/old/deep/file", b"preserve if source reappears", ts(1));
    let plan = merged(&source, &target, &Filter::include_all());
    assert!(
        plan.operations()
            .iter()
            .any(|op| op.action() == Action::Delete)
    );
    // Given an absent source at preview, When it reappears in the other spelling,
    // Then preserve the destination and refuse the deletion.
    source.put_file("b/old/deep/file", b"reappeared", ts(2));
    let report = execute(
        &plan,
        &source,
        &target,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(report.totals().refused > 0);
    assert!(target.content("B/old/deep/file").is_some());
}

#[test]
fn origin_paths_cannot_redirect_a_plan_to_an_unrelated_source() {
    let source = MemoryConnector::new("source").file("b/file", b"safe", ts(1));
    let target = MemoryConnector::new("destination")
        .writable()
        .with_case_sensitivity(CaseSensitivity::InsensitivePreserving)
        .dir("B");
    let plan = merged(&source, &target, &Filter::include_all());
    let mut json: serde_json::Value = serde_json::from_str(&plan.to_json().unwrap()).unwrap();
    for op in json["operations"].as_array_mut().unwrap() {
        if op["path"] == "B/file" {
            op["source"]["origin_paths"] = serde_json::json!(["unrelated/secret"]);
        }
    }
    assert!(ExecutionPlan::from_json(&json.to_string()).is_err());
    let mut json: serde_json::Value = serde_json::from_str(&plan.to_json().unwrap()).unwrap();
    json["schema_version"] = 1.into();
    assert!(ExecutionPlan::from_json(&json.to_string()).is_err());
}

#[test]
fn exclusion_in_one_source_spelling_never_becomes_a_mirror_deletion() {
    let source = MemoryConnector::new("source").dir("B").file(
        "b/private/deep/file",
        b"excluded source",
        ts(1),
    );
    let target = MemoryConnector::new("destination")
        .writable()
        .with_case_sensitivity(CaseSensitivity::InsensitivePreserving)
        .file("B/private/deep/file", b"preserve", ts(1));
    let filter =
        Filter::new(sync_filter::FilterRules::new().exclude_directory("b/private")).unwrap();
    let plan = merged(&source, &target, &filter);
    assert!(
        plan.operations()
            .iter()
            .all(|op| op.action() != Action::Delete)
    );
    execute(
        &plan,
        &source,
        &target,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert_eq!(
        target.content("B/private/deep/file"),
        Some(b"preserve".to_vec())
    );
}

#[test]
fn content_candidates_hash_both_original_spellings() {
    let source = MemoryConnector::new("source").file("b/file", b"same", ts(1));
    let target = MemoryConnector::new("destination")
        .with_case_sensitivity(CaseSensitivity::InsensitivePreserving)
        .file("B/file", b"same", ts(1));
    let cancel = CancellationToken::new();
    let filter = Filter::include_all();
    let listing = sync_scanner::ScanOptions::new();
    let left = sync_scanner::scan(&source, &filter, &listing, &cancel).unwrap();
    let right = sync_scanner::scan(&target, &filter, &listing, &cancel).unwrap();
    let options = sync_compare::CompareOptions::new().with_directory_merge(true);
    let candidates =
        sync_compare::content_candidates_with_options(&left, &right, &options).unwrap();
    assert_eq!(candidates, vec![p("B/file"), p("b/file")]);
    let hashing = sync_scanner::ScanOptions::new().with_hash(sync_core::HashAlgorithm::Blake3);
    let left =
        sync_scanner::hash_selected(&source, left, &candidates, &hashing, &cancel, &|_, _| {})
            .unwrap();
    let right =
        sync_scanner::hash_selected(&target, right, &candidates, &hashing, &cancel, &|_, _| {})
            .unwrap();
    assert_eq!(
        sync_compare::compare(&left, &right, &options)
            .unwrap()
            .summary()
            .content_verified,
        1
    );
}

#[test]
fn file_directory_conflict_preserves_all_destination_descendants() {
    let source = MemoryConnector::new("source").file("B", b"now a file", ts(1));
    let target = MemoryConnector::new("destination")
        .writable()
        .with_case_sensitivity(CaseSensitivity::InsensitivePreserving)
        .file("b/sub/precious", b"keep", ts(0));
    let plan = merged(&source, &target, &Filter::include_all());
    assert!(
        plan.operations()
            .iter()
            .any(|op| op.action() == Action::Conflict)
    );
    assert!(
        plan.operations()
            .iter()
            .all(|op| op.action() != Action::Delete)
    );
    execute(
        &plan,
        &source,
        &target,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert_eq!(target.content("b/sub/precious"), Some(b"keep".to_vec()));
}
