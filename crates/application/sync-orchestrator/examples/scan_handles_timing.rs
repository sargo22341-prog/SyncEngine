//! Compare complete metadata scans on a temporary deep local tree, without a NAS.
use connector_local::{LocalConnector, LocalOptions};
use std::error::Error;
use std::io::Write;
use std::time::{Duration, Instant};
use sync_core::CancellationToken;
use sync_filter::Filter;
use sync_scanner::{ScanOptions, scan};

fn main() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    for branch in 0..16 {
        let mut path = temp.path().join(format!("b{branch}"));
        for level in 0..12 {
            path = path.join(format!("d{level}"));
            std::fs::create_dir_all(&path)?;
            for file in 0..8 {
                std::fs::write(path.join(format!("f{file}")), b"fixture")?;
            }
        }
    }
    let mut timings = [Duration::ZERO; 2];
    let mut expected: Option<Vec<sync_core::FileEntry>> = None;
    // Alternating order limits warm-cache bias; no file contents are hashed.
    for round in 0..4 {
        for reuse in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let options = if reuse {
                LocalOptions::new().with_handle_reuse()
            } else {
                LocalOptions::new()
            };
            let connector = LocalConnector::new(temp.path(), options)?;
            let start = Instant::now();
            let inventory = scan(
                &connector,
                &Filter::include_all(),
                &ScanOptions::new(),
                &CancellationToken::new(),
            )?;
            let elapsed = start.elapsed();
            if !inventory.is_complete() || !inventory.issues().is_empty() {
                return Err("incomplete benchmark scan".into());
            }
            if round > 0 {
                let entries: Vec<_> = inventory.entries().cloned().collect();
                if let Some(expected) = &expected {
                    if expected != &entries {
                        let mismatch = expected.iter().zip(&entries).find(|(a, b)| a != b);
                        return Err(format!("inventories differ: {mismatch:?}").into());
                    }
                } else {
                    expected = Some(entries);
                }
                timings[usize::from(reuse)] += elapsed;
            }
        }
    }
    writeln!(
        std::io::stdout().lock(),
        "entries={} uncached_mean_ms={:.1} cached_mean_ms={:.1} ratio={:.2}",
        expected.map_or(0, |e| e.len()),
        timings[0].as_secs_f64() * 1000.0 / 3.0,
        timings[1].as_secs_f64() * 1000.0 / 3.0,
        timings[0].as_secs_f64() / timings[1].as_secs_f64()
    )?;
    Ok(())
}
