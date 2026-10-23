//! Caviardage défensif des secrets dans les textes de diagnostic.
//!
//! Ce filtre est une **défense en profondeur**, pas une garantie : la règle
//! principale reste de ne jamais placer de secret dans un message d'erreur.
//! Il masque :
//! - l'information d'identification d'une URL (`scheme://user:pass@hôte`) ;
//! - la requête et le fragment d'une URL (URL signées, jetons OAuth) ;
//! - le mot suivant `Bearer` ou `Basic` ;
//! - la valeur d'une clé sensible (`password=…`, `token: …`, `api_key=…`…).

/// Texte de remplacement d'une valeur caviardée.
pub const REDACTED: &str = "***";

const SENSITIVE_KEYS: &[&str] = &[
    "access_token",
    "api_key",
    "apikey",
    "authorization",
    "client_secret",
    "passwd",
    "password",
    "pwd",
    "refresh_token",
    "secret",
    "sig",
    "signature",
    "token",
    "x-amz-signature",
];

const AUTH_SCHEMES: &[&str] = &["bearer", "basic"];

fn is_value_end(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b'&' | b';' | b',' | b'"' | b'\'')
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn value_end(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|&b| is_value_end(b))
        .map_or(bytes.len(), |offset| start + offset)
}

/// Remplace par [`REDACTED`] les secrets reconnus dans `text`.
#[must_use]
pub fn redact_secrets(text: &str) -> String {
    let bytes = text.as_bytes();
    // `to_ascii_lowercase` conserve les positions d'octets.
    let lower = text.to_ascii_lowercase();
    let lower = lower.as_bytes();
    let mut masks: Vec<(usize, usize)> = Vec::new();

    collect_url_masks(bytes, &mut masks);
    collect_keyword_masks(lower, &mut masks);

    if masks.is_empty() {
        return text.to_owned();
    }
    masks.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in masks {
        if end <= cursor {
            continue;
        }
        let start = start.max(cursor);
        // Toutes les bornes sont adjacentes à un octet ASCII : ce sont des
        // frontières de caractères UTF-8 valides.
        out.push_str(&text[cursor..start]);
        out.push_str(REDACTED);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn collect_url_masks(bytes: &[u8], masks: &mut Vec<(usize, usize)>) {
    let mut search = 0;
    while let Some(offset) = find(&bytes[search..], b"://") {
        let authority_start = search + offset + 3;
        let url_end = bytes[authority_start..]
            .iter()
            .position(|b| b.is_ascii_whitespace() || matches!(b, b'"' | b'\'' | b'<' | b'>'))
            .map_or(bytes.len(), |o| authority_start + o);
        let authority_end = bytes[authority_start..url_end]
            .iter()
            .position(|b| matches!(b, b'/' | b'?' | b'#'))
            .map_or(url_end, |o| authority_start + o);
        if let Some(at) = bytes[authority_start..authority_end]
            .iter()
            .rposition(|&b| b == b'@')
        {
            masks.push((authority_start, authority_start + at));
        }
        if let Some(q) = bytes[authority_end..url_end]
            .iter()
            .position(|b| matches!(b, b'?' | b'#'))
        {
            let query_start = authority_end + q + 1;
            if query_start < url_end {
                masks.push((query_start, url_end));
            }
        }
        search = url_end.max(authority_start);
    }
}

fn collect_keyword_masks(lower: &[u8], masks: &mut Vec<(usize, usize)>) {
    for key in SENSITIVE_KEYS {
        for start in word_occurrences(lower, key.as_bytes()) {
            let mut i = start + key.len();
            while i < lower.len() && lower[i] == b' ' {
                i += 1;
            }
            if i < lower.len() && matches!(lower[i], b'=' | b':') {
                i += 1;
                while i < lower.len() && lower[i] == b' ' {
                    i += 1;
                }
                let end = value_end(lower, i);
                if end > i {
                    masks.push((i, end));
                }
            }
        }
    }
    for scheme in AUTH_SCHEMES {
        for start in word_occurrences(lower, scheme.as_bytes()) {
            let mut i = start + scheme.len();
            if i >= lower.len() || lower[i] != b' ' {
                continue;
            }
            while i < lower.len() && lower[i] == b' ' {
                i += 1;
            }
            let end = value_end(lower, i);
            if end > i {
                masks.push((i, end));
            }
        }
    }
}

/// Positions où `needle` apparaît comme mot entier (non précédé ni suivi
/// d'un caractère de mot).
fn word_occurrences<'a>(haystack: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    (0..haystack
        .len()
        .saturating_sub(needle.len())
        .saturating_add(1))
        .filter(move |&i| haystack[i..].starts_with(needle))
        .filter(move |&i| i == 0 || !is_word_byte(haystack[i - 1]))
        .filter(move |&i| {
            haystack
                .get(i + needle.len())
                .is_none_or(|&b| !is_word_byte(b))
        })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_plain_text_untouched() {
        let text = "échec de lecture : docs/rapport.txt (accès refusé)";
        assert_eq!(redact_secrets(text), text);
        assert_eq!(redact_secrets(""), "");
    }

    #[test]
    fn masks_url_userinfo() {
        assert_eq!(
            redact_secrets("connexion à sftp://alice:hunter2@nas.local:22/backup refusée"),
            "connexion à sftp://***@nas.local:22/backup refusée"
        );
    }

    #[test]
    fn masks_signed_url_query_and_fragment() {
        assert_eq!(
            redact_secrets(
                "GET https://bucket.s3.example/obj?X-Amz-Signature=abc&X-Amz-Credential=k 403"
            ),
            "GET https://bucket.s3.example/obj?*** 403"
        );
        assert_eq!(
            redact_secrets("redirect https://app/cb#access_token=xyz"),
            "redirect https://app/cb#***"
        );
    }

    #[test]
    fn masks_authorization_schemes() {
        assert_eq!(
            redact_secrets("header Authorization: Bearer eyJhbGciOi.payload.sig"),
            "header Authorization: *** ***"
        );
        assert_eq!(redact_secrets("basic dXNlcjpwYXNz"), "basic ***");
    }

    #[test]
    fn masks_sensitive_keys() {
        assert_eq!(
            redact_secrets("password=hunter2 user=bob"),
            "password=*** user=bob"
        );
        assert_eq!(
            redact_secrets("client_secret: s3cr3t; api_key = k-123"),
            "client_secret: ***; api_key = ***"
        );
        assert_eq!(redact_secrets("TOKEN=Abc"), "TOKEN=***");
    }

    #[test]
    fn does_not_match_inside_words() {
        let text = "tokenizer=on passwordless=yes";
        assert_eq!(redact_secrets(text), text);
    }

    #[test]
    fn preserves_non_ascii_around_masks() {
        assert_eq!(
            redact_secrets("mot de passe → password=pâté ✓"),
            "mot de passe → password=*** ✓"
        );
    }
}
