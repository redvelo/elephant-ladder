//! Catalog, installation, and the operations that report their progress.

#[cfg(debug_assertions)]
use super::expected_pack;
use super::{DictionaryInstallRequest, completed, dictionary_unavailable};
use crate::Arc;
#[cfg(debug_assertions)]
use crate::FsPath;
use crate::ReaderState;
use crate::http::auth::is_hex_id;
use crate::http::{HostError, Trusted, TrustedRead, TrustedWrite};
use crate::random_id;
use crate::request_library_document;
#[cfg(debug_assertions)]
use crate::storage;
use crate::storage::{DictionaryAudioChoice, DictionaryOperationPhase, DictionaryPackType};
#[cfg(debug_assertions)]
use crate::storage::{DictionaryInstallation, DictionaryViewType};
#[cfg(debug_assertions)]
use crate::storage::{NewDictionaryInstallation, NewDictionaryView};
use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
#[cfg(debug_assertions)]
use elephant_ladder_dictionary_pack::{AUDIO_MANIFEST_FILE, PACK_MANIFEST_FILE};
#[cfg(debug_assertions)]
use elephant_ladder_dictionary_pack::{AudioCollection, DictionaryPack};
use elephant_ladder_dictionary_pack::{CatalogPack, PackId};
#[cfg(debug_assertions)]
use sha2::Digest;
#[cfg(debug_assertions)]
use sha2::Sha256;
#[cfg(debug_assertions)]
use std::fs;
use std::str::FromStr;

/// How long a new install waits for the previous one to release its admission.
const ADMISSION_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) fn management_denied(message: impl Into<String>) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({"status": "failed", "message": message.into()})),
    )
        .into_response()
}

/// Every installed pack the catalog offers, with what is installed right now.
pub(crate) async fn catalog_listing(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: TrustedRead,
) -> Response {
    if let Err(message) = request_library_document(&headers, &state) {
        return management_denied(message);
    }
    let catalog_url = match super::catalog::configured_url() {
        Ok(url) => url,
        Err(error) => {
            tracing::warn!(event = "dictionary.catalog_unconfigured", error = %error);
            return HostError::Unavailable("dictionary-catalog-unavailable").into_response();
        }
    };
    let loaded = match super::catalog::client() {
        Ok(client) => super::catalog::load_catalog(&client, &catalog_url).await,
        Err(error) => Err(error),
    };
    let catalog = match loaded {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(event = "dictionary.catalog_unavailable", error = %error);
            return dictionary_unavailable(StatusCode::SERVICE_UNAVAILABLE, "catalog-unavailable");
        }
    };
    let installed = match installed_summary(&state).await {
        Ok(installed) => installed,
        Err(response) => return *response,
    };
    let packs = catalog
        .packs
        .iter()
        .map(|pack| {
            let release = pack.release();
            let revision = release.pack_revision.to_string();
            let installed = installed.iter().any(|value| {
                value["packId"] == release.pack_id.as_str() && value["packRevision"] == revision
            });
            match pack {
                CatalogPack::Corpus(corpus) => serde_json::json!({
                    "packType": "corpus",
                    "packId": release.pack_id,
                    "packRevision": revision,
                    "corpusLanguage": corpus.corpus_language,
                    "edition": corpus.wiktionary_edition,
                    "dumpDate": corpus.wiktionary_dump_date,
                    "monolingual": corpus.monolingual,
                    "bilingualTargets": corpus.bilingual_targets,
                    "licenses": corpus.licenses,
                    "installedBytes": release.installed_bytes,
                    "installed": installed,
                }),
                CatalogPack::Audio(audio) => serde_json::json!({
                    "packType": "audio",
                    "packId": release.pack_id,
                    "packRevision": revision,
                    "corpusLanguage": audio.corpus_language,
                    "delivery": audio.delivery.as_str(),
                    "recordings": audio.recordings.available,
                    "installedBytes": release.installed_bytes,
                    "installed": installed,
                }),
            }
        })
        .collect::<Vec<_>>();
    completed(serde_json::json!({
        "status": "completed",
        "catalogRevision": catalog.catalog_revision,
        "packs": packs,
    }))
}

pub(crate) async fn installations(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: TrustedRead,
) -> Response {
    if let Err(message) = request_library_document(&headers, &state) {
        return management_denied(message);
    }
    match installed_summary(&state).await {
        Ok(installed) => {
            completed(serde_json::json!({"status": "completed", "installations": installed}))
        }
        Err(response) => *response,
    }
}

pub(crate) async fn installed_summary(
    state: &ReaderState,
) -> Result<Vec<serde_json::Value>, Box<Response>> {
    let storage = state.storage.clone();
    let read = tokio::task::spawn_blocking(move || {
        let storage = storage.locked()?;
        storage.dictionary_installations()
    })
    .await;
    // Whether recordings play from the device or from Commons is a fact of the opened
    // collection rather than of the registry, and it is what the listing is read for when
    // the catalog cannot be reached.
    let set = state.dictionary.lease();
    match read {
        Ok(Ok(installations)) => Ok(installations
            .into_iter()
            .map(|installation| {
                let delivery = set
                    .audio
                    .get(&installation.corpus_language)
                    .filter(|collection| {
                        collection.pack_revision().to_string() == installation.pack_revision
                    })
                    .map(|collection| collection.metadata().delivery.as_str());
                serde_json::json!({
                    "packType": match installation.pack_type {
                        DictionaryPackType::Corpus => "corpus",
                        DictionaryPackType::Audio => "audio",
                    },
                    "packId": installation.pack_id,
                    "packRevision": installation.pack_revision,
                    "corpusLanguage": installation.corpus_language,
                    "installedBytes": installation.installed_bytes,
                    "delivery": delivery,
                })
            })
            .collect()),
        _ => Err(Box::new(
            HostError::Internal("dictionary-installed-read").into_response(),
        )),
    }
}

/// Starts one install and returns its operation identifier; progress is polled.
pub(crate) async fn install(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: TrustedWrite,
    body: Result<Json<DictionaryInstallRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(message) = request_library_document(&headers, &state) {
        return management_denied(message);
    }
    let Ok(Json(request)) = body else {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    };
    let Ok(audio) = DictionaryAudioChoice::parse(&request.audio) else {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    };
    if !super::valid_language(&request.corpus_language) || request.corpus_language.len() > 35 {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    }
    // An install keeps its permit until the finished language has been reopened, which is
    // after its operation already reports a terminal phase, so admission waits out that
    // handoff rather than refusing an install that arrives during it.
    let admission = state.dictionary_admission.clone();
    let permit = match tokio::time::timeout(ADMISSION_WAIT, admission.acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        Ok(Err(_)) => return HostError::Internal("dictionary-operation-start").into_response(),
        Err(_) => return management_denied("a dictionary operation is already active"),
    };
    let catalog_url = match super::catalog::configured_url() {
        Ok(url) => url,
        Err(_) => return HostError::Unavailable("dictionary-catalog-unavailable").into_response(),
    };
    let Ok(operation_id) = random_id(16) else {
        return HostError::Internal("dictionary-operation-start").into_response();
    };
    let storage = state.storage.clone();
    let begun = {
        let storage = storage.clone();
        let operation_id = operation_id.clone();
        let corpus_language = request.corpus_language.clone();
        tokio::task::spawn_blocking(move || {
            let mut storage = storage.locked()?;
            storage.begin_dictionary_operation(&operation_id, &corpus_language, audio, None)
        })
        .await
    };
    let operation_id = match begun {
        Ok(Ok(resumed)) => resumed,
        Ok(Err(error)) => return management_denied(error),
        Err(_) => return HostError::Internal("dictionary-operation-start").into_response(),
    };
    let handle = Arc::new(super::acquire::OperationHandle::new(
        operation_id.clone(),
        request.corpus_language,
        audio,
    ));
    state.dictionary_operations.insert(handle.clone());
    let operations = state.dictionary_operations.clone();
    let dictionary = state.dictionary.clone();
    tokio::spawn(async move {
        // The task owns the permit, so a dropped request cannot abandon an install.
        let _permit = permit;
        let outcome = super::acquire::install(&storage, &handle, &catalog_url, false).await;
        match &outcome {
            Ok(()) => {
                let reopened = {
                    let mut storage = storage.lock().expect("storage lock poisoned");
                    crate::open_dictionaries(&mut storage)
                };
                dictionary.store(reopened);
                tracing::info!(
                    event = "dictionary.installed",
                    language = %handle.corpus_language
                );
            }
            Err(error) if error.is_empty() => {
                tracing::info!(
                    event = "dictionary.install_cancelled",
                    language = %handle.corpus_language
                );
            }
            Err(error) => {
                tracing::warn!(event = "dictionary.install_failed", error = %error);
            }
        }
        operations.remove(&handle.operation_id);
    });
    completed(serde_json::json!({"status": "completed", "operationId": operation_id}))
}

/// Every operation the dialog or the library may still be watching.
pub(crate) async fn operations(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: TrustedRead,
) -> Response {
    if let Err(message) = request_library_document(&headers, &state) {
        return management_denied(message);
    }
    let storage = state.storage.clone();
    let read = tokio::task::spawn_blocking(move || {
        let storage = storage.locked()?;
        storage.dictionary_operations()
    })
    .await;
    let Ok(Ok(stored)) = read else {
        return HostError::Internal("dictionary-operations-read").into_response();
    };
    // A running operation's live progress beats its last persisted row.
    let operations = stored
        .into_iter()
        .map(|operation| {
            state
                .dictionary_operations
                .get(&operation.operation_id)
                .map_or_else(
                    || {
                        wire_operation(
                            &operation.operation_id,
                            &operation.corpus_language,
                            operation.audio,
                            operation.phase,
                            operation.bytes_completed,
                            operation.bytes_total,
                            operation.failure.as_deref(),
                        )
                    },
                    |handle| {
                        let progress = handle.progress();
                        wire_operation(
                            &handle.operation_id,
                            &handle.corpus_language,
                            handle.audio,
                            progress.phase,
                            progress.bytes_completed,
                            progress.bytes_total,
                            progress.failure.as_deref(),
                        )
                    },
                )
        })
        .collect::<Vec<_>>();
    completed(serde_json::json!({"status": "completed", "operations": operations}))
}

pub(crate) fn wire_operation(
    operation_id: &str,
    corpus_language: &str,
    audio: DictionaryAudioChoice,
    phase: DictionaryOperationPhase,
    bytes_completed: u64,
    bytes_total: u64,
    failure: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "operationId": operation_id,
        "corpusLanguage": corpus_language,
        "audio": audio.as_str(),
        "phase": phase.as_str(),
        "bytesCompleted": bytes_completed,
        "bytesTotal": bytes_total,
        "failure": failure,
    })
}

pub(crate) async fn operation(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    _: TrustedRead,
) -> Response {
    if let Err(message) = request_library_document(&headers, &state) {
        return management_denied(message);
    }
    if let Some(handle) = state.dictionary_operations.get(&operation_id) {
        let progress = handle.progress();
        return completed(serde_json::json!({
            "status": "completed",
            "operationId": operation_id,
            "corpusLanguage": handle.corpus_language,
            "audio": handle.audio.as_str(),
            "phase": progress.phase.as_str(),
            "bytesCompleted": progress.bytes_completed,
            "bytesTotal": progress.bytes_total,
            "failure": progress.failure,
        }));
    }
    let storage = state.storage.clone();
    let read = tokio::task::spawn_blocking(move || {
        let storage = storage.locked()?;
        storage.dictionary_operations()
    })
    .await;
    match read {
        Ok(Ok(operations)) => operations
            .into_iter()
            .find(|operation| operation.operation_id == operation_id)
            .map_or_else(
                || dictionary_unavailable(StatusCode::NOT_FOUND, "operation-unavailable"),
                |operation| {
                    completed(serde_json::json!({
                        "status": "completed",
                        "operationId": operation.operation_id,
                        "corpusLanguage": operation.corpus_language,
                        "audio": operation.audio.as_str(),
                        "phase": operation.phase.as_str(),
                        "bytesCompleted": operation.bytes_completed,
                        "bytesTotal": operation.bytes_total,
                        "failure": operation.failure,
                    }))
                },
            ),
        _ => HostError::Internal("dictionary-operation-read").into_response(),
    }
}

pub(crate) async fn cancel_operation(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    _: Trusted,
) -> Response {
    if let Err(message) = request_library_document(&headers, &state) {
        return management_denied(message);
    }
    let Some(handle) = state.dictionary_operations.get(&operation_id) else {
        return dictionary_unavailable(StatusCode::NOT_FOUND, "operation-unavailable");
    };
    handle.cancel();
    completed(serde_json::json!({"status": "completed"}))
}

pub(crate) async fn uninstall(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    Path((pack_id, pack_revision)): Path<(String, String)>,
    _: Trusted,
) -> Response {
    if let Err(message) = request_library_document(&headers, &state) {
        return management_denied(message);
    }
    if PackId::from_str(&pack_id).is_err() || !is_pack_revision(&pack_revision) {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    }
    let storage = state.storage.clone();
    let removed = tokio::task::spawn_blocking(move || {
        let mut storage = storage.locked()?;
        let marked = storage.mark_dictionary_installation_deleting(&pack_id, &pack_revision)?;
        let set = crate::open_dictionaries(&mut storage);
        Ok::<_, String>((marked, set))
    })
    .await;
    match removed {
        Ok(Ok((marked, set))) => {
            state.dictionary.store(set);
            if marked {
                completed(serde_json::json!({"status": "completed"}))
            } else {
                dictionary_unavailable(StatusCode::NOT_FOUND, "installation-unavailable")
            }
        }
        _ => HostError::Internal("dictionary-remove").into_response(),
    }
}

/// Continues an install that an earlier run left unfinished.
///
/// The partial files are still in staging, so this asks only for the bytes that are
/// missing. A failure here keeps the operation `interrupted` rather than failing it, so a
/// launch without a network simply tries again next time.
pub(crate) fn resume_interrupted_installs(state: &ReaderState) {
    let storage = state.storage.clone();
    let operations = state.dictionary_operations.clone();
    let dictionary = state.dictionary.clone();
    let admission = state.dictionary_admission.clone();
    tokio::spawn(async move {
        let stored = {
            let storage = storage.clone();
            tokio::task::spawn_blocking(move || {
                let storage = storage.lock().ok()?;
                storage.dictionary_operations().ok()
            })
            .await
        };
        let Ok(Some(stored)) = stored else { return };
        let Some(interrupted) = stored
            .into_iter()
            .find(|operation| operation.phase == DictionaryOperationPhase::Interrupted)
        else {
            return;
        };
        let Ok(permit) = admission.try_acquire_owned() else {
            return;
        };
        let Ok(catalog_url) = super::catalog::configured_url() else {
            return;
        };
        let handle = Arc::new(super::acquire::OperationHandle::new(
            interrupted.operation_id.clone(),
            interrupted.corpus_language.clone(),
            interrupted.audio,
        ));
        operations.insert(handle.clone());
        tracing::info!(
            event = "dictionary.install_resumed",
            language = %handle.corpus_language
        );
        let outcome = super::acquire::install(&storage, &handle, &catalog_url, true).await;
        if outcome.is_ok() {
            let reopened = {
                let mut storage = storage.lock().expect("storage lock poisoned");
                crate::open_dictionaries(&mut storage)
            };
            dictionary.store(reopened);
        }
        operations.remove(&handle.operation_id);
        drop(permit);
    });
}

pub(crate) fn is_pack_revision(value: &str) -> bool {
    is_hex_id(value)
}

// Development packs.

/// Debug builds install every pack or audio collection found under `directory`, fully
/// verified, with a monolingual view and bilingual views for `targets`.
#[cfg(debug_assertions)]
pub(crate) fn install_development_packs(
    storage: &mut storage::Storage,
    directory: &FsPath,
    targets: &[String],
) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("could not read {}: {error}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    entries.sort();
    for source in entries {
        let (manifest_file, pack_type) = if source.join(PACK_MANIFEST_FILE).is_file() {
            (PACK_MANIFEST_FILE, DictionaryPackType::Corpus)
        } else if source.join(AUDIO_MANIFEST_FILE).is_file() {
            (AUDIO_MANIFEST_FILE, DictionaryPackType::Audio)
        } else {
            continue;
        };
        let manifest_bytes = fs::read(source.join(manifest_file))
            .map_err(|error| format!("could not read development manifest: {error}"))?;
        let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| format!("invalid development manifest: {error}"))?;
        let pack_id = manifest["pack_id"].as_str().unwrap_or_default().to_owned();
        let revision = manifest["pack_revision"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let manifest_sha256 = format!("{:x}", Sha256::digest(&manifest_bytes));
        let already = storage
            .dictionary_installations()?
            .into_iter()
            .any(|installation| {
                installation.pack_id == pack_id && installation.pack_revision == revision
            });
        if already {
            continue;
        }
        let installation = DictionaryInstallation {
            pack_id: pack_id.clone(),
            pack_revision: revision.clone(),
            pack_type,
            corpus_language: String::new(),
            manifest_sha256: manifest_sha256.clone(),
            installed_bytes: 0,
            directory: source.clone(),
        };
        let expected = expected_pack(&installation)?;
        let (corpus_language, assets) = match pack_type {
            DictionaryPackType::Corpus => {
                let pack = DictionaryPack::verify(&source, &expected)
                    .map_err(|error| format!("development pack {pack_id} is invalid: {error}"))?;
                let assets = pack
                    .manifest()
                    .assets
                    .iter()
                    .map(|asset| (asset.file_name.clone(), asset.size_bytes))
                    .collect::<Vec<_>>();
                (pack.corpus_language().to_owned(), assets)
            }
            DictionaryPackType::Audio => {
                let collection = AudioCollection::verify(&source, &expected)
                    .map_err(|error| format!("development audio {pack_id} is invalid: {error}"))?;
                let assets = collection
                    .manifest()
                    .assets
                    .iter()
                    .map(|asset| (asset.file_name.clone(), asset.size_bytes))
                    .collect::<Vec<_>>();
                (collection.metadata().corpus_language.clone(), assets)
            }
        };
        let target = storage::dictionary_installed_directory(
            storage.dictionaries_directory(),
            &pack_id,
            &revision,
        )?;
        fs::create_dir_all(&target)
            .map_err(|error| format!("could not create {}: {error}", target.display()))?;
        let mut installed_bytes = manifest_bytes.len() as u64;
        for (file_name, size) in std::iter::once((manifest_file.to_owned(), 0)).chain(assets) {
            let destination = target.join(&file_name);
            if fs::hard_link(source.join(&file_name), &destination).is_err() {
                fs::copy(source.join(&file_name), &destination).map_err(|error| {
                    format!("could not install development asset {file_name}: {error}")
                })?;
            }
            installed_bytes += size;
        }
        let views = match pack_type {
            DictionaryPackType::Corpus => std::iter::once(NewDictionaryView {
                view_id: &corpus_language,
                view_type: DictionaryViewType::Monolingual,
                target_language: None,
            })
            .collect::<Vec<_>>(),
            DictionaryPackType::Audio => Vec::new(),
        };
        let bilingual_ids = targets
            .iter()
            .filter(|target| **target != corpus_language)
            .map(|target| (format!("{corpus_language}-{target}"), target))
            .collect::<Vec<_>>();
        let views = views
            .into_iter()
            .chain(
                bilingual_ids
                    .iter()
                    .filter(|_| pack_type == DictionaryPackType::Corpus)
                    .map(|(view_id, target)| NewDictionaryView {
                        view_id,
                        view_type: DictionaryViewType::Bilingual,
                        target_language: Some(target),
                    }),
            )
            .collect::<Vec<_>>();
        storage.register_dictionary_installation(
            &NewDictionaryInstallation {
                pack_id: &pack_id,
                pack_revision: &revision,
                pack_type,
                corpus_language: &corpus_language,
                manifest_sha256: &manifest_sha256,
                installed_bytes,
            },
            &views,
        )?;
        tracing::info!(event = "dictionary.development_pack_installed", pack_id = %pack_id);
    }
    Ok(())
}
