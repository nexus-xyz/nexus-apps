//! A feed over HTTP: read a channel's `latest.json` and download one update file, with deadlines and failure codes.
//! Nexus GPUI checks its feed with these and verifies the bytes with [`crate::verify`].
//!
//! The client is the host's: it must not follow redirects, so a credential never leaves the URL it was meant for.
//! TLS is reqwest's `rustls-no-provider` with ring installed by [`install_tls`].
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderValue};
use semver::Version;

use crate::cache::{Artifact, Cache};
use crate::manifest::{Manifest, Platform};
use crate::{App, Failure, Release};

/// A manifest check's deadline (README, "Distribution").
pub const CHECK_DEADLINE: Duration = Duration::from_secs(35);
/// A download's deadline.
pub const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(10 * 60);
/// The largest update file accepted, as the cache's limit.
pub const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// rustls' ring provider for the process, once, before the first client: reqwest is built with
/// `rustls-no-provider` (the apps' pin), which has none of its own. The same call as nexus-client's `install_tls`;
/// whichever runs first wins and the others are no-ops.
pub fn install_tls() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A client for update feeds: no redirects, and a connect deadline.
pub fn client() -> Result<reqwest::Client, Failure> {
    install_tls();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| {
            Failure::new(
                "update_failed",
                format!("The update client could not start: {error}."),
                false,
            )
        })
}

fn network(message: &'static str) -> Failure {
    Failure::new("network", message, true)
}

fn not_found() -> Failure {
    Failure::new(
        "release_not_found",
        "The update feed has no release for this channel, or it returned an error. Nexus will check again later.",
        false,
    )
}

fn get(client: &reqwest::Client, url: &str, authorization: Option<&HeaderValue>) -> reqwest::RequestBuilder {
    let request = client.get(url);
    match authorization {
        Some(authorization) => request.header(AUTHORIZATION, authorization.clone()),
        None => request,
    }
}

/// The channel's manifest at `url`. A 404 until a channel's first publish, a 403, a 5xx or a redirect is
/// `release_not_found`, not a network error.
pub async fn manifest(
    client: &reqwest::Client,
    url: &str,
    authorization: Option<&HeaderValue>,
) -> Result<Manifest, Failure> {
    let read = async {
        let response = get(client, url, authorization)
            .send()
            .await
            .map_err(|_| network("The update service could not be reached. Check your connection or try again. Nexus will retry automatically."))?;
        if !response.status().is_success() {
            return Err(not_found());
        }
        let body = response
            .text()
            .await
            .map_err(|_| network("The update service could not be reached. Check your connection or try again. Nexus will retry automatically."))?;
        Manifest::parse(&body)
    };
    tokio::time::timeout(CHECK_DEADLINE, read)
        .await
        .map_err(|_| network("The update check timed out. Check your connection or try again."))?
}

/// Download `url`, calling `progress(received, total)` as bytes arrive. The bytes are not verified here: the caller
/// checks them with [`crate::verify::verify`] before keeping them.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    authorization: Option<&HeaderValue>,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, Failure> {
    let too_large = || Failure::new("update_failed", "The update is too large. Contact support.", false);
    let read = async {
        let mut response = get(client, url, authorization)
            .send()
            .await
            .map_err(|_| network("The update service could not be reached. Check your connection or try again. Nexus will retry automatically."))?;
        if !response.status().is_success() {
            return Err(not_found());
        }
        let total = response.content_length();
        if total.is_some_and(|total| total > MAX_BYTES) {
            return Err(too_large());
        }
        let mut bytes = Vec::with_capacity(total.unwrap_or(0) as usize);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| network("The download stopped. Check your connection or try again."))?
        {
            if bytes.len() as u64 + chunk.len() as u64 > MAX_BYTES {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
            progress(bytes.len() as u64, total);
        }
        Ok(bytes)
    };
    tokio::time::timeout(DOWNLOAD_DEADLINE, read)
        .await
        .map_err(|_| network("The download timed out. Check your connection or try again."))?
}

/// What a feed does with a newer release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Download the update file and verify it: the host installs it on restart.
    Install,
    /// Only announce it: the host offers a download in the browser. For a platform that cannot replace itself yet
    /// (an unsigned macOS app).
    Announce,
}

/// A newer release, ready for the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prepared {
    /// The update file, verified against its signature and signed version.
    Verified { bytes: Vec<u8>, url: String },
    /// Where to download it. Not downloaded or verified here.
    Announced { url: String },
}

/// Progress for the host's snapshot: the version, bytes received, the total when known, and whether it is verifying.
pub type Progress = Box<dyn Fn(&str, u64, Option<u64>, bool) + Send + Sync>;

/// One app's feed for one channel, over HTTP, for [`crate::fetch_channel`].
pub struct Feed {
    pub client: reqwest::Client,
    pub app: App,
    /// The channel's `latest.json`: `Channel::feed(app)`, or a local server in tests.
    pub endpoint: String,
    /// This computer's platform (`manifest::target`).
    pub target: String,
    /// The installed version. A manifest at or below it clears the cached download.
    pub installed: String,
    /// The update keys: the base64 of one or more minisign public key files, separated by whitespace.
    pub pubkey: String,
    /// The download service's credential for the internal channel (`access::update_channel`).
    pub authorization: Option<HeaderValue>,
    pub mode: Mode,
    /// Where a completed download is kept across launches.
    pub cache: Option<Cache>,
    pub progress: Progress,
}

impl Feed {
    /// The folder the manifest is in. An update file must be under it: a manifest cannot send the app, or its
    /// credential, anywhere else.
    fn folder(&self) -> &str {
        self.endpoint.rsplit_once('/').map_or("", |(folder, _)| folder)
    }

    /// Whether `url` is under [`Self::folder`], compared as the HTTP client will fetch it: the same scheme, host and
    /// port, no user name, and a path under the folder's after `..` and `%2e%2e` are resolved.
    fn in_folder(&self, url: &str) -> bool {
        let (Ok(feed), Ok(file)) = (reqwest::Url::parse(self.folder()), reqwest::Url::parse(url)) else {
            return false;
        };
        feed.origin() == file.origin()
            && file.username().is_empty()
            && file.password().is_none()
            && file
                .path()
                .starts_with(&format!("{}/", feed.path().trim_end_matches('/')))
    }

    /// The credential, only for `url` on the download service's folder for this app.
    fn credential(&self, url: &str) -> Option<&HeaderValue> {
        let parsed = reqwest::Url::parse(url).ok()?;
        self.authorization
            .as_ref()
            .filter(|_| crate::access::employee_url_of(self.app, &parsed))
    }
}

impl crate::Feed for Feed {
    type Candidate = Platform;
    type Prepared = Prepared;

    async fn check(&self) -> Result<Option<Release<Platform>>, Failure> {
        let manifest = manifest(&self.client, &self.endpoint, self.credential(&self.endpoint)).await?;
        let newer = matches!(
            (Version::parse(&manifest.version), Version::parse(&self.installed)),
            (Ok(remote), Ok(installed)) if remote > installed
        );
        if !newer {
            // Once the installed version catches up, the staged artifact is no longer needed.
            if let Some(cache) = &self.cache {
                let _ = cache.clear();
            }
            return Ok(None);
        }
        // A platform without a signed file is left out of the manifest: its people are not offered the update.
        Ok(manifest.platform(&self.target).cloned().map(|artifact| Release {
            version: manifest.version.clone(),
            artifact,
        }))
    }

    async fn prepare(&self, release: Release<Platform>) -> Result<Prepared, Failure> {
        let url = release.artifact.url.clone();
        if !self.in_folder(&url) {
            return Err(Failure::new(
                "invalid_download_origin",
                "The update download address is invalid. Contact support.",
                false,
            ));
        }
        if self.mode == Mode::Announce {
            return Ok(Prepared::Announced { url });
        }
        let artifact = Artifact {
            version: release.version.clone(),
            target: self.target.clone(),
            url: url.clone(),
            signature: release.artifact.signature.clone(),
        };
        if let Some(bytes) = self
            .cache
            .as_ref()
            .and_then(|cache| cache.load(&artifact, &self.pubkey))
        {
            (self.progress)(&release.version, bytes.len() as u64, Some(bytes.len() as u64), true);
            return Ok(Prepared::Verified { bytes, url });
        }
        (self.progress)(&release.version, 0, None, false);
        let bytes = download(&self.client, &url, self.credential(&url), |received, total| {
            (self.progress)(&release.version, received, total, false)
        })
        .await?;
        (self.progress)(&release.version, bytes.len() as u64, Some(bytes.len() as u64), true);
        crate::verify::verify(&bytes, &artifact.signature, &self.pubkey, &release.version)?;
        if let Some(cache) = &self.cache
            && let Err(error) = cache.save(&artifact, &bytes)
        {
            eprintln!("nexus-updates: could not save completed download: {error}");
        }
        Ok(Prepared::Verified { bytes, url })
    }
}

#[cfg(test)]
mod tests;
