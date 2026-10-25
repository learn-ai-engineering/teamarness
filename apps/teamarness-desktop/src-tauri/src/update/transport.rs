use super::{BoxFuture, Cancellation, Clock, ErrorCategory};
use reqwest::{Client, Url, redirect::Policy};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::Instant;

const HOSTS: &[&str] = &[
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
    "github-releases.githubusercontent.com",
];
pub(super) fn validate_url(url: &Url) -> Result<(), ErrorCategory> {
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !HOSTS.contains(&url.host_str().unwrap_or_default())
    {
        return Err(ErrorCategory::Metadata);
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) enum Resource {
    Feed,
    Archive,
}
impl Resource {
    fn limit(self) -> usize {
        match self {
            Self::Feed => super::config::FEED_LIMIT,
            Self::Archive => super::config::ARCHIVE_LIMIT,
        }
    }
    fn total(self) -> Duration {
        match self {
            Self::Feed => Duration::from_secs(30),
            Self::Archive => Duration::from_secs(15 * 60),
        }
    }
}
pub(super) type Progress = Arc<dyn Fn(usize) + Send + Sync>;
pub(super) trait Transfer: Send + Sync {
    fn fetch(
        &self,
        url: Url,
        resource: Resource,
        cancel: Cancellation,
        progress: Progress,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, ErrorCategory>>;
}

pub(super) trait Body: Send {
    fn chunk(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, ErrorCategory>>;
}
pub(super) struct Response {
    pub status: u16,
    pub location: Option<String>,
    pub encoding: Option<String>,
    pub body: Box<dyn Body>,
}
pub(super) trait Http: Send + Sync {
    fn get(&self, url: Url) -> BoxFuture<'_, Result<Response, ErrorCategory>>;
}
struct ReqwestHttp(Client);
struct ReqwestBody(reqwest::Response);
impl Body for ReqwestBody {
    fn chunk(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, ErrorCategory>> {
        Box::pin(async {
            self.0
                .chunk()
                .await
                .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
                .map_err(|_| ErrorCategory::Network)
        })
    }
}
impl Http for ReqwestHttp {
    fn get(&self, url: Url) -> BoxFuture<'_, Result<Response, ErrorCategory>> {
        Box::pin(async move {
            let response = self
                .0
                .get(url)
                .header("Accept-Encoding", "identity")
                .header("Accept", "application/octet-stream")
                .send()
                .await
                .map_err(|_| ErrorCategory::Network)?;
            fn header(
                response: &reqwest::Response,
                name: &str,
            ) -> Result<Option<String>, ErrorCategory> {
                response
                    .headers()
                    .get(name)
                    .map(|value| {
                        value
                            .to_str()
                            .map(str::to_owned)
                            .map_err(|_| ErrorCategory::Network)
                    })
                    .transpose()
            }
            Ok(Response {
                status: response.status().as_u16(),
                location: header(&response, "location")?,
                encoding: header(&response, "content-encoding")?,
                body: Box::new(ReqwestBody(response)),
            })
        })
    }
}
fn client_builder() -> reqwest::ClientBuilder {
    // reqwest rustls-no-provider deliberately avoids adding a second crypto backend.
    // The locked dependency closure already uses ring. Installation is process-wide
    // and idempotent: a concurrent installer may have selected the provider first.
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder()
        .https_only(true)
        .redirect(Policy::none())
        .referer(false)
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .connect_timeout(Duration::from_secs(10))
        .user_agent("Teamarness-updater")
}
fn client() -> Result<Client, ErrorCategory> {
    client_builder().build().map_err(|_| ErrorCategory::Network)
}

pub(super) struct BoundedTransfer {
    http: Box<dyn Http>,
    clock: Arc<dyn Clock>,
}
impl BoundedTransfer {
    pub fn new(clock: Arc<dyn Clock>) -> Result<Self, ErrorCategory> {
        Ok(Self {
            http: Box::new(ReqwestHttp(client()?)),
            clock,
        })
    }
    async fn bounded<T>(
        &self,
        future: BoxFuture<'_, Result<T, ErrorCategory>>,
        deadline: Instant,
        cancel: &Cancellation,
    ) -> Result<T, ErrorCategory> {
        tokio::select! { biased;
            _ = cancel.cancelled() => Err(ErrorCategory::Cancelled),
            _ = self.clock.sleep_until(deadline) => Err(ErrorCategory::Timeout),
            value = future => value,
        }
    }
    async fn acquire(
        &self,
        url: Url,
        resource: Resource,
        cancel: Cancellation,
        progress: Progress,
    ) -> Result<Option<Vec<u8>>, ErrorCategory> {
        self.acquire_with_limit(url, resource, cancel, progress, resource.limit())
            .await
    }
    async fn acquire_with_limit(
        &self,
        mut url: Url,
        resource: Resource,
        cancel: Cancellation,
        progress: Progress,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, ErrorCategory> {
        let total = self.clock.now() + resource.total();
        let mut visited = HashSet::new();
        for hop in 0..=5 {
            validate_url(&url)?;
            if !visited.insert(url.clone()) {
                return Err(ErrorCategory::Metadata);
            }
            let connection_deadline = (self.clock.now() + Duration::from_secs(10)).min(total);
            let mut response = self
                .bounded(self.http.get(url.clone()), connection_deadline, &cancel)
                .await?;
            if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                if hop == 5 {
                    return Err(ErrorCategory::Metadata);
                }
                url = url
                    .join(
                        response
                            .location
                            .as_deref()
                            .ok_or(ErrorCategory::Metadata)?,
                    )
                    .map_err(|_| ErrorCategory::Metadata)?;
                continue;
            }
            if response.status == 404 && matches!(resource, Resource::Feed) {
                return Ok(None);
            }
            if response.status != 200 {
                return Err(ErrorCategory::Network);
            }
            if response
                .encoding
                .as_deref()
                .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
            {
                return Err(ErrorCategory::Metadata);
            }
            let mut bytes = Vec::new();
            let mut inactivity = self.clock.now() + Duration::from_secs(30);
            loop {
                let deadline = total.min(inactivity);
                let Some(chunk) = self
                    .bounded(response.body.chunk(), deadline, &cancel)
                    .await?
                else {
                    break;
                };
                append(&mut bytes, &chunk, limit)?;
                if !chunk.is_empty() {
                    inactivity = self.clock.now() + Duration::from_secs(30);
                    progress(bytes.len());
                }
            }
            return Ok(Some(bytes));
        }
        Err(ErrorCategory::Metadata)
    }
}
fn append(bytes: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<(), ErrorCategory> {
    if chunk.len() > limit.saturating_sub(bytes.len()) {
        return Err(ErrorCategory::TooLarge);
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}
impl Transfer for BoundedTransfer {
    fn fetch(
        &self,
        url: Url,
        resource: Resource,
        cancel: Cancellation,
        progress: Progress,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, ErrorCategory>> {
        Box::pin(self.acquire(url, resource, cancel, progress))
    }
}

#[cfg(test)]
mod tests;
