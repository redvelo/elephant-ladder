//! Release catalog retrieval: what the application trusts, and where it may fetch from.

use std::time::Duration;

use elephant_ladder_dictionary_pack::{Catalog, MAX_CATALOG_BYTES, parse_catalog};
use url::Url;

/// The catalog is committed to the dictionary repository and read from its default branch,
/// so publishing packs needs no application release. HTTPS to this one authority is what
/// makes the answer trustworthy; every asset the catalog names is then accepted only on
/// its declared digest, so the assets themselves need no trusted host.
const CATALOG_URL: &str =
    "https://raw.githubusercontent.com/redvelo/elephant-ladder-dictionary/main/catalog-v1.json";

/// Authorities that may serve a catalog or a pack asset, including redirect targets.
const RELEASE_HOSTS: &[&str] = &[
    "raw.githubusercontent.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// The only authority remote pronunciations are fetched from.
const COMMONS_HOSTS: &[&str] = &["upload.wikimedia.org"];

/// Which authorities a fetch may reach, checked again at every redirect hop.
#[derive(Clone, Copy)]
pub(super) enum Destination {
    Release,
    Commons,
}

const MAX_REDIRECTS: usize = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Tests point the host at a loopback release without touching process environment.
#[cfg(test)]
pub(crate) static TEST_SOURCE: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// The configured catalog URL, or a development override in debug builds.
pub(crate) fn configured_url() -> Result<String, String> {
    Ok(allowed_url(&configured(), Destination::Release)?.to_string())
}

fn configured() -> String {
    #[cfg(test)]
    if let Some(url) = TEST_SOURCE
        .read()
        .expect("dictionary test source lock poisoned")
        .clone()
    {
        return url;
    }
    release_url()
}

#[cfg(debug_assertions)]
fn release_url() -> String {
    std::env::var("ELEPHANT_DICTIONARY_CATALOG_URL").unwrap_or_else(|_| CATALOG_URL.to_owned())
}

#[cfg(not(debug_assertions))]
fn release_url() -> String {
    CATALOG_URL.to_owned()
}

/// An absolute URL this application may fetch from.
///
/// The catalog format admits `http://` asset URLs, so this is the authority check, and it
/// runs again for every redirect hop rather than only for the configured URL.
pub(super) fn allowed_url(value: &str, destination: Destination) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "the dictionary URL is invalid".to_owned())?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let loopback = matches!(url.scheme(), "http" | "https")
        && matches!(host.as_str(), "localhost" | "127.0.0.1");
    let listed = |hosts: &[&str]| url.scheme() == "https" && hosts.contains(&host.as_str());
    // A development catalog replaces the release host only; pronunciations still come
    // from Commons.
    let allowed = match destination {
        Destination::Release if development_source() => loopback,
        Destination::Release => listed(RELEASE_HOSTS),
        Destination::Commons => listed(COMMONS_HOSTS) || (cfg!(test) && loopback),
    };
    if !allowed || !url.username().is_empty() || url.password().is_some() {
        return Err("the dictionary URL is not an allowed release address".to_owned());
    }
    Ok(url)
}

/// Debug builds pointed at a development catalog fetch from loopback instead of the release.
fn development_source() -> bool {
    #[cfg(test)]
    if TEST_SOURCE
        .read()
        .expect("dictionary test source lock poisoned")
        .is_some()
    {
        return true;
    }
    cfg!(debug_assertions) && std::env::var_os("ELEPHANT_DICTIONARY_CATALOG_URL").is_some()
}

/// One client for every dictionary transfer.
///
/// Redirects are refused here and followed by [`fetch`] instead, so each hop is checked
/// against [`allowed_url`] rather than trusted because the first URL was allowed.
pub(crate) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .use_preconfigured_tls(tls_configuration())
        .redirect(reqwest::redirect::Policy::none())
        // Wikimedia's robot policy asks every client to name itself and a contact.
        .user_agent(concat!(
            "ElephantLadder/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/redvelo/elephant-ladder-dictionary)"
        ))
        .connect_timeout(REQUEST_TIMEOUT)
        .read_timeout(TRANSFER_TIMEOUT)
        .build()
        .map_err(|error| format!("the dictionary client could not start: {error}"))
}

/// Trusts the bundled Mozilla root store rather than the platform's.
///
/// reqwest's default rustls path uses `rustls-platform-verifier`, which panics on Android
/// unless it is initialized with a JNI environment the host thread never has. The
/// application only ever talks to one release host, so a fixed root store is both simpler
/// and identical on desktop, Android, and the reader devices.
fn tls_configuration() -> rustls::ClientConfig {
    static PROVIDER: std::sync::Once = std::sync::Once::new();
    PROVIDER.call_once(|| {
        // Installing twice is an error, not a problem; another component may have won.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// The client pronunciation playback reuses, so repeated plays keep their connection.
pub(super) fn shared_client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = client()?;
    Ok(CLIENT.get_or_init(|| client))
}

/// Performs one GET, following redirects manually so every hop is authority-checked.
pub(super) async fn fetch(
    client: &reqwest::Client,
    url: &str,
    range_from: Option<u64>,
    destination: Destination,
) -> Result<reqwest::Response, String> {
    let mut current = allowed_url(url, destination)?;
    for _ in 0..=MAX_REDIRECTS {
        let mut request = client.get(current.clone());
        if let Some(offset) = range_from {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("the dictionary request failed: {error}"))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "the dictionary redirect is invalid".to_owned())?;
            let next = current
                .join(location)
                .map_err(|_| "the dictionary redirect is invalid".to_owned())?;
            current = allowed_url(next.as_str(), destination)?;
            continue;
        }
        return Ok(response);
    }
    Err("the dictionary request redirected too many times".to_owned())
}

/// Fetches and parses the catalog.
pub(crate) async fn load_catalog(
    client: &reqwest::Client,
    catalog_url: &str,
) -> Result<Catalog, String> {
    let catalog = read_catalog(client, catalog_url).await?;
    parse_catalog(&catalog).map_err(|error| format!("the dictionary catalog is invalid: {error}"))
}

async fn read_catalog(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let mut response = fetch(client, url, None, Destination::Release).await?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(format!(
            "the dictionary catalog request returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("the dictionary catalog could not be read: {error}"))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_CATALOG_BYTES {
            return Err("the dictionary catalog is too large".to_owned());
        }
    }
    Ok(bytes)
}
