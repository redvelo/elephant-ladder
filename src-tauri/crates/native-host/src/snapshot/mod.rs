//! Immutable publication snapshots and the products derived from them.
//!
//! One snapshot binds the exact source blob and selected rendition, validated
//! publication facts, a sealed response manifest, and the current reading
//! contract. Snapshots are never edited in place, and serving never broadens or
//! reinterprets a sealed response decision. The registry retains at most one
//! most-recently-used snapshot without a live lease; eviction never removes
//! durable state.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path as FsPath;
use std::sync::{Arc, Mutex, atomic::Ordering};
use std::time::Instant;

use epub_navigator::kernel::{ReadingOrderOccurrence, project_reading_order};
use epub_stack::{EpubPath, EpubZip, resource::ResourceAddress};
use serde_json::Value;

use crate::media_overlay::backing as media_overlay_backing;
use crate::media_overlay::derive as media_overlay_derive;
use crate::search::search_analysis_limits;
use crate::{
    MAX_FACTS_SNAPSHOT_BYTES, MAX_RESPONSE_RESOURCES, PRODUCTION_READING_REVISION, StagedCover,
    duration_milliseconds, media_overlay, publication_policy_relevance, publication_title,
    stage_selected_cover, storage,
};

/// An EPUB opened from an app-owned archive file.
mod cache;
mod manifest;

pub(crate) use cache::TransformedOutcome;
pub use cache::{ResourceCache, TransformCache, TransformCacheEntry, TransformCacheKey};
pub(crate) use manifest::agreed_declared_media_mime;
#[cfg(test)]
pub(crate) use manifest::normalized_media_mime;
pub use manifest::{
    Foreground, ForegroundPreparationState, Measurement, ResponseManifest, ResponseRule,
    response_manifest,
};
use manifest::{validate_effective_response_manifest, validate_response_manifest};

pub type ImportedEpub = epub_stack::Epub<epub_stack::EpubZip<std::io::BufReader<std::fs::File>>>;

pub struct SnapshotEntry {
    pub snapshot_id: Arc<str>,
    pub facts: Arc<Value>,
    pub reading_order: Arc<Vec<ReadingOrderOccurrence>>,
    pub epub: Arc<ImportedEpub>,
    pub responses: Arc<ResponseManifest>,
    pub(crate) media_overlay: Arc<media_overlay::MediaOverlayDescription>,
    pub(crate) media_overlay_timeline_files: Arc<BTreeMap<u32, storage::MediaOverlayTimelineFile>>,
    pub(crate) media_overlay_capability: media_overlay::MediaOverlayCapability,
    pub media_sidecars: Arc<BTreeMap<String, storage::MediaSidecar>>,
    pub policy_relevance: storage::PublicationPolicyRelevance,
    pub transforms: Mutex<TransformCache>,
    pub resources: Mutex<ResourceCache>,
}

#[derive(Default)]
pub struct SnapshotRegistry {
    // A single current/warm entry gives the process a stricter 24 MiB transformed cache
    // bound than the conceptual 32 MiB process allowance.
    pub(crate) warm: Option<Arc<SnapshotEntry>>,
}

pub(crate) struct LoadedSnapshotEntry {
    pub(crate) entry: SnapshotEntry,
    pub(crate) zip_open_ms: u64,
    pub(crate) rendition_open_ms: u64,
}

pub(crate) fn load_snapshot_entry(
    selected: Box<storage::SelectedPublication>,
) -> Result<LoadedSnapshotEntry, String> {
    let mut selected = *selected;
    let zip_started = Instant::now();
    let zip = EpubZip::open(&selected.blob_path)
        .map_err(|error| format!("could not open stored EPUB: {error}"))?;
    let zip_open_ms = duration_milliseconds(zip_started.elapsed());
    let rendition_started = Instant::now();
    let epub = zip
        .default_rendition()
        .map_err(|error| format!("could not open stored rendition: {error}"))?;
    let rendition_open_ms = duration_milliseconds(rendition_started.elapsed());
    let publication_facts = epub_navigator::facts::PublicationFacts::of(&epub)
        .map_err(|error| format!("could not recreate stored facts: {error}"))?;
    let reading_order = project_reading_order(&publication_facts)
        .map_err(|error| format!("could not project stored reading order: {error}"))?;
    let current_facts = serde_json::to_value(&publication_facts)
        .map_err(|error| format!("could not serialize current stored facts: {error}"))?;
    if current_facts != selected.facts {
        return Err("stored publication facts do not match the immutable EPUB".to_owned());
    }
    validate_stored_reading_products(&selected.facts, &selected.responses)?;
    validate_effective_response_manifest(&selected.responses, &reading_order)?;
    // The capability was derived and validated at import, so opening reads it instead of parsing
    // every timeline. Only its agreement with the stored timeline files is rechecked here.
    let timelines_match = match (
        &selected.media_overlay_capability,
        &selected.media_overlay.playback,
    ) {
        (
            media_overlay::MediaOverlayCapability::Available { .. },
            media_overlay::MediaOverlayPlayback::Available { overlays, .. },
        ) => overlays.len() == selected.media_overlay_timeline_files.len(),
        _ => true,
    };
    let media_overlay_capability = if timelines_match {
        selected.media_overlay_capability.clone()
    } else {
        // Narration never decides whether a publication opens.
        tracing::warn!(
            "stored narration timelines do not match the description; narration unavailable"
        );
        selected.media_overlay = media_overlay::representation_limited_description();
        selected.media_overlay_timeline_files = BTreeMap::new();
        media_overlay::MediaOverlayCapability::Unavailable {}
    };
    for (path, sidecar) in &selected.media_sidecars {
        if selected
            .responses
            .resources
            .get(path)
            .is_none_or(|rule| sidecar.media_type.is_none() && rule.size_bytes != sidecar.length)
        {
            return Err("stored media sidecar does not match its response resource".to_owned());
        }
    }
    Ok(LoadedSnapshotEntry {
        entry: SnapshotEntry {
            snapshot_id: Arc::from(selected.snapshot_id),
            facts: Arc::new(selected.facts),
            reading_order: Arc::new(reading_order),
            epub: Arc::new(epub),
            responses: Arc::new(selected.responses),
            media_overlay: Arc::new(selected.media_overlay),
            media_overlay_timeline_files: Arc::new(selected.media_overlay_timeline_files),
            media_overlay_capability,
            media_sidecars: Arc::new(selected.media_sidecars),
            policy_relevance: selected.policy_relevance,
            transforms: Mutex::new(TransformCache::default()),
            resources: Mutex::new(ResourceCache::default()),
        },
        zip_open_ms,
        rendition_open_ms,
    })
}

/// Keeps the database, staging, and the rest of the system writable. This is physical headroom,
/// not a bound on how large a narration library may be.
const SIDECAR_FREE_SPACE_RESERVE_BYTES: u64 = 512 * 1_048_576;

/// Narration sidecars are bounded by storage that actually exists rather than by an invented
/// ceiling, matching how archive imports already reserve against real free space.
pub(crate) fn remaining_sidecar_budget(storage: &storage::Storage) -> Result<u64, String> {
    let available = fs2::available_space(storage.media_sidecars_dir())
        .map_err(|error| format!("could not inspect available sidecar storage: {error}"))?;
    Ok(available.saturating_sub(SIDECAR_FREE_SPACE_RESERVE_BYTES))
}

pub(crate) fn prepare_publication(
    storage: &storage::StorageHandle,
    blob: storage::InstalledBlob,
) -> Result<DerivationMetrics, String> {
    let (staging, sidecar_budget, reusable_sidecars) = {
        let storage = storage.locked()?;
        (
            storage.staging_path().to_path_buf(),
            remaining_sidecar_budget(&storage)?,
            storage.reusable_media_transcodes(media_overlay_backing::transcode_derivation())?,
        )
    };
    let derived = derive_publication(&blob.path, &staging, sidecar_budget, &reusable_sidecars)?;
    let mut metrics = derived.metrics.clone();
    let mut storage = storage.locked()?;
    let installed_cover = derived
        .cover
        .as_ref()
        .map(|cover| {
            storage.install_staged_cover(cover.guard.path(), &cover.hash, cover.size, &cover.mime)
        })
        .transpose()?;
    let commit_started = Instant::now();
    let committed = storage.commit_import_snapshot_with_policy_relevance(
        storage::InstalledImport {
            archive: &blob,
            cover: installed_cover.as_ref(),
        },
        PRODUCTION_READING_REVISION,
        &derived.title,
        storage::PolicyAwareReadingProducts {
            facts: &derived.facts,
            package: &derived.package,
            responses: &derived.responses,
            media_overlay: &derived.media_overlay,
            media_overlay_timelines: &derived.media_overlay_timelines,
            prepared_sidecars: derived.prepared_sidecars,
            policy_relevance: derived.policy_relevance,
        },
    );
    if committed.is_err() {
        let _ = storage.cleanup_unreferenced_covers();
    }
    committed?;
    metrics.commit_ms = duration_milliseconds(commit_started.elapsed());
    Ok(metrics)
}

#[derive(Clone, Default)]
pub(crate) struct DerivationMetrics {
    pub(crate) zip_open_ms: u64,
    pub(crate) rendition_open_ms: u64,
    pub(crate) facts_ms: u64,
    pub(crate) cover_ms: u64,
    pub(crate) analysis_ms: u64,
    pub(crate) response_manifest_ms: u64,
    pub(crate) validation_ms: u64,
    pub(crate) commit_ms: u64,
    pub(crate) sidecar_preparation_ms: u64,
    pub(crate) sidecar_transcoded_count: u64,
}

pub(crate) struct DerivedPublication {
    pub(crate) title: String,
    pub(crate) facts: Value,
    pub(crate) package: epub_stack::package::Package,
    pub(crate) responses: ResponseManifest,
    pub(crate) media_overlay: media_overlay::MediaOverlayDescription,
    pub(crate) media_overlay_timelines: Vec<media_overlay::MediaOverlayTimeline>,
    pub(crate) prepared_sidecars: Vec<media_overlay_backing::PreparedSidecar>,
    pub(crate) policy_relevance: storage::PublicationPolicyRelevance,
    pub(crate) cover: Option<StagedCover>,
    pub(crate) metrics: DerivationMetrics,
}

/// Prepares every audio sidecar, in parallel when there is more than one, then admits them in
/// reading order against the publication budget so the outcome does not depend on scheduling.
fn prepare_audio_sidecars(
    epub: &ImportedEpub,
    archive: &FsPath,
    staging: &FsPath,
    audio_paths: &[String],
    budget: u64,
    reusable: &HashMap<String, media_overlay_backing::ReusableSidecar>,
) -> (
    Vec<media_overlay_backing::PreparedSidecar>,
    std::collections::HashSet<String>,
) {
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(audio_paths.len().max(1));
    let prepare = |epub: &ImportedEpub, canonical_path: &str| {
        let Ok(path) = EpubPath::new(canonical_path) else {
            tracing::warn!(canonical_path, "audio path is not a publication path");
            return None;
        };
        match media_overlay_backing::prepare_audio_sidecar(epub, &path, staging, budget, reusable) {
            Ok(prepared) => Some(prepared),
            Err(error) => {
                tracing::warn!(
                    canonical_path,
                    ?error,
                    "audio could not be prepared; it is served as authored and any narration segment over it becomes unavailable"
                );
                None
            }
        }
    };
    let results = if workers <= 1 {
        audio_paths
            .iter()
            .map(|canonical_path| prepare(epub, canonical_path))
            .collect::<Vec<_>>()
    } else {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots = Mutex::new(
            (0..audio_paths.len())
                .map(|_| None)
                .collect::<Vec<Option<media_overlay_backing::PreparedSidecar>>>(),
        );
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    let Ok(worker_epub) = EpubZip::open(archive)
                        .map_err(|_| ())
                        .and_then(|zip| zip.default_rendition().map_err(|_| ()))
                    else {
                        return;
                    };
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(canonical_path) = audio_paths.get(index) else {
                            break;
                        };
                        let prepared = prepare(&worker_epub, canonical_path);
                        slots.lock().expect("sidecar slots lock poisoned")[index] = prepared;
                    }
                });
            }
        });
        slots.into_inner().expect("sidecar slots lock poisoned")
    };
    let mut remaining = budget;
    let mut prepared = Vec::new();
    let mut failed = std::collections::HashSet::new();
    for (canonical_path, sidecar) in audio_paths.iter().zip(results) {
        match sidecar {
            Some(sidecar) if sidecar.length <= remaining => {
                remaining -= sidecar.length;
                prepared.push(sidecar);
            }
            _ => {
                failed.insert(canonical_path.clone());
            }
        }
    }
    (prepared, failed)
}

pub(crate) fn derive_publication(
    path: &FsPath,
    staging: &FsPath,
    sidecar_budget: u64,
    reusable_sidecars: &HashMap<String, media_overlay_backing::ReusableSidecar>,
) -> Result<DerivedPublication, String> {
    let mut metrics = DerivationMetrics::default();
    let zip_started = Instant::now();
    let zip =
        EpubZip::open(path).map_err(|error| format!("could not open imported EPUB: {error}"))?;
    metrics.zip_open_ms = duration_milliseconds(zip_started.elapsed());
    let rendition_started = Instant::now();
    let epub = zip
        .default_rendition()
        .map_err(|error| format!("could not open imported rendition: {error}"))?;
    metrics.rendition_open_ms = duration_milliseconds(rendition_started.elapsed());
    let facts_started = Instant::now();
    let publication_facts = epub_navigator::facts::PublicationFacts::of(&epub)
        .map_err(|error| format!("could not create imported facts: {error}"))?;
    let reading_order = project_reading_order(&publication_facts)
        .map_err(|error| format!("could not project imported reading order: {error}"))?;
    metrics.facts_ms = duration_milliseconds(facts_started.elapsed());
    let cover_started = Instant::now();
    let cover = stage_selected_cover(&epub, &publication_facts, staging)?;
    metrics.cover_ms = duration_milliseconds(cover_started.elapsed());
    let analysis_started = Instant::now();
    let analysis = epub.analyze_with_limits(search_analysis_limits());
    metrics.analysis_ms = duration_milliseconds(analysis_started.elapsed());
    let policy_relevance = publication_policy_relevance(&analysis);
    let responses_started = Instant::now();
    let responses = response_manifest(&analysis, &reading_order)?;
    validate_effective_response_manifest(&responses, &reading_order)?;
    metrics.response_manifest_ms = duration_milliseconds(responses_started.elapsed());
    let media_overlay_derive::MediaOverlayDerivation {
        mut description,
        mut timelines,
    } = media_overlay_derive::derive_media_overlay_description(&analysis, &reading_order);
    let audio_paths = media_overlay_backing::audio_paths_to_back(
        media_overlay_derive::audio_resources_to_back(&timelines),
        &responses,
    );
    let sidecars_started = Instant::now();
    let (prepared_sidecars, failed_audio_paths) = prepare_audio_sidecars(
        &epub,
        path,
        staging,
        &audio_paths,
        sidecar_budget,
        reusable_sidecars,
    );
    metrics.sidecar_preparation_ms = duration_milliseconds(sidecars_started.elapsed());
    metrics.sidecar_transcoded_count = prepared_sidecars
        .iter()
        .filter(|sidecar| sidecar.origin == media_overlay_backing::SidecarOrigin::Transcoded)
        .count() as u64;
    media_overlay_derive::apply_audio_backing_failures(
        &mut description,
        &mut timelines,
        &failed_audio_paths,
    );
    media_overlay::validate_media_overlay_description(&description)
        .map_err(|error| format!("derived media overlay description is invalid: {error}"))?;
    let validation_started = Instant::now();
    validate_reading_products(&publication_facts, &responses)?;
    metrics.validation_ms = duration_milliseconds(validation_started.elapsed());
    let title = publication_title(publication_facts.package);
    let package = publication_facts.package.clone();
    let facts = serde_json::to_value(&publication_facts)
        .map_err(|error| format!("could not serialize imported facts: {error}"))?;
    Ok(DerivedPublication {
        title,
        facts,
        package,
        responses,
        media_overlay: description,
        media_overlay_timelines: timelines,
        prepared_sidecars,
        policy_relevance,
        cover,
        metrics,
    })
}

pub fn validate_reading_products(
    facts: &epub_navigator::facts::PublicationFacts<'_>,
    responses: &ResponseManifest,
) -> Result<(), String> {
    let facts_bytes = serde_json::to_vec(facts)
        .map_err(|error| format!("could not validate publication facts: {error}"))?;
    if facts_bytes.len() > MAX_FACTS_SNAPSHOT_BYTES {
        return Err("publication facts exceed the 24 MiB storage bound".to_owned());
    }
    if facts.resources.resources().len() > MAX_RESPONSE_RESOURCES {
        return Err("publication facts exceed the resource bound".to_owned());
    }
    let mut local_paths = HashSet::with_capacity(facts.resources.resources().len());
    for resource in facts.resources.resources() {
        if let ResourceAddress::Local(path) = resource.address()
            && !local_paths.insert(path.as_str())
        {
            return Err("publication facts contain a duplicate local path".to_owned());
        }
    }
    validate_response_manifest(responses)?;
    if responses.resources.len() != local_paths.len()
        || responses
            .resources
            .keys()
            .any(|path| !local_paths.contains(path.as_str()))
    {
        return Err("response manifest does not cover the local resource inventory".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_stored_reading_products(
    facts: &Value,
    responses: &ResponseManifest,
) -> Result<(), String> {
    let facts_bytes = serde_json::to_vec(facts)
        .map_err(|error| format!("could not validate publication facts: {error}"))?;
    if facts_bytes.len() > MAX_FACTS_SNAPSHOT_BYTES {
        return Err("publication facts exceed the 24 MiB storage bound".to_owned());
    }
    let facts = facts
        .as_object()
        .ok_or_else(|| "publication facts must be an object".to_owned())?;
    let expected_keys = [
        "package",
        "navigation",
        "resources",
        "navigationLoading",
        "navigationTargets",
    ];
    if facts.len() != expected_keys.len()
        || expected_keys.iter().any(|key| !facts.contains_key(*key))
    {
        return Err("publication facts have an unexpected top-level shape".to_owned());
    }
    let index = facts
        .get("resources")
        .and_then(Value::as_object)
        .ok_or_else(|| "publication facts have no resource inventory".to_owned())?;
    let resources = index
        .get("resources")
        .and_then(Value::as_array)
        .ok_or_else(|| "publication facts have no resource inventory".to_owned())?;
    let package_path = index
        .get("packagePath")
        .and_then(Value::as_str)
        .ok_or_else(|| "publication facts have no package path".to_owned())?;
    EpubPath::new(package_path)
        .map_err(|_| "publication facts contain an invalid package path".to_owned())?;
    if resources.len() > MAX_RESPONSE_RESOURCES {
        return Err("publication facts exceed the resource bound".to_owned());
    }
    let mut local_paths = HashSet::with_capacity(resources.len());
    for resource in resources {
        let address = resource
            .as_object()
            .and_then(|resource| resource.get("address"))
            .and_then(Value::as_object)
            .ok_or_else(|| "publication facts contain an invalid resource address".to_owned())?;
        match address.get("kind").and_then(Value::as_str) {
            Some("local") => {
                let path = address
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "publication facts contain an invalid local path".to_owned())?;
                EpubPath::new(path)
                    .map_err(|_| "publication facts contain an invalid local path".to_owned())?;
                if !local_paths.insert(path) {
                    return Err("publication facts contain a duplicate local path".to_owned());
                }
            }
            Some("remote" | "data" | "external") => {}
            _ => return Err("publication facts contain an unknown resource address".to_owned()),
        }
    }

    validate_response_manifest(responses)?;
    if responses.resources.len() != local_paths.len()
        || responses
            .resources
            .keys()
            .any(|path| !local_paths.contains(path.as_str()))
    {
        return Err("response manifest does not cover the local resource inventory".to_owned());
    }
    Ok(())
}
