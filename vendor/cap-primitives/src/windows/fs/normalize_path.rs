// SYNCENGINE-PATCH: tested directly by connector-local's Windows unit tests.
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf, Prefix};

pub(super) fn normalize(path: &Path) -> PathBuf {
    let wide: Vec<_> = path.as_os_str().encode_wide().collect();
    match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimUNC(_, _) => {
                // \\?\UNC\server\share -> \\server\share, never UNC\server\share.
                let normal: Vec<_> = [b'\\' as u16, b'\\' as u16]
                    .into_iter()
                    .chain(wide[8..].iter().copied())
                    .collect();
                PathBuf::from(OsString::from_wide(&normal))
            }
            Prefix::VerbatimDisk(_) => PathBuf::from(OsString::from_wide(&wide[4..])),
            // Other device paths have no equivalent prefix we can safely strip.
            _ => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unc_root_and_nested_paths_remain_absolute() {
        for (input, expected) in [
            (r"\\?\UNC\server\share", r"\\server\share"),
            (r"\\?\UNC\server\share\folder", r"\\server\share\folder"),
            (r"\\?\UNC\server\share\été", r"\\server\share\été"),
            (r"\\server\share", r"\\server\share"),
            (r"\\?\C:\folder", r"C:\folder"),
            (r"C:\folder", r"C:\folder"),
        ] {
            let result = normalize(Path::new(input));
            assert_eq!(result, Path::new(expected));
            assert!(result.is_absolute());
            assert!(result.join("child").is_absolute());
        }
    }

    #[test]
    fn device_paths_and_non_unicode_names_are_preserved() {
        let device = Path::new(r"\\?\Volume{00000000-0000-0000-0000-000000000000}\folder");
        assert_eq!(normalize(device), device);
        let mut input: Vec<_> = r"\\?\UNC\server\share\".encode_utf16().collect();
        input.push(0xd800);
        let input = PathBuf::from(OsString::from_wide(&input));
        let normalized = normalize(&input);
        assert_eq!(normalized.as_os_str().encode_wide().last(), Some(0xd800));
        assert!(normalized.is_absolute());
    }
}
