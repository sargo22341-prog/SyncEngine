//! Read-only Windows SMB diagnostics, without logging file names or contents.
use std::error::Error;
use std::io::Write;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
fn main() -> Result<(), Box<dyn Error>> {
    use cap_fs_ext::DirExt;
    use cap_primitives::windows_listing::HandleReadDir;
    use cap_std::fs::Dir;

    let mut args = std::env::args().skip(1);
    let root =
        std::path::PathBuf::from(args.next().ok_or("usage: directory_timing ROOT [rounds]")?);
    let rounds: u32 = args.next().map_or(Ok(100), |s| s.parse())?;
    if rounds == 0 {
        return Err("rounds must be positive".into());
    }
    let pinned = Dir::open_ambient_dir(&root, cap_std::ambient_authority())?;
    let child = pinned
        .entries()?
        .find_map(|e| {
            let e = e.ok()?;
            let m = e.metadata().ok()?;
            (m.is_dir() && !m.file_type().is_symlink()).then(|| e.file_name())
        })
        .ok_or("root must contain an ordinary directory")?;
    let mut totals = [Duration::ZERO; 5];
    let mut direct = 0;
    let mut count = 0;
    for _ in 0..rounds {
        let start = Instant::now();
        let current = Dir::open_ambient_dir(&root, cap_std::ambient_authority())?;
        totals[0] += start.elapsed();
        let start = Instant::now();
        let _metadata = current.dir_metadata()?;
        totals[1] += start.elapsed();
        let start = Instant::now();
        let directory = pinned.open_dir_nofollow(&child)?;
        totals[2] += start.elapsed();
        let start = Instant::now();
        let reader = HandleReadDir::new_without_file_ids(directory.into_std_file())?;
        direct += u32::from(reader.uses_handle_batches());
        for entry in reader {
            let _entry = entry?;
            count += 1;
        }
        totals[3] += start.elapsed();
        let start = Instant::now();
        let _identity =
            cap_primitives::windows_listing::root_identity(&root, cap_std::ambient_authority())?;
        totals[4] += start.elapsed();
    }
    let mut out = std::io::stdout().lock();
    for (name, total) in [
        "root open",
        "root identity",
        "child nofollow open",
        "child listing",
        "root single-query identity",
    ]
    .into_iter()
    .zip(totals)
    {
        writeln!(
            out,
            "{name}: total {total:.3?}, mean {:.3?}",
            total / rounds
        )?;
    }
    writeln!(
        out,
        "{rounds} rounds, {direct} direct listings, {count} entries"
    )?;
    Ok(())
}

#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn Error>> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "This diagnostic requires Windows.")?;
    Ok(())
}
