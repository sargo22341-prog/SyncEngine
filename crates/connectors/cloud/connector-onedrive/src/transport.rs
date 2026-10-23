use connector_api::{StorageError, StorageErrorKind as Kind, StorageOperation as Op};
use reqwest::{
    Url,
    blocking::{Client, Response},
    header::{AUTHORIZATION, HeaderValue, RETRY_AFTER},
};
use serde::de::DeserializeOwned;
use std::{
    io::Read,
    sync::Arc,
    time::{Duration, Instant},
};
use sync_core::{CancellationToken, ConnectorId, ResourcePath};
use sync_credentials::{AccessTokenProvider, CredentialError};

const JSON_LIMIT: u64 = 8 * 1024 * 1024;

pub(super) fn error(kind: Kind, op: Op, path: &ResourcePath) -> StorageError {
    let err = StorageError::new(kind, op).with_path(path.clone());
    match ConnectorId::new("onedrive") {
        Ok(id) => err.with_connector(id),
        Err(_) => err,
    }
}
pub(super) fn check(
    cancel: &CancellationToken,
    op: Op,
    path: &ResourcePath,
) -> Result<(), StorageError> {
    if cancel.is_cancelled() {
        Err(error(Kind::Cancelled, op, path))
    } else {
        Ok(())
    }
}
pub(super) fn client() -> Result<Client, StorageError> {
    Client::builder()
        .https_only(true)
        .tls_version_min(reqwest::tls::Version::TLS_1_2)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .referer(false)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| error(Kind::Permanent, Op::Connect, &ResourcePath::root()))
}

#[derive(Clone)]
pub(super) struct Http {
    pub client: Client,
    pub base: Url,
    pub tokens: Arc<dyn AccessTokenProvider>,
    pub download_hosts: Vec<String>,
}
impl Http {
    pub(super) fn graph_url(&self, url: &Url, expected_path: &str) -> bool {
        url.origin() == self.base.origin()
            && url.path() == expected_path
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    }
    pub(super) fn download_url(&self, url: &Url) -> bool {
        url.scheme() == "https"
            && url.port_or_known_default() == self.base.port_or_known_default()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url
                .host_str()
                .is_some_and(|h| self.download_hosts.iter().any(|v| v == h))
    }
    // Only GETs are retried, never a stream once handed to the caller.
    pub(super) fn get(
        &self,
        url: &Url,
        auth: bool,
        op: Op,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Response, StorageError> {
        for attempt in 0..3 {
            check(cancel, op, path)?;
            let mut request = self.client.get(url.clone());
            if auth {
                let token = self.tokens.access_token(cancel).map_err(|e| {
                    error(
                        match e {
                            CredentialError::Cancelled => Kind::Cancelled,
                            CredentialError::Unavailable => Kind::Unauthenticated,
                        },
                        op,
                        path,
                    )
                })?;
                let mut header = HeaderValue::from_str(&format!("Bearer {}", token.expose()))
                    .map_err(|_| error(Kind::Unauthenticated, op, path))?;
                header.set_sensitive(true);
                request = request.header(AUTHORIZATION, header);
            }
            let response = request
                .send()
                .map_err(|_| error(Kind::Disconnected, op, path))?;
            check(cancel, op, path)?;
            let status = response.status().as_u16();
            if status == 200 || (auth && status == 302) {
                return Ok(response);
            }
            let retry = response.headers().get(RETRY_AFTER);
            let seconds = retry
                .and_then(|s| s.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            let kind = match status {
                401 => Kind::Unauthenticated,
                403 => Kind::PermissionDenied,
                404 => Kind::NotFound,
                409 | 412 => Kind::Conflict,
                429 => Kind::Throttled {
                    retry_after: seconds.map(Duration::from_secs),
                },
                507 => Kind::InsufficientSpace,
                500 | 502 | 503 | 504 => Kind::Transient,
                _ => Kind::Permanent,
            };
            let delay = seconds.unwrap_or(1 << attempt);
            // Unknown Retry-After (HTTP date etc.) is surfaced without early retry.
            if !matches!(kind, Kind::Throttled { .. } | Kind::Transient)
                || attempt == 2
                || delay > 2
                || (retry.is_some() && seconds.is_none())
            {
                return Err(error(kind, op, path));
            }
            drop(response);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(delay) {
                check(cancel, op, path)?;
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        Err(error(Kind::Transient, op, path))
    }
    pub(super) fn json<T: DeserializeOwned>(
        &self,
        url: &Url,
        op: Op,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<T, StorageError> {
        let response = self.get(url, true, op, path, cancel)?;
        if response.status().as_u16() != 200 {
            return Err(error(Kind::Permanent, op, path));
        }
        let mut bytes = Vec::new();
        response
            .take(JSON_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| error(Kind::Disconnected, op, path))?;
        check(cancel, op, path)?;
        if bytes.len() as u64 > JSON_LIMIT {
            return Err(error(Kind::Permanent, op, path));
        }
        serde_json::from_slice(&bytes).map_err(|_| error(Kind::Permanent, op, path))
    }
}
