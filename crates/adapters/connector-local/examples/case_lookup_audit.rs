//! Read-only Jellyfin B/b directory identity and Windows enumeration audit.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use cap_fs_ext::{DirExt, MetadataExt};
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let mut upper_first = false;
    let mut show_names = false;
    let mut paths = Vec::new();
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--upper-first" => upper_first = true,
            "--show-names" => show_names = true,
            _ => paths.push(argument),
        }
    }
    let names = if upper_first {
        ["B", "b", "C", "c", "S", "s"]
    } else {
        ["b", "B", "c", "C", "s", "S"]
    };
    for path in paths {
        let root = cap_std::fs::Dir::open_ambient_dir(&path, cap_std::ambient_authority())?;
        for name in names {
            let dir = root.open_dir_nofollow(name)?;
            let metadata = dir.dir_metadata()?;
            let reader = cap_primitives::windows_listing::HandleReadDir::new_without_file_ids(
                dir.into_std_file(),
            )?;
            let direct = reader.uses_handle_batches();
            let mut count = 0;
            let mut matched = 0;
            for entry in reader {
                let entry = entry?;
                if show_names {
                    writeln!(
                        out,
                        "entry\t{path}\t{name}\t{}",
                        entry.file_name().to_string_lossy()
                    )?;
                }
                count += 1;
                if entry.file_name().to_string_lossy().starts_with("bbno$") {
                    matched += 1;
                }
            }
            writeln!(
                out,
                "{path} {name}: identity={}:{} direct={direct} entries={count} bbno_prefix={matched}",
                metadata.dev(),
                metadata.ino()
            )?;
        }
    }
    Ok(())
}
#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("Windows required".into())
}
