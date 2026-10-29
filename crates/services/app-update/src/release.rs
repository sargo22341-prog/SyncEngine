//! Release text: notes and the SHA-256 block appended by the CI.
//!
//! `.github/workflows/ci.yml` writes the notes (`RELEASE_NOTES.md`), then:
//!
//! ````text
//! ### Empreintes SHA-256
//!
//! ```
//! <64 hex>  SyncEngine_<version>_<target>.zip
//! <64 hex>  SyncEngine_<version>_<target>.tar.gz
//! ```
//! ````

use crate::error::{ErrorKind, UpdateError, fail};
use crate::version::short;

const HEADING: &str = "### Empreintes SHA-256";
const FENCE: &str = "```";

/// Release notes without the checksum block (whole text when it is absent).
#[must_use]
pub fn release_notes(body: &str) -> String {
    let notes = match heading_offset(body) {
        Some(at) => &body[..at],
        None => body,
    };
    notes.replace("\r\n", "\n").trim().to_owned()
}

fn heading_offset(body: &str) -> Option<usize> {
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if line.trim() == HEADING {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn missing(reason: &str) -> UpdateError {
    fail(
        ErrorKind::ChecksumMissing,
        format!("empreinte SHA-256 de la release inutilisable : {reason}"),
    )
}

/// Every `(file name, SHA-256)` pair of the checksum block (`sha256sum`
/// format: 64 hexadecimal digits, two spaces or ` *`, file name).
///
/// # Errors
/// [`ErrorKind::ChecksumMissing`] when the heading or its fenced block is
/// absent, empty or unterminated, or when one line is malformed or a name is
/// repeated: the whole block is then refused.
pub fn parse_checksums(body: &str) -> Result<Vec<(String, [u8; 32])>, UpdateError> {
    let start = heading_offset(body).ok_or_else(|| missing("bloc absent"))?;
    let mut lines = body[start..].lines().skip(1).map(str::trim);
    let opening = lines.find(|line| !line.is_empty());
    if opening != Some(FENCE) {
        return Err(missing("bloc de code attendu après le titre"));
    }
    let mut sums: Vec<(String, [u8; 32])> = Vec::new();
    let mut closed = false;
    for line in lines {
        if line == FENCE {
            closed = true;
            break;
        }
        if line.is_empty() {
            continue;
        }
        let (name, digest) = parse_line(line)?;
        if sums.iter().any(|(known, _)| *known == name) {
            return Err(missing(&format!("nom répété « {} »", short(&name))));
        }
        sums.push((name, digest));
    }
    if !closed {
        return Err(missing("bloc non terminé"));
    }
    if sums.is_empty() {
        return Err(missing("bloc vide"));
    }
    Ok(sums)
}

fn parse_line(line: &str) -> Result<(String, [u8; 32]), UpdateError> {
    let malformed = || missing(&format!("ligne mal formée « {} »", short(line)));
    let (hex, rest) = line.split_at_checked(64).ok_or_else(malformed)?;
    let name = rest
        .strip_prefix("  ")
        .or_else(|| rest.strip_prefix(" *"))
        .ok_or_else(malformed)?;
    if name.is_empty() || name.contains(['/', '\\']) || name.trim() != name {
        return Err(malformed());
    }
    // Checked first: `from_str_radix` alone would accept a leading `+`.
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(malformed());
    }
    let mut digest = [0_u8; 32];
    for (byte, pair) in digest.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
        let text = std::str::from_utf8(pair).map_err(|_| malformed())?;
        *byte = u8::from_str_radix(text, 16).map_err(|_| malformed())?;
    }
    Ok((name.to_owned(), digest))
}

/// Published SHA-256 of `asset`.
///
/// # Errors
/// [`ErrorKind::ChecksumMissing`] when the block is unusable (see
/// [`parse_checksums`]) or does not list `asset`.
pub fn checksum_for(body: &str, asset: &str) -> Result<[u8; 32], UpdateError> {
    parse_checksums(body)?
        .into_iter()
        .find(|(name, _)| name == asset)
        .map(|(_, digest)| digest)
        .ok_or_else(|| missing(&format!("« {} » n’y figure pas", short(asset))))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZIP: &str = "SyncEngine_0.1.9_x86_64-pc-windows-msvc.zip";
    const HASH: &str = "0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789abcdef";

    fn body(block: &str) -> String {
        format!("- Correction A\r\n- Ajout B\r\n\r\n### Empreintes SHA-256\r\n\r\n{block}")
    }

    #[test]
    fn present_block_is_read_and_notes_exclude_it() -> Result<(), UpdateError> {
        let text = body(&format!(
            "```\n{HASH}  {ZIP}\n{HASH} *SyncEngine_0.1.9_x86_64-unknown-linux-gnu.tar.gz\n```\n"
        ));
        let digest = checksum_for(&text, ZIP)?;
        assert_eq!(digest[0], 0x01);
        assert_eq!(digest[15], 0xef);
        assert_eq!(parse_checksums(&text)?.len(), 2);
        assert_eq!(release_notes(&text), "- Correction A\n- Ajout B");
        Ok(())
    }

    #[test]
    fn absent_block_is_refused_and_notes_are_whole_text() {
        let text = "- Seulement des notes";
        assert_eq!(
            checksum_for(text, ZIP).map_err(|e| e.kind()),
            Err(ErrorKind::ChecksumMissing)
        );
        assert_eq!(release_notes(text), text);
    }

    #[test]
    fn malformed_blocks_are_refused() {
        let cases = [
            String::new(),
            "pas de bloc".to_owned(),
            format!("```\n{HASH}  {ZIP}\n"),
            "```\n```\n".to_owned(),
            format!("```\n{}  {ZIP}\n```", &HASH[..63]),
            format!("```\n{}g  {ZIP}\n```", &HASH[..63]),
            format!("```\n{HASH} {ZIP}\n```"),
            format!("```\n{HASH}  ../{ZIP}\n```"),
            format!("```\n{HASH}  {ZIP}\n{HASH}  {ZIP}\n```"),
        ];
        for block in cases {
            assert_eq!(
                checksum_for(&body(&block), ZIP).map_err(|e| e.kind()),
                Err(ErrorKind::ChecksumMissing),
                "{block:?}"
            );
        }
    }

    #[test]
    fn missing_file_name_is_refused() {
        let text = body(&format!("```\n{HASH}  autre.zip\n```"));
        let error = checksum_for(&text, ZIP).err();
        assert_eq!(error.map(|e| e.kind()), Some(ErrorKind::ChecksumMissing));
    }
}
