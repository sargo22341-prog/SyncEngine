//! Bounded directory workers. Only connectors advertising independent cursors
//! enter here. Each worker owns one directory result; only the coordinator
//! merges observations, schedules filtered children and invokes the observer.
use super::*;

pub(super) fn list(
    connector: &dyn StorageConnector,
    filter: &Filter,
    options: &ScanOptions,
    cancel: &CancellationToken,
    observe: &dyn Fn(&crate::ScanStats, usize),
    mut inventory: Inventory,
    workers: usize,
) -> Result<Inventory, ScanError> {
    let budget = AtomicUsize::new(options.entry_limit);
    std::thread::scope(|scope| {
        let mut senders = Vec::new();
        let mut handles = Vec::new();
        let (replies, results) = mpsc::sync_channel(workers);
        for index in 0..workers {
            let (send, receive) = mpsc::sync_channel::<ResourcePath>(1);
            let reply = replies.clone();
            let budget = &budget;
            handles.push(scope.spawn(move || {
                while let Ok(directory) = receive.recv() {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut part = Inventory::new(
                            connector.connector_id().clone(),
                            connector.capabilities().clone(),
                            filter.rules().clone(),
                            options.hash,
                        );
                        let children = list_directory(
                            connector, filter, &mut part, &directory, cancel, budget,
                        );
                        (part, children)
                    }))
                    .ok();
                    if reply.send((index, directory, outcome)).is_err() {
                        break; // Coordinator stopped; no further storage calls.
                    }
                }
            }));
            senders.push(send);
        }
        drop(replies);
        let mut pending = vec![ResourcePath::root()];
        let mut free: Vec<_> = (0..workers).collect();
        let mut busy = 0;
        let mut interrupted = false;
        let result = (|| {
            while !pending.is_empty() || busy > 0 {
                interrupted |= cancel.checkpoint();
                while !interrupted && !pending.is_empty() && !free.is_empty() {
                    let (Some(index), Some(directory)) = (free.pop(), pending.pop()) else {
                        break;
                    };
                    if senders[index].send(directory).is_err() {
                        return Err(worker_failed(inventory));
                    }
                    busy += 1;
                }
                if busy == 0 {
                    break;
                }
                let Ok((index, directory, Some((mut part, children)))) = results.recv() else {
                    return Err(worker_failed(inventory));
                };
                busy -= 1;
                free.push(index);
                merge(&mut inventory, &mut part);
                match children {
                    Ok(children) => pending.extend(children.into_iter().rev()),
                    Err(error) if error.kind() == StorageErrorKind::Cancelled => {
                        interrupted = true;
                        pending.push(directory);
                    }
                    Err(error) if directory.is_root() => {
                        return Err(ScanError::RootUnavailable(error));
                    }
                    Err(error) => {
                        let kind = error.kind();
                        inventory.directories.insert(
                            directory.clone(),
                            DirectoryState::Incomplete(IncompleteReason::ListFailed(kind)),
                        );
                        inventory
                            .issues
                            .push(ScanIssue::new(directory, ScanIssueKind::ListFailed(kind)));
                    }
                }
                observe(&inventory.stats, inventory.issues.len());
            }
            if interrupted {
                return Err(cancelled(inventory, &pending));
            }
            Ok(inventory)
        })();
        drop(senders);
        drop(results); // Release workers even if their result was not consumed.
        let mut failed = false;
        for handle in handles {
            failed |= handle.join().is_err();
        }
        match result {
            Ok(inventory) if failed => Err(worker_failed(inventory)),
            other => other,
        }
    })
}

fn worker_failed(mut inventory: Inventory) -> ScanError {
    inventory.directories.insert(
        ResourcePath::root(),
        DirectoryState::Incomplete(IncompleteReason::NotScanned),
    );
    ScanError::WorkerFailed {
        partial: Box::new(inventory),
    }
}

fn merge(inventory: &mut Inventory, part: &mut Inventory) {
    // Every accepted entry is a direct child of its task directory. Tasks are
    // unique, so valid entries from different directories cannot overlap.
    inventory.entries.extend(std::mem::take(&mut part.entries));
    inventory
        .excluded
        .extend(std::mem::take(&mut part.excluded));
    inventory
        .directories
        .extend(std::mem::take(&mut part.directories));
    inventory.issues.append(&mut part.issues);
    inventory.stats.files += part.stats.files;
    inventory.stats.directories += part.stats.directories;
    inventory.stats.others += part.stats.others;
    inventory.stats.excluded += part.stats.excluded;
    inventory.stats.hashed += part.stats.hashed;
    inventory.stats.consistency_warnings += part.stats.consistency_warnings;
    inventory.stats.known_bytes = inventory
        .stats
        .known_bytes
        .saturating_add(part.stats.known_bytes);
}
