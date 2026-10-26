//! Projection onto a case-insensitive receiver, retaining actual sender paths.
use super::*;

#[derive(Default)]
struct Group<'a> {
    left: Vec<&'a FileEntry>,
    right: Vec<&'a FileEntry>,
}

pub(super) fn compare(
    left: &Inventory,
    right: &Inventory,
    options: &CompareOptions,
) -> ComparisonReport {
    let mut groups: BTreeMap<String, Group<'_>> = BTreeMap::new();
    for entry in left.entries() {
        groups
            .entry(entry.path().as_str().to_lowercase())
            .or_default()
            .left
            .push(entry);
    }
    for entry in right.entries() {
        groups
            .entry(entry.path().as_str().to_lowercase())
            .or_default()
            .right
            .push(entry);
    }
    let mut canonical = BTreeMap::<String, ResourcePath>::new();
    let mut origins = BTreeMap::<ResourcePath, Vec<ResourcePath>>::new();
    let mut blocked = BTreeSet::<ResourcePath>::new();
    let mut type_conflicts = BTreeSet::<ResourcePath>::new();
    let mut rows = Vec::with_capacity(groups.len());
    let mut summary = ComparisonSummary::default();
    for (key, group) in groups {
        let Some(preferred) = group.right.first().or(group.left.first()) else {
            continue;
        };
        let preferred = preferred.path();
        let projected = preferred
            .parent()
            .and_then(|parent| canonical.get(&parent.as_str().to_lowercase()))
            .zip(preferred.file_name())
            .map(|(parent, name)| parent.join(name));
        let valid = !matches!(&projected, Some(Err(_)));
        let path = projected
            .and_then(Result::ok)
            .unwrap_or_else(|| preferred.clone());
        canonical.insert(key, path.clone());
        let l = group.left.first().copied();
        let r = group.right.first().copied();
        let directories = group
            .left
            .iter()
            .all(|entry| entry.kind() == EntryKind::Directory);
        let collision = group.right.len() > 1
            || (group.left.len() > 1 && !directories)
            || (group.left.len() > 1
                && r.is_some_and(|entry| entry.kind() != EntryKind::Directory));
        let ancestor_blocked = std::iter::successors(path.parent(), |parent| parent.parent())
            .any(|parent| blocked.contains(&parent));
        let behind_type_conflict = std::iter::successors(path.parent(), |parent| parent.parent())
            .any(|parent| type_conflicts.contains(&parent));
        if l.zip(r).is_some_and(|(a, b)| {
            a.kind() != b.kind()
                && (a.kind() == EntryKind::Directory || b.kind() == EntryKind::Directory)
        }) {
            type_conflicts.insert(path.clone());
        }
        let mut source_origins: Vec<_> = group
            .left
            .iter()
            .map(|entry| entry.path().clone())
            .collect();
        if directories && !source_origins.is_empty() {
            origins.insert(path.clone(), source_origins.clone());
        }
        let outcome = if behind_type_conflict {
            // A file/dir conflict cannot justify pruning the directory's content.
            ComparisonOutcome::Uncertain(vec![Uncertainty::InconsistentObservations])
        } else if collision || ancestor_blocked {
            if group
                .left
                .iter()
                .chain(&group.right)
                .any(|entry| entry.kind() == EntryKind::Directory)
            {
                blocked.insert(path.clone());
            }
            ComparisonOutcome::Uncertain(vec![Uncertainty::CaseCollision {
                with: group
                    .left
                    .iter()
                    .chain(&group.right)
                    .map(|entry| entry.path().clone())
                    .collect(),
            }])
        } else if !valid {
            ComparisonOutcome::Uncertain(vec![Uncertainty::InconsistentObservations])
        } else {
            match (l, r) {
                (Some(l), Some(r)) => compare_entries(l, r, options),
                (Some(_), None) => one_sided(Side::Left, right.presence(&path)),
                (None, Some(_)) => projected_absence(left, &path, &origins),
                (None, None) => continue,
            }
        };
        if !directories {
            source_origins.truncate(1);
        }
        if source_origins.as_slice() == [path.clone()] {
            source_origins.clear();
        }
        match &outcome {
            ComparisonOutcome::LeftOnly => summary.left_only += 1,
            ComparisonOutcome::RightOnly => summary.right_only += 1,
            ComparisonOutcome::Equivalent(basis) => {
                summary.equivalent += 1;
                if basis.is_content_verified() {
                    summary.content_verified += 1;
                }
            }
            ComparisonOutcome::Different(_) => summary.different += 1,
            ComparisonOutcome::Uncertain(_) => summary.uncertain += 1,
        }
        rows.push(PathComparison {
            path,
            left: l.cloned(),
            right: r.cloned(),
            outcome,
            source_origins,
        });
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    ComparisonReport {
        rows,
        left_complete: left.is_complete(),
        right_complete: right.is_complete(),
        summary,
        filter: left.filter_rules().clone(),
        left_connector: left.connector().clone(),
        right_connector: right.connector().clone(),
    }
}

fn projected_absence(
    left: &Inventory,
    path: &ResourcePath,
    origins: &BTreeMap<ResourcePath, Vec<ResourcePath>>,
) -> ComparisonOutcome {
    for parent in std::iter::successors(path.parent(), |p| p.parent()) {
        if let Some(parents) = origins.get(&parent) {
            for origin in parents {
                let Ok(candidate) = ResourcePath::from_segments(
                    origin
                        .segments()
                        .chain(path.segments().skip(parent.depth())),
                ) else {
                    return ComparisonOutcome::Uncertain(vec![
                        Uncertainty::InconsistentObservations,
                    ]);
                };
                let outcome = one_sided(Side::Right, left.presence(&candidate));
                if outcome != ComparisonOutcome::RightOnly {
                    return outcome;
                }
            }
            return ComparisonOutcome::RightOnly;
        }
    }
    one_sided(Side::Right, left.presence(path))
}
