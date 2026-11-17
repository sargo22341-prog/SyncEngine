use crate::webdav::{WebDavConnector, error, status_kind};
use connector_api::{
    CommitMode, StorageConnector, StorageError, StorageErrorKind as Kind, StorageFeature,
    StorageOperation as Op, WriteOptions,
};
use connector_staging::RemoteStage;
use reqwest::{
    Method, Url,
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
        self.mutation_at(method, self.url(path)?, path, cancel)
    }
    fn mutation_at(
        &self,
        method: &str,
        url: Url,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<RequestBuilder, StorageError> {
        crate::paging::check(cancel, Op::Write, path)?;
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|_| error(Kind::Permanent, Op::Write, path))?;
        Ok(self
            .client
            .request(method, url)
            .header(AUTHORIZATION, self.auth.clone()))
    }
    /// Envoie une mutation sans corps ; une réponse `301`/`308` vers la même
    /// URL suivie de `/` (collection Apache) prouve qu'elle n'a **pas** été
    /// traitée : elle est réémise une seule fois vers cette URL. Toute autre
    /// redirection reste une erreur ; aucune autre reprise n'est faite.
    fn send_to(
        &self,
        method: &str,
        url: Url,
        path: &ResourcePath,
        cancel: &CancellationToken,
        decorate: &dyn Fn(RequestBuilder) -> RequestBuilder,
        expected: &[u16],
    ) -> Result<Response, StorageError> {
        let first = decorate(self.mutation_at(method, url.clone(), path, cancel)?)
            .send()
            .map_err(|_| uncertain(path))?;
        let response = match crate::webdav::slash_redirect(&url, &first) {
            Some(target) => {
                drop(first);
                decorate(self.mutation_at(method, target, path, cancel)?)
                    .send()
                    .map_err(|_| uncertain(path))?
            }
            None => first,
        };
        check_status(response, path, expected)
    }
    fn send_mutation(
        &self,
        builder: RequestBuilder,
        path: &ResourcePath,
        expected: &[u16],
    ) -> Result<Response, StorageError> {
        // Never retry a mutation: lost replies may mean that it already happened.
        let response = builder.send().map_err(|_| uncertain(path))?;
        check_status(response, path, expected)
    }
    fn strong_etag(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<String, StorageError> {
        let response = self.send_to(
            "HEAD",
            self.url(path)?,
            path,
            cancel,
            &|request| request,
            &[200],
        )?;
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
    // Barre finale : une collection (RFC 4918), adressée comme le serveur
    // la redirigera ensuite (Apache `DirectorySlash`).
    c.send_to(
        "MKCOL",
        c.collection_url(path)?,
        path,
        cancel,
        &|request| request,
        &[201],
    )?;
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
    c.send_to(
        "DELETE",
        c.url(path)?,
        path,
        cancel,
        &|request| request.header(IF_MATCH, tag.clone()),
        &[204],
    )?;
    Ok(())
}

fn uncertain(path: &ResourcePath) -> StorageError {
    error(Kind::Disconnected, Op::Write, path)
        .with_message("résultat distant incertain ; diagnostiquer avant reprise")
}

fn check_status(
    response: Response,
    path: &ResourcePath,
    expected: &[u16],
) -> Result<Response, StorageError> {
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
        let condition = match mode {
            CommitMode::CreateNew => None,
            CommitMode::ReplaceExisting => {
                if self.stat(target, cancel)?.kind() != EntryKind::File {
                    return Err(error(Kind::Conflict, Op::Move, target));
                }
                let tag = self.strong_etag(target, cancel)?;
                Some(format!("<{destination}> ([{tag}])"))
            }
            _ => return Err(error(Kind::NotSupported, Op::Move, target)),
        };
        let decorate = |request: RequestBuilder| {
            let request = request.header("Destination", &destination);
            match &condition {
                None => request.header("Overwrite", "F"),
                Some(condition) => request
                    .header("Overwrite", "T")
                    .header("If", condition.as_str()),
            }
        };
        self.send_to(
            "MOVE",
            self.url(stage)?,
            target,
            cancel,
            &decorate,
            &[201, 204],
        )?;
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
        self.send_to(
            "DELETE",
            self.collection_url(dir)?,
            dir,
            &cancel,
            &|request| request.header(IF_MATCH, tag.clone()),
            &[204],
        )?;
        Ok(())
    }
}
