use crate::webdav::error;
use connector_api::{StorageError, StorageErrorKind as Kind, StorageOperation as Op};
use reqwest::Url;
use std::collections::BTreeSet;
use sync_core::{EntryKind, FileEntry, FileMetadata, ResourcePath};

fn dav(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    node.has_tag_name(("DAV:", name))
}

pub(super) fn decoded_path(path: &str) -> Result<ResourcePath, ()> {
    let path = path
        .strip_prefix('/')
        .ok_or(())?
        .strip_suffix('/')
        .unwrap_or(path.strip_prefix('/').ok_or(())?);
    if path.is_empty() {
        return Ok(ResourcePath::root());
    }
    let mut result = ResourcePath::root();
    for segment in path.split('/') {
        let decoded = percent_encoding::percent_decode_str(segment)
            .decode_utf8()
            .map_err(|_| ())?;
        // Reject double encodings to avoid a server decoding a second time.
        if decoded.contains('%') {
            return Err(());
        }
        result = result.join(&decoded).map_err(|_| ())?;
    }
    Ok(result)
}

fn href_path(root: &Url, href: &str) -> Result<ResourcePath, ()> {
    // Validate the raw path before Url can normalize dot segments.
    let raw = if href.starts_with('/') && !href.starts_with("//") {
        if href.contains(['?', '#']) {
            return Err(());
        }
        href
    } else {
        let scheme = href.find("://").ok_or(())?;
        let start = href[scheme + 3..]
            .find('/')
            .map(|n| n + scheme + 3)
            .ok_or(())?;
        &href[start..]
    };
    let decoded = decoded_path(raw)?;
    let url = root.join(href).map_err(|_| ())?;
    if url.origin() != root.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    let base = decoded_path(root.path())?;
    let stripped = decoded.as_str().strip_prefix(base.as_str()).ok_or(())?;
    if base.is_root() {
        return Ok(decoded);
    }
    if stripped.is_empty() {
        return Ok(ResourcePath::root());
    }
    ResourcePath::parse(stripped.strip_prefix('/').ok_or(())?).map_err(|_| ())
}

pub(super) fn parse(
    root: &Url,
    requested: &ResourcePath,
    depth: &str,
    text: &str,
) -> Result<Vec<FileEntry>, StorageError> {
    let invalid = || {
        error(Kind::Permanent, Op::List, requested)
            .with_message("rÃ©ponse DAV invalide ou incomplÃ¨te")
    };
    let doc = roxmltree::Document::parse(text).map_err(|_| invalid())?;
    let multi = doc.root_element();
    if !dav(multi, "multistatus") {
        return Err(invalid());
    }
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    let mut found_self = false;
    for response in multi.children().filter(|n| n.is_element()) {
        if !dav(response, "response") {
            return Err(invalid());
        }
        let hrefs: Vec<_> = response.children().filter(|n| dav(*n, "href")).collect();
        if hrefs.len() != 1 {
            return Err(invalid());
        }
        let path = href_path(root, hrefs[0].text().ok_or_else(invalid)?).map_err(|_| invalid())?;
        if !seen.insert(path.clone()) {
            return Err(invalid());
        }
        if path != *requested && (depth == "0" || path.parent().as_ref() != Some(requested)) {
            return Err(invalid());
        }
        let mut kind = None;
        let mut size = None;
        let mut modified = None;
        for propstat in response.children().filter(|n| dav(*n, "propstat")) {
            let status = propstat
                .children()
                .find(|n| dav(*n, "status"))
                .and_then(|n| n.text())
                .ok_or_else(invalid)?;
            let code = status
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u16>().ok())
                .ok_or_else(invalid)?;
            if code != 200 {
                continue;
            } // Missing required properties still fails below.
            let prop = propstat
                .children()
                .find(|n| dav(*n, "prop"))
                .ok_or_else(invalid)?;
            for property in prop.children().filter(|n| n.is_element()) {
                if dav(property, "resourcetype") {
                    if kind.is_some() {
                        return Err(invalid());
                    }
                    let children: Vec<_> = property.children().filter(|n| n.is_element()).collect();
                    kind = Some(if children.is_empty() {
                        EntryKind::File
                    } else if children.len() == 1 && dav(children[0], "collection") {
                        EntryKind::Directory
                    } else {
                        EntryKind::Other
                    });
                } else if dav(property, "getcontentlength") {
                    size = Some(
                        property
                            .text()
                            .ok_or_else(invalid)?
                            .parse::<u64>()
                            .map_err(|_| invalid())?,
                    );
                } else if dav(property, "getlastmodified") {
                    if modified.is_some() {
                        return Err(invalid());
                    }
                    let value = property.text().ok_or_else(invalid)?;
                    if value.len() > 128 {
                        return Err(invalid());
                    }
                    let date =
                        chrono::DateTime::parse_from_rfc2822(value).map_err(|_| invalid())?;
                    modified = Some(
                        sync_core::Timestamp::new(date.timestamp(), date.timestamp_subsec_nanos())
                            .map_err(|_| invalid())?,
                    );
                }
            }
        }
        let kind = kind.ok_or_else(invalid)?;
        if path == *requested {
            found_self = true;
            if depth == "1" && kind != EntryKind::Directory {
                return Err(invalid());
            }
        }
        let mut metadata = FileMetadata::unknown();
        if let Some(size) = size {
            metadata = metadata.with_size(size);
        }
        if let Some(modified) = modified {
            metadata = metadata.with_modified(modified, sync_core::TimestampPrecision::Second);
        }
        entries.push(FileEntry::new(path, kind, metadata));
        if entries.len() > 10_001 {
            return Err(invalid());
        }
    }
    if !found_self {
        return Err(invalid());
    }
    Ok(entries)
}

#[cfg(test)]
mod inventory_tests {
    use super::*;

    #[test]
    fn successful_date_property_is_validated_without_extra_requests() {
        let root = Url::parse("https://localhost/root/").unwrap();
        for date in ["Sun, 25 Oct 2026 12:00:00 GMT", "invalid"] {
            let xml = format!(
                "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/root/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype><d:getlastmodified>{date}</d:getlastmodified></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"
            );
            assert_eq!(
                parse(&root, &ResourcePath::root(), "1", &xml).is_ok(),
                date != "invalid"
            );
        }
    }
}
