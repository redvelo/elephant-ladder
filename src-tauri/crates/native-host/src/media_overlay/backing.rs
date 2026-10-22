//! Materializes compressed Media Overlay audio into bounded, immutable, app-owned sidecar files.
//!
//! A sidecar is derived cache data: the immutable source EPUB remains authoritative and can
//! regenerate it. This module only prepares one staged sidecar file per referenced local audio
//! resource; committing it into permanent content-addressed storage alongside the reading
//! snapshot transaction is `storage::Storage`'s responsibility.
//!
//! Direct archive-extent slicing for stored, uncompressed audio entries is intentionally not
//! implemented in this release: the one required acceptance publication (and, in practice, most
//! EPUB packaging tools) DEFLATE-compresses audio regardless of format, so every local playback
//! audio resource is materialized through this sidecar path. `ResponseBacking` therefore has no
//! `ArchiveExtent` variant yet.
//!
//! No engine this app runs on plays every core EPUB audio type: desktop CEF ships without
//! AAC, and the Android System WebView ships without Opus. A referenced audio resource the
//! reading engine cannot play is therefore replaced by a playable manifest fallback, a
//! previously converted sidecar of the same source bytes, or a fresh conversion into a
//! format that engine does play.

use crate::staging::StagedFile;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::media_overlay::transcode::{AudioTarget, SourceAudio};
use epub_stack::resource::provider::ResourceProvider;
use epub_stack::{Epub, EpubPath};

/// Which core EPUB audio types the engine behind this build's reading view decodes.
///
/// Both gaps are measured rather than assumed; the engine-support probe records what each
/// platform answered, and the core media type corpus rows fail when one of these is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EngineAudio {
    plays_aac: bool,
    plays_ogg_opus: bool,
}

impl EngineAudio {
    /// The format this engine is given when it cannot play what the publication ships.
    ///
    /// MP3 is the only audio type every engine decodes, so it is the fallback whenever the
    /// engine cannot take Ogg Opus.
    pub(crate) fn target(self) -> AudioTarget {
        if self.plays_ogg_opus {
            AudioTarget::OggOpus
        } else {
            AudioTarget::Mp3
        }
    }

    /// Whether a resource declared as this media type could need conversion.
    ///
    /// The container is what finally decides, but reading it means copying the resource
    /// first, and an audiobook chapter is large. A declared type the engine certainly
    /// plays is therefore never copied at all.
    pub(crate) fn may_need_conversion(self, mime: &str) -> bool {
        let mut parts = mime.split(';').map(str::trim);
        let essence = parts.next().unwrap_or_default();
        // EPUB 3.4 names Opus in MP4 as its own core type, so a conforming publication
        // declares the codec, and that declaration is the only way to tell the two apart
        // without copying the resource to read its container.
        let declares_opus = parts.any(|parameter| {
            parameter
                .strip_prefix("codecs=")
                .is_some_and(|value| value.trim_matches('"').eq_ignore_ascii_case("opus"))
        });
        match essence {
            "audio/mp4" | "audio/aac" | "audio/aacp" => declares_opus || !self.plays_aac,
            "audio/ogg" | "audio/opus" => !self.plays_ogg_opus,
            _ => false,
        }
    }

    /// The conversion this resource needs, or `None` when the engine plays it as authored.
    pub(crate) fn conversion(self, source: Option<SourceAudio>) -> Option<AudioTarget> {
        let playable = match source? {
            SourceAudio::Aac => self.plays_aac,
            SourceAudio::OggOpus => self.plays_ogg_opus,
            // CEF cannot demux MP4 and the Android WebView cannot decode Opus, so this
            // one is playable nowhere and is always converted.
            SourceAudio::OpusInMp4 => false,
        };
        (!playable).then(|| self.target())
    }
}

/// CEF carries no proprietary codecs and so cannot demux MP4 or decode AAC. The AOSP
/// Android System WebView is the mirror of it: AAC plays, Opus does not.
///
/// This is the build's engine rather than the device's, and it is deliberately the
/// conservative reading of Android. A device carrying Google's WebView instead of the
/// AOSP one does decode Opus, so there the conversion is work that was not needed; it
/// costs time and a second lossy pass but never correctness. Deciding this per device
/// needs the reading page to report what it can play, which it does not do yet.
pub(crate) const READING_ENGINE_AUDIO: EngineAudio = if cfg!(target_os = "android") {
    EngineAudio {
        plays_aac: true,
        plays_ogg_opus: false,
    }
} else {
    EngineAudio {
        plays_aac: false,
        plays_ogg_opus: true,
    }
};

/// Every audio resource that needs an app owned sidecar, in a stable order.
///
/// Narration is always backed, because playback needs a file the host can range read.
/// Audio a content document references is backed only when this engine might not play
/// it, since otherwise the archive entry is served directly.
pub(crate) fn audio_paths_to_back(
    narration: Vec<String>,
    responses: &crate::snapshot::ResponseManifest,
) -> Vec<String> {
    let mut paths = narration;
    let mut extra: Vec<&String> = responses
        .resources
        .iter()
        .filter(|(path, rule)| {
            !rule.missing
                && READING_ENGINE_AUDIO.may_need_conversion(&rule.mime)
                && !paths.contains(path)
        })
        .map(|(path, _)| path)
        .collect();
    extra.sort_unstable();
    paths.extend(extra.into_iter().cloned());
    paths
}

/// Identifies the conversion output so installed sidecars are reused only when identical.
pub(crate) fn transcode_derivation() -> &'static str {
    READING_ENGINE_AUDIO.target().derivation()
}

const MAX_FALLBACK_DECLARATIONS: usize = 16;

/// How a sidecar's bytes relate to the referenced audio resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidecarOrigin {
    Copied,
    Fallback,
    Transcoded,
    Reused,
}

/// An installed transcode that can back another derivation of the same source bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReusableSidecar {
    pub storage_name: String,
    pub length: u64,
}

/// A staged sidecar file plus the metadata needed to install and reference it.
#[derive(Debug)]
pub(crate) struct PreparedSidecar {
    /// Exact canonical publication path this sidecar backs.
    pub canonical_path: String,
    /// Content-addressed storage name (BLAKE3 hex) the sidecar will be installed under.
    pub storage_name: String,
    /// Exact decoded byte length.
    pub length: u64,
    /// Response media type when the bytes are not the referenced resource's own.
    pub media_type: Option<String>,
    pub origin: SidecarOrigin,
    /// BLAKE3 of the referenced resource bytes a transcode was derived from.
    pub source_hash: Option<String>,
    guard: StagedFile,
}

impl PreparedSidecar {
    #[cfg(test)]
    fn staging_path(&self) -> &Path {
        self.guard.path()
    }

    /// Consumes this value, disarming staging cleanup and returning every field the caller needs
    /// to move the staged file into permanent storage and record it. A reused sidecar has no
    /// staging file.
    pub(crate) fn into_parts(mut self) -> PreparedSidecarParts {
        PreparedSidecarParts {
            canonical_path: self.canonical_path.clone(),
            storage_name: self.storage_name.clone(),
            length: self.length,
            media_type: self.media_type.take(),
            origin: self.origin,
            source_hash: self.source_hash.take(),
            staging_path: self.guard.disarm(),
        }
    }
}

/// The owned fields of a [`PreparedSidecar`] after staging cleanup has been disarmed.
pub(crate) struct PreparedSidecarParts {
    pub canonical_path: String,
    pub storage_name: String,
    pub length: u64,
    pub media_type: Option<String>,
    pub origin: SidecarOrigin,
    pub source_hash: Option<String>,
    pub staging_path: Option<PathBuf>,
}

/// A bounded reason a sidecar could not be prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidecarPreparationError {
    /// The resource could not be opened for reading (missing, remote, or provider failure).
    ResourceUnavailable,
    /// Preparing this resource would not fit in the storage still available to the import.
    InsufficientStorage,
    /// The decoded byte count differs from the resource's sealed size.
    LengthMismatch,
    /// The audio could not be decoded for transcoding.
    Decode(String),
    /// The decoded audio could not be encoded.
    Encode(String),
    /// Staging I/O failed, including a ZIP CRC failure detected at end of entry.
    Io(String),
}

impl std::fmt::Display for SidecarPreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ResourceUnavailable => write!(formatter, "audio resource is unavailable"),
            Self::InsufficientStorage => {
                write!(
                    formatter,
                    "audio resource does not fit in available storage"
                )
            }
            Self::LengthMismatch => {
                write!(
                    formatter,
                    "audio resource length differs from its sealed size"
                )
            }
            Self::Decode(message) => write!(formatter, "audio could not be decoded: {message}"),
            Self::Encode(message) => write!(formatter, "audio could not be encoded: {message}"),
            Self::Io(message) => write!(formatter, "sidecar staging failed: {message}"),
        }
    }
}

/// Materializes one local audio resource into a staged sidecar file.
///
/// `available_bytes` is the storage this sidecar may occupy, measured from the filesystem by the
/// caller; callers accumulate prepared sidecar lengths across calls and stop issuing further
/// preparations once it reaches zero. `reusable` maps source BLAKE3 hashes to installed transcodes.
pub(crate) fn prepare_audio_sidecar<R: ResourceProvider>(
    epub: &Epub<R>,
    canonical_path: &EpubPath,
    staging_dir: &Path,
    available_bytes: u64,
    reusable: &HashMap<String, ReusableSidecar>,
) -> Result<PreparedSidecar, SidecarPreparationError> {
    prepare_audio_sidecar_with_limit(epub, canonical_path, staging_dir, available_bytes, reusable)
}

fn prepare_audio_sidecar_with_limit<R: ResourceProvider>(
    epub: &Epub<R>,
    canonical_path: &EpubPath,
    staging_dir: &Path,
    available_bytes: u64,
    reusable: &HashMap<String, ReusableSidecar>,
) -> Result<PreparedSidecar, SidecarPreparationError> {
    let limits = Limits {
        available: available_bytes,
    };
    let copied = copy_resource(epub, canonical_path, staging_dir, limits)?;
    let source = crate::media_overlay::transcode::source_audio(copied.guard.path());
    if let Some(target) = READING_ENGINE_AUDIO.conversion(source) {
        return playable_sidecar(
            epub,
            canonical_path,
            copied,
            staging_dir,
            limits,
            reusable,
            target,
        );
    }
    Ok(copied)
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    available: u64,
}

impl Limits {
    fn bound(self) -> u64 {
        self.available
    }

    fn exceeded(self) -> SidecarPreparationError {
        SidecarPreparationError::InsufficientStorage
    }
}

fn copy_resource<R: ResourceProvider>(
    epub: &Epub<R>,
    canonical_path: &EpubPath,
    staging_dir: &Path,
    limits: Limits,
) -> Result<PreparedSidecar, SidecarPreparationError> {
    let record = epub
        .resources()
        .resource_by_path(canonical_path)
        .ok_or(SidecarPreparationError::ResourceUnavailable)?;
    let (mut file, guard) = create_sidecar_staging_file(staging_dir)
        .map_err(|error| SidecarPreparationError::Io(error.to_string()))?;
    let expected_length = record
        .presence()
        .size_bytes()
        .ok_or(SidecarPreparationError::ResourceUnavailable)?;
    let outcome = epub.read_with(canonical_path, |reader| {
        copy_bounded(reader, &mut file, limits.bound())
    });
    let (hash, length) = match outcome {
        Ok(Ok(value)) => value,
        Ok(Err(BoundedCopyError::LimitExceeded)) => return Err(limits.exceeded()),
        Ok(Err(BoundedCopyError::Io(error))) => {
            return Err(SidecarPreparationError::Io(error.to_string()));
        }
        Err(_) => return Err(SidecarPreparationError::ResourceUnavailable),
    };
    if length != expected_length {
        return Err(SidecarPreparationError::LengthMismatch);
    }
    file.sync_all()
        .map_err(|error| SidecarPreparationError::Io(error.to_string()))?;
    drop(file);
    Ok(PreparedSidecar {
        canonical_path: canonical_path.as_str().to_owned(),
        storage_name: hash,
        length,
        media_type: None,
        origin: SidecarOrigin::Copied,
        source_hash: None,
        guard,
    })
}

fn playable_sidecar<R: ResourceProvider>(
    epub: &Epub<R>,
    canonical_path: &EpubPath,
    source: PreparedSidecar,
    staging_dir: &Path,
    limits: Limits,
    reusable: &HashMap<String, ReusableSidecar>,
    target: AudioTarget,
) -> Result<PreparedSidecar, SidecarPreparationError> {
    use crate::media_overlay::transcode::TranscodeError;
    use crate::staging::StagedFile;

    if let Some(fallback) = playable_fallback(epub, canonical_path, staging_dir, limits) {
        return Ok(fallback);
    }
    if let Some(reused) = reusable.get(&source.storage_name) {
        if reused.length > limits.bound() {
            return Err(limits.exceeded());
        }
        return Ok(PreparedSidecar {
            canonical_path: source.canonical_path.clone(),
            storage_name: reused.storage_name.clone(),
            length: reused.length,
            media_type: Some(target.media_type().to_owned()),
            origin: SidecarOrigin::Reused,
            source_hash: Some(source.storage_name.clone()),
            guard: StagedFile::disarmed(),
        });
    }
    let staging_error = |error: io::Error| SidecarPreparationError::Io(error.to_string());
    let (mut output, guard) = create_sidecar_staging_file(staging_dir).map_err(staging_error)?;
    let convert = match target {
        AudioTarget::OggOpus => crate::media_overlay::transcode::transcode_to_opus,
        AudioTarget::Mp3 => crate::media_overlay::transcode::transcode_to_mp3,
    };
    let transcoded =
        convert(source.guard.path(), &mut output, limits.bound()).map_err(|error| match error {
            TranscodeError::Decode(message) => SidecarPreparationError::Decode(message),
            TranscodeError::Encode(message) => SidecarPreparationError::Encode(message),
            TranscodeError::LimitExceeded => limits.exceeded(),
            TranscodeError::Io(error) => SidecarPreparationError::Io(error.to_string()),
        })?;
    output.sync_all().map_err(staging_error)?;
    Ok(PreparedSidecar {
        canonical_path: source.canonical_path.clone(),
        storage_name: transcoded.hash,
        length: transcoded.length,
        media_type: Some(target.media_type().to_owned()),
        origin: SidecarOrigin::Transcoded,
        source_hash: Some(source.storage_name.clone()),
        guard,
    })
}

fn playable_fallback<R: ResourceProvider>(
    epub: &Epub<R>,
    canonical_path: &EpubPath,
    staging_dir: &Path,
    limits: Limits,
) -> Option<PreparedSidecar> {
    use epub_stack::resource::ManifestIdValue;
    use std::collections::{HashSet, VecDeque};

    let resource = epub.resources().resource_by_path(canonical_path)?;
    let mut pending: VecDeque<String> = resource
        .declarations()
        .filter_map(|declaration| declaration.fallback())
        .map(|fallback| fallback.as_str().to_owned())
        .collect();
    let mut visited = HashSet::new();
    while let Some(id) = pending.pop_front() {
        if visited.len() == MAX_FALLBACK_DECLARATIONS {
            break;
        }
        if !visited.insert(id.clone()) {
            continue;
        }
        let Some(declaration) = epub.resources().declarations().find(
            |declaration| matches!(declaration.id(), ManifestIdValue::Valid(value) if *value == id),
        ) else {
            continue;
        };
        if let Some(next) = declaration.fallback() {
            pending.push_back(next.as_str().to_owned());
        }
        let Some(fallback) = declaration.resource() else {
            continue;
        };
        let (Some(path), Some(media_type)) = (
            fallback.local_path(),
            crate::snapshot::agreed_declared_media_mime(fallback),
        ) else {
            continue;
        };
        if path == canonical_path {
            continue;
        }
        let Ok(mut copied) = copy_resource(epub, path, staging_dir, limits) else {
            continue;
        };
        let source = crate::media_overlay::transcode::source_audio(copied.guard.path());
        if READING_ENGINE_AUDIO.conversion(source).is_some() {
            continue;
        }
        copied.canonical_path = canonical_path.as_str().to_owned();
        copied.media_type = Some(media_type);
        copied.origin = SidecarOrigin::Fallback;
        return Some(copied);
    }
    None
}

enum BoundedCopyError {
    LimitExceeded,
    Io(io::Error),
}

impl From<io::Error> for BoundedCopyError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn copy_bounded(
    reader: &mut dyn Read,
    writer: &mut File,
    limit: u64,
) -> Result<(String, u64), BoundedCopyError> {
    let mut hasher = blake3::Hasher::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let read_u64 = u64::try_from(read).map_err(|_| BoundedCopyError::LimitExceeded)?;
        size = size
            .checked_add(read_u64)
            .ok_or(BoundedCopyError::LimitExceeded)?;
        if size > limit {
            return Err(BoundedCopyError::LimitExceeded);
        }
        writer.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

fn create_sidecar_staging_file(staging_dir: &Path) -> io::Result<(File, StagedFile)> {
    for _ in 0..16 {
        let id = crate::random_id(16).map_err(io::Error::other)?;
        let path = staging_dir.join(format!("{id}.media-sidecar.part"));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((file, StagedFile::new(path))),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(
        "could not allocate a sidecar staging file",
    ))
}

#[cfg(test)]
mod tests;
