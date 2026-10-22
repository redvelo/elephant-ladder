//! Pronunciations of index-only collections, fetched from Commons when someone plays one.

use std::time::Duration;

use elephant_ladder_dictionary_pack::RecordingFacts;
use md5::Md5;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha1::{Digest as _, Sha1};

use super::catalog::{self, Destination};

/// Far above any single spoken word; the largest French original is 13 MB.
const MAX_RECORDING_BYTES: u64 = 4 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const UPLOAD_BASE: &str = "https://upload.wikimedia.org/wikipedia/commons";

/// Commons writes upload paths with `rawurlencode`, which leaves only these unescaped.
const UPLOAD_PATH: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

#[cfg(test)]
pub(crate) static TEST_UPLOAD_BASE: std::sync::RwLock<Option<String>> =
    std::sync::RwLock::new(None);

pub(super) enum RemoteFailure {
    /// Commons could not be reached, asked us to wait, or took too long.
    Unreachable,
    /// Commons answered with something other than the recording the index describes.
    Rejected,
}

/// One original on Commons, as the index describes it.
pub(super) struct RemoteRecording {
    url: String,
    sha1: [u8; 20],
    media_type: &'static str,
}

impl RemoteRecording {
    /// The original to fetch, or `None` when the index cannot vouch for one worth playing.
    pub(super) fn from_facts(facts: &RecordingFacts) -> Option<Self> {
        let name = facts.source_title.as_deref()?.strip_prefix("File:")?;
        if facts
            .source_size_bytes
            .is_some_and(|size| size > MAX_RECORDING_BYTES)
        {
            return None;
        }
        Some(Self {
            url: upload_url(name),
            sha1: facts.source_sha1?,
            media_type: playable_media_type(facts.source_media_type.as_deref()?)?,
        })
    }

    pub(super) const fn media_type(&self) -> &'static str {
        self.media_type
    }

    /// The original's bytes, accepted only when they are exactly what the index names.
    pub(super) async fn fetch(&self) -> Result<Vec<u8>, RemoteFailure> {
        tokio::time::timeout(FETCH_TIMEOUT, self.read())
            .await
            .map_err(|_| RemoteFailure::Unreachable)?
    }

    async fn read(&self) -> Result<Vec<u8>, RemoteFailure> {
        let client = catalog::shared_client().map_err(|_| RemoteFailure::Unreachable)?;
        let mut response = catalog::fetch(client, &self.url, None, Destination::Commons)
            .await
            .map_err(|_| RemoteFailure::Unreachable)?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(RemoteFailure::Unreachable);
        }
        if status != reqwest::StatusCode::OK {
            return Err(RemoteFailure::Rejected);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| RemoteFailure::Unreachable)?
        {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > MAX_RECORDING_BYTES {
                return Err(RemoteFailure::Rejected);
            }
        }
        if Sha1::digest(&bytes).as_slice() != self.sha1 {
            return Err(RemoteFailure::Rejected);
        }
        Ok(bytes)
    }
}

/// Where Commons serves a file: a directory named by the MD5 of its title, then the title.
fn upload_url(name: &str) -> String {
    let name = name.replace(' ', "_");
    let digest = hex_digest(&Md5::digest(name.as_bytes()));
    format!(
        "{}/{}/{}/{}",
        upload_base(),
        &digest[..1],
        &digest[..2],
        utf8_percent_encode(&name, UPLOAD_PATH)
    )
}

fn upload_base() -> String {
    #[cfg(test)]
    if let Some(base) = TEST_UPLOAD_BASE
        .read()
        .expect("upload base lock poisoned")
        .clone()
    {
        return base;
    }
    UPLOAD_BASE.to_owned()
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The type the reader's audio element is given, for the source types Commons stores.
fn playable_media_type(source: &str) -> Option<&'static str> {
    match source {
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("audio/wav"),
        "application/ogg" | "audio/ogg" => Some("audio/ogg"),
        "audio/x-flac" | "audio/flac" => Some("audio/flac"),
        "audio/mpeg" => Some("audio/mpeg"),
        "audio/webm" => Some("audio/webm"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use elephant_ladder_dictionary_pack::{RecordingFacts, RecordingStatus};

    use super::{RemoteRecording, upload_url};

    #[tokio::test]
    #[ignore = "requires network access to Wikimedia Commons"]
    async fn fetches_and_verifies_a_real_commons_original() {
        let mut sha1 = [0; 20];
        for (index, byte) in sha1.iter_mut().enumerate() {
            *byte = u8::from_str_radix(
                &"2273224542AF2EF074AE6BC39046E8028C34BE30"[index * 2..index * 2 + 2],
                16,
            )
            .unwrap();
        }
        let facts = RecordingFacts {
            file_name: "LL-Q150 (fra)-Adem M. (BiblioCanet66)-chat.wav".to_owned(),
            status: RecordingStatus::Available,
            reason: None,
            reference_count: 1,
            source_title: Some("File:LL-Q150 (fra)-Adem M. (BiblioCanet66)-chat.wav".to_owned()),
            source_sha1: Some(sha1),
            source_timestamp: None,
            source_media_type: Some("audio/wav".to_owned()),
            source_size_bytes: Some(95_788),
            description_url: None,
            license: None,
            author_text: None,
            author_urls: Vec::new(),
            attribution_required: None,
        };
        let recording = RemoteRecording::from_facts(&facts).unwrap();
        let bytes = recording
            .fetch()
            .await
            .ok()
            .expect("the original should verify");
        assert_eq!(bytes.len(), 95_788);
        assert_eq!(recording.media_type(), "audio/wav");
    }

    #[test]
    fn upload_urls_follow_the_commons_hashed_layout() {
        assert_eq!(
            upload_url("(be-fr)-Mons-anus.wav"),
            "https://upload.wikimedia.org/wikipedia/commons/a/af/%28be-fr%29-Mons-anus.wav"
        );
        assert_eq!(
            upload_url(
                "\"Le village de Mollon dans l'Ain\" prononcé par un habitant (dans la rue).ogg"
            ),
            "https://upload.wikimedia.org/wikipedia/commons/f/f7/%22Le_village_de_Mollon_dans_l%27Ain%22_prononc%C3%A9_par_un_habitant_%28dans_la_rue%29.ogg"
        );
    }
}
