//! Downloading, verifying and installing one pack revision.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use elephant_ladder_dictionary_pack::{
    AudioCollection, AudioDelivery, Catalog, CatalogAsset, CatalogPack, DictionaryPack,
};
use sha2::{Digest, Sha256};

use super::catalog;
use crate::storage::{
    self, DictionaryAudioChoice, DictionaryOperationPhase, DictionaryPackType, DictionaryViewType,
    NewDictionaryInstallation, NewDictionaryView,
};

/// Free space kept beyond the pack itself, so an install cannot fill the device.
const FREE_SPACE_RESERVE_BYTES: u64 = 512 * 1024 * 1024;

const DOWNLOAD_CHUNK_BYTES: usize = 1024 * 1024;

/// Live state of one operation. The worker owns this, so a dropped request cannot stop it.
pub(crate) struct OperationHandle {
    pub operation_id: String,
    pub corpus_language: String,
    pub audio: DictionaryAudioChoice,
    pub cancelled: AtomicBool,
    progress: Mutex<Progress>,
}

#[derive(Clone)]
pub(crate) struct Progress {
    pub phase: DictionaryOperationPhase,
    pub bytes_completed: u64,
    pub bytes_total: u64,
    pub failure: Option<String>,
}

impl OperationHandle {
    pub(crate) fn new(
        operation_id: String,
        corpus_language: String,
        audio: DictionaryAudioChoice,
    ) -> Self {
        Self {
            operation_id,
            corpus_language,
            audio,
            cancelled: AtomicBool::new(false),
            progress: Mutex::new(Progress {
                phase: DictionaryOperationPhase::Catalog,
                bytes_completed: 0,
                bytes_total: 0,
                failure: None,
            }),
        }
    }

    pub(crate) fn progress(&self) -> Progress {
        self.progress
            .lock()
            .expect("dictionary progress lock poisoned")
            .clone()
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn set(&self, phase: DictionaryOperationPhase, failure: Option<String>) {
        let mut progress = self
            .progress
            .lock()
            .expect("dictionary progress lock poisoned");
        progress.phase = phase;
        progress.failure = failure;
    }

    fn set_total(&self, bytes_total: u64, bytes_completed: u64) {
        let mut progress = self
            .progress
            .lock()
            .expect("dictionary progress lock poisoned");
        progress.bytes_total = bytes_total;
        progress.bytes_completed = bytes_completed;
    }

    fn advance(&self, bytes: u64) {
        let mut progress = self
            .progress
            .lock()
            .expect("dictionary progress lock poisoned");
        progress.bytes_completed = progress.bytes_completed.saturating_add(bytes);
    }
}

/// Runs one install to completion.
///
/// Every failure leaves the registry untouched; only the install step makes a pack visible.
/// `keep_partial` marks a failure `interrupted` rather than `failed` and keeps the staging
/// directory, which is how an automatic resume survives an offline launch.
pub(super) async fn install(
    storage: &crate::storage::StorageHandle,
    handle: &Arc<OperationHandle>,
    catalog_url: &str,
    keep_partial: bool,
) -> Result<(), String> {
    let client = match catalog::client() {
        Ok(client) => client,
        Err(error) => return Err(error),
    };
    let outcome = run(storage, handle, catalog_url, &client).await;
    let progress = handle.progress();
    let phase = match &outcome {
        Ok(()) => DictionaryOperationPhase::Completed,
        Err(error) if error.is_empty() => DictionaryOperationPhase::Cancelled,
        Err(_) if keep_partial => DictionaryOperationPhase::Interrupted,
        Err(_) => DictionaryOperationPhase::Failed,
    };
    let failure = match &outcome {
        Err(error) if !error.is_empty() => Some(error.clone()),
        _ => None,
    };
    handle.set(phase, failure.clone());
    record(
        storage,
        handle,
        phase,
        progress.bytes_completed,
        progress.bytes_total,
        failure.as_deref(),
    );
    if phase == DictionaryOperationPhase::Cancelled || phase == DictionaryOperationPhase::Failed {
        // A stop or a real failure should not leave gigabytes behind; an interrupted
        // operation keeps its partial files for the next attempt.
        if let Ok(staging) = staging_directory(storage, &handle.operation_id) {
            let _ = fs::remove_dir_all(staging);
        }
    }
    outcome
}

/// Installs one language: its corpus and the pronunciations that come with it.
///
/// This is one download rather than a sequence the caller drives, so a restart resumes the
/// language rather than whichever pack happened to be in flight, and the packs that are
/// already installed are not asked for again.
async fn run(
    storage: &crate::storage::StorageHandle,
    handle: &Arc<OperationHandle>,
    catalog_url: &str,
    client: &reqwest::Client,
) -> Result<(), String> {
    let catalog = catalog::load_catalog(client, catalog_url).await?;
    let offered = resolve(&catalog, &handle.corpus_language, handle.audio)?;
    let installed = {
        let guard = storage.lock().expect("storage lock poisoned");
        guard.dictionary_installations()?
    };
    let packs = offered
        .into_iter()
        .filter(|pack| !is_installed(&installed, pack))
        .collect::<Vec<_>>();
    if packs.is_empty() {
        // The language is already installed the way it was asked for.
        return Ok(());
    }

    phase(storage, handle, DictionaryOperationPhase::Preflight);
    let staging = staging_directory(storage, &handle.operation_id)?;
    fs::create_dir_all(&staging)
        .map_err(|error| format!("could not create the dictionary staging directory: {error}"))?;
    let available = fs2::available_space(&staging)
        .map_err(|error| format!("could not read free space: {error}"))?;
    let wanted_bytes = packs
        .iter()
        .map(|pack| pack.release().installed_bytes)
        .fold(0u64, u64::saturating_add);
    if available < wanted_bytes.saturating_add(FREE_SPACE_RESERVE_BYTES) {
        return Err("there is not enough free space for this dictionary".to_owned());
    }
    // One total for the whole language, so progress reads as the single download it is.
    handle.set_total(wanted_bytes, 0);

    phase(storage, handle, DictionaryOperationPhase::Downloading);
    // Each pack stages under its own revision, so a resume can tell their partial files
    // apart and a pack is installed by one rename of its own directory.
    let staged = |pack: &CatalogPack| staging.join(pack_directory(pack));
    for pack in &packs {
        let directory = staged(pack);
        fs::create_dir_all(&directory).map_err(|error| {
            format!("could not create the dictionary staging directory: {error}")
        })?;
        for asset in &pack.release().assets {
            download(client, handle, &directory, asset).await?;
            record_progress(storage, handle);
        }
    }

    phase(storage, handle, DictionaryOperationPhase::Verifying);
    for pack in &packs {
        admit(pack, &staged(pack))?;
    }
    if handle.stopped() {
        return Err(String::new());
    }

    phase(storage, handle, DictionaryOperationPhase::Installing);
    for pack in &packs {
        place(storage, pack, &staged(pack))?;
    }
    // Only orphaned partial files of revisions the catalog no longer offers are left here.
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

/// Accepts a staged pack as what the catalog named.
///
/// Every asset was hashed against the catalog's declared digest as it arrived, so admission
/// checks identity, sizes, schema and metadata rather than hashing gigabytes a second time.
/// This is the same admission every later launch performs.
fn admit(pack: &CatalogPack, staging: &Path) -> Result<(), String> {
    for asset in &pack.release().assets {
        let part = staging.join(format!("{}.part", asset.file_name));
        let final_path = staging.join(&asset.file_name);
        fs::rename(&part, &final_path).map_err(|error| {
            format!(
                "could not finish the dictionary asset {}: {error}",
                asset.file_name
            )
        })?;
    }
    let expected = pack.expected_pack();
    let admitted = match pack {
        CatalogPack::Corpus(_) => DictionaryPack::open_installed(staging, &expected).map(drop),
        CatalogPack::Audio(_) => AudioCollection::open_installed(staging, &expected).map(drop),
    };
    admitted.map_err(|error| format!("the downloaded dictionary is invalid: {error}"))
}

/// Moves a staged pack into place and registers it, which is what makes it visible.
fn place(
    storage: &crate::storage::StorageHandle,
    pack: &CatalogPack,
    staging: &Path,
) -> Result<(), String> {
    let release = pack.release();
    let pack_id = release.pack_id.as_str();
    let pack_revision = release.pack_revision.to_string();
    let installed = {
        let guard = storage.lock().expect("storage lock poisoned");
        storage::dictionary_installed_directory(
            guard.dictionaries_directory(),
            pack_id,
            &pack_revision,
        )?
    };
    if let Some(parent) = installed.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create the dictionary directory: {error}"))?;
    }
    let _ = fs::remove_dir_all(&installed);
    // One rename on the same filesystem, so a multi-gigabyte pack is never on disk twice.
    fs::rename(staging, &installed)
        .map_err(|error| format!("could not install the dictionary: {error}"))?;
    sync_directory(&installed);

    let (pack_type, corpus_language, bilingual) = match pack {
        CatalogPack::Corpus(corpus) => (
            DictionaryPackType::Corpus,
            &corpus.corpus_language,
            corpus
                .bilingual_targets
                .iter()
                .map(|target| {
                    (
                        format!("{}-{target}", corpus.corpus_language),
                        target.clone(),
                    )
                })
                .collect::<Vec<_>>(),
        ),
        CatalogPack::Audio(audio) => (
            DictionaryPackType::Audio,
            &audio.corpus_language,
            Vec::new(),
        ),
    };
    // Audio collections add pronunciations to the corpus of their language; they are
    // not dictionaries a reader looks words up in.
    let mut views = Vec::new();
    if pack_type == DictionaryPackType::Corpus {
        views.push(NewDictionaryView {
            view_id: corpus_language,
            view_type: DictionaryViewType::Monolingual,
            target_language: None,
        });
    }
    for (view_id, target) in &bilingual {
        views.push(NewDictionaryView {
            view_id,
            view_type: DictionaryViewType::Bilingual,
            target_language: Some(target),
        });
    }
    let mut guard = storage.lock().expect("storage lock poisoned");
    guard.register_dictionary_installation(
        &NewDictionaryInstallation {
            pack_id,
            pack_revision: &pack_revision,
            pack_type,
            corpus_language,
            manifest_sha256: &pack.expected_pack().manifest_sha256.to_string(),
            installed_bytes: release.installed_bytes,
        },
        &views,
    )
}

fn is_installed(installed: &[storage::DictionaryInstallation], pack: &CatalogPack) -> bool {
    let release = pack.release();
    installed.iter().any(|installation| {
        installation.pack_id == release.pack_id.as_str()
            && installation.pack_revision == release.pack_revision.to_string()
    })
}

/// A staging directory name of one pack revision, from identity the catalog validated.
fn pack_directory(pack: &CatalogPack) -> String {
    let release = pack.release();
    format!("{}-{}", release.pack_id.as_str(), release.pack_revision)
}

/// The packs one language installs: its corpus, then the pronunciations it comes with.
///
/// Pronunciations are part of a language rather than a separate choice, so the only decision
/// is where they play from; a language the catalog has no recordings for installs its corpus
/// alone, and a delivery it does not offer falls back to the one it does.
fn resolve<'a>(
    catalog: &'a Catalog,
    corpus_language: &str,
    audio: DictionaryAudioChoice,
) -> Result<Vec<&'a CatalogPack>, String> {
    let corpus = catalog
        .packs
        .iter()
        .find(|pack| match pack {
            CatalogPack::Corpus(corpus) => corpus.corpus_language == corpus_language,
            CatalogPack::Audio(_) => false,
        })
        .ok_or_else(|| "the catalog does not offer that dictionary".to_owned())?;
    let collection = |wanted: AudioDelivery| {
        catalog.packs.iter().find(|pack| match pack {
            CatalogPack::Audio(audio) => {
                audio.corpus_language == corpus_language && audio.delivery == wanted
            }
            CatalogPack::Corpus(_) => false,
        })
    };
    let (asked, other) = match audio {
        DictionaryAudioChoice::Stored => (AudioDelivery::Bundled, AudioDelivery::Remote),
        DictionaryAudioChoice::Streamed => (AudioDelivery::Remote, AudioDelivery::Bundled),
    };
    Ok(std::iter::once(corpus)
        .chain(collection(asked).or_else(|| collection(other)))
        .collect())
}

/// Streams one asset into `<file_name>.part`, resuming a partial file when the server allows.
///
/// Bytes arrive on the async runtime but are written and hashed on a blocking worker, since
/// hashing gigabytes is real CPU work that must not sit on an executor thread.
async fn download(
    client: &reqwest::Client,
    handle: &Arc<OperationHandle>,
    staging: &Path,
    asset: &CatalogAsset,
) -> Result<(), String> {
    let part = staging.join(format!("{}.part", asset.file_name));
    let mut digest = Sha256::new();
    let mut completed = 0u64;
    if let Ok(existing) = File::open(&part) {
        let limit = asset.size_bytes;
        let (hashed, bytes) = tokio::task::spawn_blocking(move || rehash(existing, limit))
            .await
            .map_err(|_| "the dictionary download worker failed".to_owned())??;
        digest = hashed;
        completed = bytes;
        handle.advance(bytes);
    }
    if completed == asset.size_bytes {
        return settle(&part, finish(digest, asset, completed));
    }

    let mut response = catalog::fetch(
        client,
        &asset.url,
        (completed > 0).then_some(completed),
        catalog::Destination::Release,
    )
    .await?;
    let status = response.status().as_u16();
    let (mut file, mut digest, mut completed) = if status == 206 {
        (open_append(&part)?, digest, completed)
    } else if status == 200 {
        // The server ignored the range; start over rather than splice mismatched bytes.
        if completed > 0 {
            handle.advance(0u64.wrapping_sub(completed));
        }
        (File::create(&part).map_err(io_error)?, Sha256::new(), 0)
    } else {
        return Err(format!("the dictionary download returned HTTP {status}"));
    };

    let mut buffer: Vec<u8> = Vec::with_capacity(DOWNLOAD_CHUNK_BYTES);
    loop {
        if handle.stopped() {
            return Err(String::new());
        }
        let chunk = response
            .chunk()
            .await
            .map_err(|error| format!("the dictionary download failed: {error}"))?;
        let Some(chunk) = chunk else { break };
        completed = completed.saturating_add(chunk.len() as u64);
        if completed > asset.size_bytes {
            return Err(format!(
                "the dictionary asset {} is larger than the catalog declares",
                asset.file_name
            ));
        }
        handle.advance(chunk.len() as u64);
        buffer.extend_from_slice(&chunk);
        if buffer.len() >= DOWNLOAD_CHUNK_BYTES {
            (file, digest) = absorb(file, digest, std::mem::take(&mut buffer)).await?;
            buffer.reserve(DOWNLOAD_CHUNK_BYTES);
        }
    }
    if !buffer.is_empty() {
        (file, digest) = absorb(file, digest, buffer).await?;
    }
    tokio::task::spawn_blocking(move || file.sync_all().map_err(io_error))
        .await
        .map_err(|_| "the dictionary download worker failed".to_owned())??;
    settle(&part, finish(digest, asset, completed))
}

/// Discards a partial file whose bytes do not match the catalog.
///
/// Resuming is only worth it while the bytes on disk are known good; keeping a mismatched
/// file would make every later attempt fail the same way.
fn settle(part: &Path, outcome: Result<(), String>) -> Result<(), String> {
    if outcome.is_err() {
        let _ = fs::remove_file(part);
    }
    outcome
}

/// Writes one buffer and folds it into the running digest, off the async runtime.
async fn absorb(
    mut file: File,
    mut digest: Sha256,
    buffer: Vec<u8>,
) -> Result<(File, Sha256), String> {
    tokio::task::spawn_blocking(move || {
        digest.update(&buffer);
        file.write_all(&buffer).map_err(io_error)?;
        Ok::<_, String>((file, digest))
    })
    .await
    .map_err(|_| "the dictionary download worker failed".to_owned())?
}

fn finish(digest: Sha256, asset: &CatalogAsset, completed: u64) -> Result<(), String> {
    if completed != asset.size_bytes {
        return Err(format!(
            "the dictionary asset {} ended early",
            asset.file_name
        ));
    }
    if format!("{:x}", digest.finalize()) != asset.sha256.to_string() {
        return Err(format!(
            "the dictionary asset {} does not match the catalog",
            asset.file_name
        ));
    }
    Ok(())
}

/// Hashes an existing partial file, ignoring anything beyond the declared size.
fn rehash(mut file: File, limit: u64) -> Result<(Sha256, u64), String> {
    let mut digest = Sha256::new();
    let mut hashed = 0u64;
    let mut chunk = vec![0u8; DOWNLOAD_CHUNK_BYTES];
    loop {
        let read = file.read(&mut chunk).map_err(io_error)?;
        if read == 0 {
            break;
        }
        let usable = read.min(usize::try_from(limit - hashed).unwrap_or(read));
        digest.update(&chunk[..usable]);
        hashed += usable as u64;
        if hashed >= limit {
            break;
        }
    }
    Ok((digest, hashed))
}

fn open_append(path: &Path) -> Result<File, String> {
    let mut file = File::options().write(true).open(path).map_err(io_error)?;
    file.seek(SeekFrom::End(0)).map_err(io_error)?;
    Ok(file)
}

fn io_error(error: std::io::Error) -> String {
    format!("the dictionary download failed: {error}")
}

fn sync_directory(path: &Path) {
    let _ = crate::storage::fsync_directory(path);
}

fn staging_directory(
    storage: &crate::storage::StorageHandle,
    operation_id: &str,
) -> Result<PathBuf, String> {
    let guard = storage.lock().expect("storage lock poisoned");
    storage::dictionary_staging_directory(guard.dictionaries_directory(), operation_id)
}

fn phase(
    storage: &crate::storage::StorageHandle,
    handle: &Arc<OperationHandle>,
    phase: DictionaryOperationPhase,
) {
    handle.set(phase, None);
    record_progress(storage, handle);
}

fn record_progress(storage: &crate::storage::StorageHandle, handle: &Arc<OperationHandle>) {
    let progress = handle.progress();
    record(
        storage,
        handle,
        progress.phase,
        progress.bytes_completed,
        progress.bytes_total,
        progress.failure.as_deref(),
    );
}

fn record(
    storage: &crate::storage::StorageHandle,
    handle: &Arc<OperationHandle>,
    phase: DictionaryOperationPhase,
    bytes_completed: u64,
    bytes_total: u64,
    failure: Option<&str>,
) {
    let mut guard = storage.lock().expect("storage lock poisoned");
    if let Err(error) = guard.update_dictionary_operation(
        &handle.operation_id,
        phase,
        bytes_completed,
        bytes_total,
        failure,
    ) {
        tracing::warn!(event = "dictionary.operation_not_recorded", error = %error);
    }
}
