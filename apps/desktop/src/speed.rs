//! Live throughput, computed like FreeFileSync's `SpeedTest`
//! (`base/speed_test.cpp`): over a sliding time window, rate = (last count −
//! first count) / (last time − first time), keeping one sample before the
//! window start so that a gap between two observations is bridged instead of
//! read as a stop. The first second is skipped (startup numbers are noisy).
//!
//! Each curve point is the windowed rate at one sample time, never the delta
//! between two consecutive 200 ms polls: a counter that moves in bursts (a
//! file validated at once, a buffered write) no longer draws 0, then a spike.

use std::time::{Duration, Instant};

/// Averaging window (FreeFileSync: `PERF_WINDOW_BYTES_PER_SEC`, 4 s).
pub(crate) const WINDOW: Duration = Duration::from_secs(4);
/// Time span drawn by the curve.
pub(crate) const SPAN: Duration = Duration::from_secs(30);
/// Samples older than this are no longer needed (span + one window).
pub(crate) const RETAIN: Duration = Duration::from_secs(34);
/// Rates are not estimated during the first second of a job.
const STARTUP_SKIP: Duration = Duration::from_secs(1);

/// Windowed rate per second at each sample of the last [`SPAN`], oldest
/// first. `samples` are `(time, cumulative count)` in time order, the first
/// one being taken when the job started (or the oldest retained one).
/// A count below the window's first one is read as no progress.
pub(crate) fn rates(samples: &[(Instant, u64)], started: Instant) -> Vec<f32> {
    let Some(&(last, _)) = samples.last() else {
        return Vec::new();
    };
    let shown_from = last.checked_sub(SPAN);
    let mut reference = 0;
    let mut out = Vec::new();
    for (index, &(at, count)) in samples.iter().enumerate() {
        // Latest sample at least one window older than `at` (or the oldest).
        while reference + 1 < index && samples[reference + 1].0 + WINDOW <= at {
            reference += 1;
        }
        if at.saturating_duration_since(started) < STARTUP_SKIP
            || shown_from.is_some_and(|from| at < from)
        {
            continue;
        }
        let (since, base) = samples[reference];
        let elapsed = at.saturating_duration_since(since).as_secs_f32();
        if index == reference || elapsed <= 0.0 {
            continue;
        }
        out.push(count.saturating_sub(base) as f32 / elapsed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(start: Instant, step_ms: u64, counts: &[u64]) -> Vec<(Instant, u64)> {
        counts
            .iter()
            .enumerate()
            .map(|(i, c)| (start + Duration::from_millis(step_ms * i as u64), *c))
            .collect()
    }

    #[test]
    fn steady_transfer_gives_a_flat_curve() {
        let start = Instant::now();
        // 10 MB/s polled every 200 ms for 10 s.
        let counts: Vec<u64> = (0..=50).map(|i| i * 2_000_000).collect();
        let values = rates(&series(start, 200, &counts), start);
        assert!(!values.is_empty());
        for v in values {
            assert!((v - 10_000_000.0).abs() < 1.0, "{v}");
        }
    }

    /// Given a counter that only moves in bursts (one 4 MB jump every
    /// second, nothing in between), When rates are computed, Then the curve
    /// stays near the true 4 MB/s average instead of alternating 0 and 20 MB/s.
    #[test]
    fn bursts_are_averaged_over_the_window() {
        let start = Instant::now();
        let counts: Vec<u64> = (0..=60).map(|i| (i / 5) * 4_000_000).collect();
        let values = rates(&series(start, 200, &counts), start);
        // Once a full window is available, every point is within ±25 %.
        let settled: Vec<f32> = values.iter().copied().skip(20).collect();
        assert!(!settled.is_empty());
        for v in settled {
            assert!((3_000_000.0..=5_000_000.0).contains(&v), "{v}");
        }
        assert!(values.iter().all(|v| *v > 0.0));
    }

    #[test]
    fn startup_second_is_skipped_and_gaps_are_bridged() {
        let start = Instant::now();
        // Polls stall for 3 s (no samples), progress continued meanwhile.
        let samples = vec![
            (start, 0),
            (start + Duration::from_millis(500), 500),
            (start + Duration::from_millis(1_000), 1_000),
            (start + Duration::from_millis(4_000), 4_000),
        ];
        let values = rates(&samples, start);
        // 500 ms is skipped; 1 s and 4 s are both 1 000/s, no spike.
        assert_eq!(values.len(), 2);
        for v in values {
            assert!((v - 1_000.0).abs() < 0.01, "{v}");
        }
    }

    #[test]
    fn only_the_last_span_is_drawn_and_regressions_read_as_zero() {
        let start = Instant::now();
        let counts: Vec<u64> = (0..=200).map(|i| i * 100).collect();
        let values = rates(&series(start, 200, &counts), start);
        // 40 s of samples, 30 s drawn ⇒ 151 points (inclusive bounds).
        assert_eq!(values.len(), 151);
        let back = vec![(start, 100), (start + Duration::from_secs(2), 50)];
        let values = rates(&back, start);
        assert_eq!(values.last().copied(), Some(0.0));
        assert!(rates(&[], start).is_empty());
        assert!(rates(&[(start, 5)], start).is_empty());
    }
}
