use crate::webdav::{WebDavConnector, error, status_kind};
use connector_api::{
    CommitMode, StorageConnector, StorageError, StorageErrorKind as Kind, StorageFeature,
    StorageOperation as Op, WriteOptions,
};
use connector_staging::RemoteStage;
use reqwest::{
    Method,
    blocking::{RequestBuilder, Response},
    header::{AUTHORIZATION, ETAG, IF_MATCH},
};
use std::fs::File;
use sync_core::{CancellationToken, EntryKind, FileEntry, ResourcePath};

impl WebDavConnector {
    fn mutation(
        &self,
        method: &str,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<RequestBuilder, StorageError> {
        crate::paging::check(cancel, Op::Write, path)?;
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|_| error(Kind::Permanent, Op::Write, path))?;
        Ok(self
            .client
            .request(method, self.url(path)?)
            .header(AUTHORIZATION, self.auth.clone()))
    }
    fn send_mutation(
        &self,
        builder: RequestBuilder,
        path: &ResourcePath,
        expected: &[u16],
    ) -> Result<Response, StorageError> {
        // Never retry a mutation: lost replies may mean that it already happened.
        let response = builder.send().map_err(|_| {
            error(Kind::Disconnected, Op::Write, path)
                .with_message("résultat distant incertain ; diagnostiquer avant reprise")
        })?;
        if !expected.contains(&response.status().as_u16()) {
            return Err(error(
                status_kind(
                    response.status().as_u16(),
                    response.headers().get("Retry-After"),
                ),
                Op::Write,
                path,
            ));
        }
        Ok(response)
    }
    fn strong_etag(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<String, StorageError> {
        let response = self.send_mutation(self.mutation("HEAD", path, cancel)?, path, &[200])?;
        let tag = response
            .headers()
            .get(ETAG)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| error(Kind::NotSupported, Op::Write, path))?;
        if tag.len() < 2
            || !tag.starts_with('"')
            || !tag.ends_with('"')
            || tag[1..tag.len() - 1].contains(['"', '\r', '\n'])
        {
            return Err(error(Kind::NotSupported, Op::Write, path));
        }
        Ok(tag.to_owned())
    }
}

pub(super) fn mkdir(
    c: &WebDavConnector,
    path: &ResourcePath,
    cancel: &CancellationToken,
) -> Result<FileEntry, StorageError> {
    connector_api::ensure_supported(c, StorageFeature::CreateDirectory, Op::CreateDirectory)?;
    let parent = path
        .parent()
        .ok_or_else(|| error(Kind::Conflict, Op::CreateDirectory, path))?;
    if c.stat(&parent, cancel)?.kind() != EntryKind::Directory {
        return Err(error(Kind::Conflict, Op::CreateDirectory, path));
    }
    c.send_mutation(c.mutation("MKCOL", path, cancel)?, path, &[201])?;
    c.stat(path, cancel)
}

pub(super) fn delete(
    c: &WebDavConnector,
    path: &ResourcePath,
    expected: EntryKind,
    cancel: &CancellationToken,
) -> Result<(), StorageError> {
    connector_api::ensure_supported(c, StorageFeature::Delete, Op::Delete)?;
    // DAV DELETE on a collection is recursive; the public nonrecursive contract
    // cannot be implemented by checking an empty listing then deleting it.
    if expected != EntryKind::File || path.is_root() {
        return Err(error(Kind::NotSupported, Op::Delete, path));
    }
    if c.stat(path, cancel)?.kind() != EntryKind::File {
        return Err(error(Kind::Conflict, Op::Delete, path));
    }
    let tag = c.strong_etag(path, cancel)?;
    c.send_mutation(
        c.mutation("DELETE", path, cancel)?.header(IF_MATCH, tag),
        path,
        &[204],
    )?;
    Ok(())
}

impl RemoteStage for WebDavConnector {
    fn reserve(&self, path: &ResourcePath, cancel: &CancellationToken) -> Result<(), StorageError> {
        mkdir(self, path, cancel).map(|_| ())
    }
    fn upload(
        &self,
        path: &ResourcePath,
        file: File,
        _options: WriteOptions,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        let size = file
            .metadata()
            .map_err(|_| error(Kind::Permanent, Op::Write, path))?
            .len();
        self.send_mutation(
            self.mutation("PUT", path, cancel)?
                .header("If-None-Match", "*")
                .body(reqwest::blocking::Body::sized(file, size)),
            path,
            &[201],
        )?;
        crate::paging::check(cancel, Op::Write, path)
    }
    fn publish(
        &self,
        stage: &ResourcePath,
        target: &ResourcePath,
        mode: CommitMode,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        let parent = target
            .parent()
            .ok_or_else(|| error(Kind::Conflict, Op::Move, target))?;
        if self.stat(&parent, cancel)?.kind() != EntryKind::Directory {
            return Err(error(Kind::Conflict, Op::Move, target));
        }
        let destination = self.url(target)?.to_string();
        let mut request = self
            .mutation("MOVE", stage, cancel)?
            .header("Destination", &destination);
        match mode {
            CommitMode::CreateNew => request = request.header("Overwrite", "F"),
            CommitMode::ReplaceExisting => {
                if self.stat(target, cancel)?.kind() != EntryKind::File {
                    return Err(error(Kind::Conflict, Op::Move, target));
                }
                let tag = self.strong_etag(target, cancel)?;
                request = request
                    .header("Overwrite", "T")
                    .header("If", format!("<{destination}> ([{tag}])"));
            }
            _ => return Err(error(Kind::NotSupported, Op::Move, target)),
        }
        self.send_mutation(request, target, &[201, 204])?;
        self.stat(target, cancel)
    }
    fn cleanup(&self, stage: &ResourcePath, dir: &ResourcePath) -> Result<(), StorageError> {
        let cancel = CancellationToken::new();
        match delete(self, stage, EntryKind::File, &cancel) {
            Ok(()) => {}
            Err(e) if e.kind() == Kind::NotFound => {}
            Err(e) => return Err(e),
        }
        // Only our exclusive random staging collection, never a user collection.
        let page = self.list(dir, None, &cancel)?;
        if !page.entries().is_empty() || page.next_page().is_some() || !page.issues().is_empty() {
            return Err(error(Kind::Conflict, Op::Delete, dir));
        }
        let tag = self.strong_etag(dir, &cancel)?;
        self.send_mutation(
            self.mutation("DELETE", dir, &cancel)?.header(IF_MATCH, tag),
            dir,
            &[204],
        )?;
        Ok(())
    }
}
