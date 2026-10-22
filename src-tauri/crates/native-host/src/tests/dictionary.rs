use super::support::*;

use crate::*;
use elephant_ladder_dictionary_pack::{
    AUDIO_MANIFEST_FILE, AudioDelivery, CorpusIdentity, PACK_MANIFEST_FILE, PackRevision, Sha256Hex,
};
use elephant_ladder_dictionary_pack_builder::{
    AcquisitionState, AudioBuildOptions, AudioEditionConfig, CommonsFacts, Phase, ReferenceSet,
    build_audio_collection, build_pack,
};
use serde_json::{Value, json};
use sha2::Sha256;
use std::path::{Path, PathBuf};

use crate::dictionary::{DictionaryHost, DictionarySet};
use crate::storage::{
    DictionaryPackType, DictionaryViewType, NewDictionaryInstallation, NewDictionaryView,
};

const CORPUS: &str = concat!(
    "{\"word\":\"cubi\",\"lang_code\":\"fr\",\"lang\":\"Français\",\"pos\":\"noun\",",
    "\"sounds\":[{\"ipa\":\"\\\\ky.bi\\\\\",\"audio\":\"LL-Q150_(fra)-Antochkat-cubi.wav\",",
    "\"wav_url\":\"https://commons.wikimedia.org/wiki/Special:FilePath/cubi.wav\"},",
    "{\"audio\":\"Fr-disparu.ogg\"}],",
    "\"forms\":[{\"form\":\"cubis\",\"tags\":[\"plural\"]}],",
    "\"senses\":[{\"glosses\":[\"Unité de volume.\"],\"examples\":[{\"text\":\"Un cubi.\"}]}],",
    "\"translations\":[{\"word\":\"cubit\",\"lang_code\":\"en\"}]}\n",
    "{\"word\":\"chat\",\"lang_code\":\"fr\",\"pos\":\"noun\",",
    "\"senses\":[{\"glosses\":[\"Mammifère.\"]}]}\n",
);

fn dictionary_fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../dictionary/tests/fixtures")
}

fn digest(bytes: &[u8]) -> Sha256Hex {
    Sha256Hex::from_bytes(Sha256::digest(bytes).into())
}

fn build_corpus(output: &Path) {
    let identity = CorpusIdentity {
        pack_id: "wiktionary-fr-fr".parse().unwrap(),
        corpus_language: "fr".to_owned(),
        wiktionary_edition: "frwiktionary".to_owned(),
        wiktionary_dump_date: "2026-09-01".to_owned(),
        kaikki_extraction_date: "2026-09-15".to_owned(),
        source_url: "https://example.invalid/fr.jsonl.gz".to_owned(),
        compressed_source_sha256: digest(b"compressed"),
        uncompressed_source_sha256: digest(CORPUS.as_bytes()),
        wiktextract_revision: "test".to_owned(),
        wikitextprocessor_revision: "test".to_owned(),
        builder_revision: "test".to_owned(),
        target_shard_payload_bytes: 1_000_000,
        source_manifest: json!({"provider": "native-host-test"}).to_string(),
        license_manifest: json!({"licenses": [{"spdx": "CC-BY-SA-4.0"}]}).to_string(),
        minimum_app_version: "0.1.0".to_owned(),
    };
    build_pack(&identity, Cursor::new(CORPUS), output).unwrap();
}

fn build_audio(work: &Path, output: &Path, delivery: AudioDelivery) {
    let provenance: Value = serde_json::from_slice(
        &std::fs::read(dictionary_fixtures().join("audio/commons-fixtures.provenance.json"))
            .unwrap(),
    )
    .unwrap();
    let recording = provenance["recordings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|recording| recording["file"] == "LL-Q150_fra-Antochkat-cubi.wav")
        .unwrap();
    let references = ReferenceSet {
        schema_version: 1,
        corpus_pack_id: "wiktionary-fr-fr".parse().unwrap(),
        corpus_revision: PackRevision::from_bytes(*digest(b"corpus").as_bytes()),
        corpus_language: "fr".to_owned(),
        files: vec![
            ("Fr-disparu.ogg".to_owned(), 1),
            ("LL-Q150 (fra)-Antochkat-cubi.wav".to_owned(), 1),
        ],
        invalid: Vec::new(),
    };
    let state =
        AcquisitionState::open_or_create(work, &references, "2026-09-16T00:00:00Z").unwrap();
    let metadata = |key: &str| recording["extmetadata"][key].as_str().map(str::to_owned);
    let sha1 = recording["sha1"].as_str().unwrap().to_owned();
    state
        .resolve(
            "LL-Q150 (fra)-Antochkat-cubi.wav",
            &CommonsFacts {
                title: recording["commons_title"].as_str().unwrap().to_owned(),
                sha1: sha1.clone(),
                timestamp: recording["timestamp"].as_str().unwrap().to_owned(),
                media_type: recording["mime"].as_str().unwrap().to_owned(),
                size_bytes: recording["size_bytes"].as_u64().unwrap(),
                description_url: recording["description_url"].as_str().unwrap().to_owned(),
                original_url: recording["url"].as_str().unwrap().to_owned(),
                license_short_name: metadata("LicenseShortName"),
                license_identifier: metadata("License"),
                license_url: metadata("LicenseUrl"),
                artist_html: metadata("Artist"),
                attribution_required: Some(false),
                restrictions: None,
            },
        )
        .unwrap();
    let original = state.original_path(&sha1);
    std::fs::create_dir_all(original.parent().unwrap()).unwrap();
    std::fs::copy(
        dictionary_fixtures().join("audio/LL-Q150_fra-Antochkat-cubi.wav"),
        &original,
    )
    .unwrap();
    state
        .set_phase("LL-Q150 (fra)-Antochkat-cubi.wav", Phase::Downloaded, None)
        .unwrap();
    state
        .set_phase("Fr-disparu.ogg", Phase::Missing, Some("gone"))
        .unwrap();
    assert!(state.complete_if_settled("2026-09-16T01:00:00Z").unwrap());
    build_audio_collection(
        &state,
        &AudioEditionConfig {
            pack_id: "wiktionary-fr-audio".parse().unwrap(),
            chunk_count: 1,
        },
        &AudioBuildOptions {
            builder_revision: "test".to_owned(),
            minimum_app_version: "0.1.0".to_owned(),
            workers: 1,
            delivery,
        },
        output,
    )
    .unwrap();
}

fn install(
    storage: &mut crate::storage::Storage,
    source: &Path,
    manifest_file: &str,
    pack_type: DictionaryPackType,
    views: &[NewDictionaryView<'_>],
) {
    let manifest_bytes = std::fs::read(source.join(manifest_file)).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    let pack_id = manifest["pack_id"].as_str().unwrap();
    let revision = manifest["pack_revision"].as_str().unwrap();
    let target = crate::storage::dictionary_installed_directory(
        storage.dictionaries_directory(),
        pack_id,
        revision,
    )
    .unwrap();
    std::fs::create_dir_all(&target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
    }
    storage
        .register_dictionary_installation(
            &NewDictionaryInstallation {
                pack_id,
                pack_revision: revision,
                pack_type,
                corpus_language: "fr",
                manifest_sha256: &format!("{:x}", Sha256::digest(&manifest_bytes)),
                installed_bytes: 1,
            },
            views,
        )
        .unwrap();
}

fn dictionary_state(directory: &Path, delivery: AudioDelivery) -> ReaderState {
    let mut state = fixture_reader_state(directory);
    let work = tempfile::tempdir().unwrap();
    build_corpus(&work.path().join("corpus"));
    build_audio(
        &work.path().join("acquisition"),
        &work.path().join("audio"),
        delivery,
    );
    let set = {
        let mut storage = state.storage.lock().unwrap();
        install(
            &mut storage,
            &work.path().join("corpus"),
            PACK_MANIFEST_FILE,
            DictionaryPackType::Corpus,
            &[
                NewDictionaryView {
                    view_id: "fr",
                    view_type: DictionaryViewType::Monolingual,
                    target_language: None,
                },
                NewDictionaryView {
                    view_id: "fr-en",
                    view_type: DictionaryViewType::Bilingual,
                    target_language: Some("en"),
                },
            ],
        );
        install(
            &mut storage,
            &work.path().join("audio"),
            AUDIO_MANIFEST_FILE,
            DictionaryPackType::Audio,
            &[],
        );
        DictionarySet::open(
            &storage.dictionary_installations().unwrap(),
            &storage.dictionary_views().unwrap(),
        )
    };
    assert!(set.unavailable().is_empty(), "{:?}", set.unavailable());
    state.dictionary = Arc::new(DictionaryHost::new(set));
    state
}

fn dictionary_request(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn json_response(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn opened_session(directory: &Path) -> (Router, String, String) {
    opened_session_with(directory, AudioDelivery::Bundled).await
}

async fn opened_session_with(
    directory: &Path,
    delivery: AudioDelivery,
) -> (Router, String, String) {
    let state = dictionary_state(directory, delivery);
    let capability = serde_json::to_value(state.dictionary.lease().capability().unwrap()).unwrap();
    assert_eq!(
        capability["views"],
        json!([
            {"viewId": "fr", "viewType": "monolingual", "sourceName": "Wiktionary", "editionLanguage": "fr", "sourceLanguage": "fr", "targetLanguage": null},
            {"viewId": "fr-en", "viewType": "bilingual", "sourceName": "Wiktionary", "editionLanguage": "fr", "sourceLanguage": "fr", "targetLanguage": "en"},
        ])
    );
    assert_eq!(capability["audioLanguages"], json!(["fr"]));
    let revision = capability["revision"].as_str().unwrap().to_owned();
    let active = state.lifecycle.active_view();
    let router = reader_router(state, None);
    let imported = router
        .clone()
        .oneshot(import_request_with(ALICE_BYTES.to_vec()))
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::CREATED);
    let imported: Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();
    let opened = router
        .clone()
        .oneshot(open_request(publication_id))
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
    let session = active.load().unwrap().reader_session_id.to_string();
    (router, session, revision)
}

fn lookup_body(session: &str, revision: &str, text: &str) -> Value {
    json!({
        "readerSessionId": session,
        "revision": revision,
        "requestId": "lookup-1",
        "text": text,
        "language": "fr-FR",
    })
}

#[tokio::test(flavor = "current_thread")]
async fn lookup_returns_grouped_views_audio_credits_and_punctuation_retries() {
    let directory = tempfile::tempdir().unwrap();
    let (router, session, revision) = opened_session(directory.path()).await;

    let (status, cubi) = json_response(
        &router,
        dictionary_request(
            "/native/dictionary/lookup",
            lookup_body(&session, &revision, "  «cubi», "),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cubi}");
    assert_eq!(cubi["status"], "completed");
    assert_eq!(cubi["query"], "cubi");
    assert_eq!(cubi["views"][0]["viewId"], "fr");
    assert_eq!(cubi["views"][1]["viewId"], "fr-en");
    let entry = &cubi["views"][0]["outcome"]["entries"][0]["entry"];
    assert_eq!(cubi["views"][0]["outcome"]["type"], "completed");
    assert_eq!(entry["headword"], "cubi");
    assert_eq!(entry["forms"]["total"], 1);
    assert!(entry["forms"]["items"].as_array().unwrap().is_empty());
    let audio = &entry["pronunciations"]["items"][0]["audio"];
    assert_eq!(audio["status"], "available");
    assert_eq!(
        audio["credit"]["author"],
        "Speaker: Antochkat\nRecorder: Antochkat"
    );
    assert_eq!(audio["credit"]["license"]["shortName"], "CC0");
    assert_eq!(
        audio["credit"]["modification"],
        "Transcoded to Ogg Opus 24 kbps mono"
    );
    assert_eq!(
        entry["pronunciations"]["items"][1]["audio"]["status"],
        "missing"
    );
    assert!(!cubi.to_string().contains("wavUrl"));
    assert!(
        !cubi
            .to_string()
            .contains("commons.wikimedia.org/wiki/Special")
    );
    assert_eq!(
        cubi["views"][1]["outcome"]["entries"][0]["entry"]["translations"]["items"][0]["word"],
        "cubit"
    );

    let (_, chat) = json_response(
        &router,
        dictionary_request(
            "/native/dictionary/lookup",
            lookup_body(&session, &revision, "chat"),
        ),
    )
    .await;
    assert_eq!(chat["views"][0]["outcome"]["type"], "completed");
    assert_eq!(chat["views"][1]["outcome"]["type"], "no-translation");

    let (_, absent) = json_response(
        &router,
        dictionary_request(
            "/native/dictionary/lookup",
            lookup_body(&session, &revision, "«absent»"),
        ),
    )
    .await;
    assert_eq!(absent["query"], "absent");
    assert_eq!(absent["views"][0]["outcome"]["type"], "no-entry");

    let handle = entry["handle"].as_str().unwrap();
    let (status, detail) = json_response(
        &router,
        dictionary_request(
            "/native/dictionary/entry",
            json!({"readerSessionId": session, "revision": revision, "entry": handle}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["entry"]["forms"]["items"][0]["form"], "cubis");
    assert_eq!(
        detail["entry"]["senses"]["items"][0]["examples"]["items"][0]["text"],
        "Un cubi."
    );

    let (_, forms) = json_response(
        &router,
        dictionary_request(
            "/native/dictionary/section",
            json!({"readerSessionId": session, "revision": revision, "entry": handle,
                       "section": {"type": "forms"}, "offset": 0, "limit": 1}),
        ),
    )
    .await;
    assert_eq!(forms["section"]["type"], "forms");
    assert_eq!(forms["section"]["page"]["items"][0]["form"], "cubis");
    let (_, no_sense) = json_response(
        &router,
        dictionary_request(
            "/native/dictionary/section",
            json!({"readerSessionId": session, "revision": revision, "entry": handle,
                       "section": {"type": "sense-examples", "sense": 9}, "offset": 0, "limit": 4}),
        ),
    )
    .await;
    assert_eq!(no_sense["section"]["type"], "no-such-sense");

    let played = router
        .clone()
        .oneshot(dictionary_request(
            "/native/dictionary/audio",
            json!({"readerSessionId": session, "revision": revision,
                       "audio": audio["handle"]}),
        ))
        .await
        .unwrap();
    assert_eq!(played.status(), StatusCode::OK);
    assert_eq!(played.headers()[CONTENT_TYPE], "audio/ogg; codecs=opus");
    assert_eq!(played.headers()[CACHE_CONTROL], "no-store");
    let bytes = played.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.starts_with(b"OggS"));
}

/// Points remote pronunciations at a loopback Commons for the duration of one test.
struct TestUploadBase;

impl TestUploadBase {
    fn set(base: String) -> Self {
        *crate::dictionary::remote_audio::TEST_UPLOAD_BASE
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(base);
        Self
    }
}

impl Drop for TestUploadBase {
    fn drop(&mut self) {
        *crate::dictionary::remote_audio::TEST_UPLOAD_BASE
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn remote_pronunciations_are_fetched_verified_and_played() {
    let directory = tempfile::tempdir().unwrap();
    let (router, session, revision) =
        opened_session_with(directory.path(), AudioDelivery::Remote).await;

    let original =
        std::fs::read(dictionary_fixtures().join("audio/LL-Q150_fra-Antochkat-cubi.wav")).unwrap();
    let served = Arc::new(Mutex::new(original.clone()));
    let requested = Arc::new(Mutex::new(Vec::<String>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let commons = Router::new().route(
        "/{*path}",
        get({
            let served = served.clone();
            let requested = requested.clone();
            move |uri: axum::http::Uri| {
                requested.lock().unwrap().push(uri.path().to_owned());
                let bytes = served.lock().unwrap().clone();
                async move { bytes }
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, commons).await.unwrap();
    });
    let _commons = TestUploadBase::set(format!("http://127.0.0.1:{port}"));

    let (status, cubi) = json_response(
        &router,
        dictionary_request(
            "/native/dictionary/lookup",
            lookup_body(&session, &revision, "cubi"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cubi}");
    let pronunciations = &cubi["views"][0]["outcome"]["entries"][0]["entry"]["pronunciations"];
    let audio = &pronunciations["items"][0]["audio"];
    assert_eq!(audio["status"], "remote", "{audio}");
    assert!(audio.get("durationMs").is_none());
    assert_eq!(audio["credit"]["license"]["shortName"], "CC0");
    assert_eq!(audio["credit"]["modification"], Value::Null);
    assert_eq!(pronunciations["items"][1]["audio"]["status"], "missing");

    let play = || {
        router.clone().oneshot(dictionary_request(
            "/native/dictionary/audio",
            json!({"readerSessionId": session, "revision": revision,
                       "audio": audio["handle"]}),
        ))
    };
    let played = play().await.unwrap();
    assert_eq!(played.status(), StatusCode::OK);
    assert_eq!(played.headers()[CONTENT_TYPE], "audio/wav");
    assert_eq!(played.headers()[CACHE_CONTROL], "no-store");
    let bytes = played.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), original.as_slice());
    let path = requested.lock().unwrap()[0].clone();
    assert!(
        path.ends_with("/LL-Q150_%28fra%29-Antochkat-cubi.wav"),
        "{path}"
    );

    // Bytes that are not the recording the index names are never played.
    *served.lock().unwrap() = b"RIFF not the recording".to_vec();
    let (status, rejected) = {
        let response = play().await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice::<Value>(&body).unwrap())
    };
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(rejected["error"], "pronunciation-unverified");
}

#[tokio::test(flavor = "current_thread")]
async fn dictionary_routes_reject_stale_sessions_revisions_and_invalid_requests() {
    let directory = tempfile::tempdir().unwrap();
    let (router, session, revision) = opened_session(directory.path()).await;
    let stale_revision = "0".repeat(64);
    let other_session = "a".repeat(64);
    for (body, expected) in [
        (
            lookup_body(&session, &stale_revision, "cubi"),
            StatusCode::CONFLICT,
        ),
        (
            lookup_body(&other_session, &revision, "cubi"),
            StatusCode::CONFLICT,
        ),
        (
            lookup_body("short", &revision, "cubi"),
            StatusCode::BAD_REQUEST,
        ),
        (
            lookup_body(&session, &revision, "   "),
            StatusCode::BAD_REQUEST,
        ),
        (
            lookup_body(&session, &revision, &"x".repeat(4097)),
            StatusCode::OK,
        ),
        (
            json!({"readerSessionId": session, "revision": revision, "requestId": "r",
                       "text": "cubi", "language": "not a tag"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"readerSessionId": session, "revision": revision, "requestId": "r",
                       "text": "cubi", "language": null, "extra": true}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, _) = json_response(
            &router,
            dictionary_request("/native/dictionary/lookup", body),
        )
        .await;
        assert_eq!(status, expected);
    }

    for (path, body) in [
        (
            "/native/dictionary/entry",
            json!({"readerSessionId": session, "revision": revision, "entry": "e9.00.0"}),
        ),
        (
            "/native/dictionary/audio",
            json!({"readerSessionId": session, "revision": revision,
                       "audio": "a.fr.VW5rbm93bi5vZ2c"}),
        ),
        (
            "/native/dictionary/audio",
            json!({"readerSessionId": session, "revision": revision,
                       "audio": "a.de.VW5rbm93bi5vZ2c"}),
        ),
    ] {
        let (status, value) = json_response(&router, dictionary_request(path, body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} {value}");
    }

    let (status, _) = json_response(
        &router,
        dictionary_request(
            "/native/dictionary/section",
            json!({"readerSessionId": session, "revision": revision, "entry": "e0.00.0",
                       "section": {"type": "forms"}, "offset": 0, "limit": 257}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/native/dictionary/lookup")
                .header("host", "reader.test")
                .header(ORIGIN, TEST_READER_ORIGIN)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    lookup_body(&session, &revision, "cubi").to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
}

// `install_development_packs` exists only in debug builds.
#[cfg(debug_assertions)]
#[test]
fn development_packs_are_verified_installed_once_and_opened() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let packs = tempfile::tempdir().unwrap();
    build_corpus(&packs.path().join("corpus"));
    build_audio(
        &tempfile::tempdir().unwrap().path().join("acquisition"),
        &packs.path().join("audio"),
        AudioDelivery::Bundled,
    );
    std::fs::create_dir(packs.path().join("unrelated")).unwrap();
    let mut storage = state.storage.lock().unwrap();
    for _ in 0..2 {
        crate::dictionary::install_development_packs(
            &mut storage,
            packs.path(),
            &["en".to_owned(), "fr".to_owned()],
        )
        .unwrap();
    }
    let installations = storage.dictionary_installations().unwrap();
    assert_eq!(
        installations
            .iter()
            .map(|installation| {
                (
                    installation.pack_id.as_str(),
                    installation.corpus_language.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        [("wiktionary-fr-audio", "fr"), ("wiktionary-fr-fr", "fr")]
    );
    let set = DictionarySet::open(&installations, &storage.dictionary_views().unwrap());
    assert!(set.unavailable().is_empty());
    let capability = serde_json::to_value(set.capability().unwrap()).unwrap();
    assert_eq!(capability["views"][0]["viewId"], "fr");
    assert_eq!(capability["views"][1]["viewId"], "fr-en");
    assert_eq!(capability["views"].as_array().unwrap().len(), 2);
    assert_eq!(capability["audioLanguages"], json!(["fr"]));
}

#[test]
fn empty_registries_provide_no_capability() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    assert!(state.dictionary.lease().capability().is_none());
}

/// Serializes the tests that point the host at a loopback release.
static CATALOG_SOURCE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Release {
    directory: tempfile::TempDir,
    pack_id: String,
    pack_revision: String,
    audio_pack_id: String,
}

/// Builds a pack and assembles a release served from a loopback address.
///
/// `assemble_catalog` requires an `https` base URL, so the assembled catalog is
/// rewritten to the loopback address the host actually reads.
async fn published_release(port: u16) -> Release {
    let directory = tempfile::tempdir().unwrap();
    let pack = directory.path().join("pack");
    build_corpus(&pack);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(pack.join(PACK_MANIFEST_FILE)).unwrap()).unwrap();
    let audio = directory.path().join("audio");
    build_audio(
        &directory.path().join("acquisition"),
        &audio,
        AudioDelivery::Remote,
    );
    let audio_manifest: Value =
        serde_json::from_slice(&std::fs::read(audio.join(AUDIO_MANIFEST_FILE)).unwrap()).unwrap();
    let config = elephant_ladder_dictionary_pack_builder::CatalogConfig {
        catalog_revision: "2026-09-18.1".to_owned(),
        generated_at: "2026-09-18T00:00:00Z".to_owned(),
        release_base_url: format!("https://127.0.0.1:{port}/assets"),
        corpora: vec![elephant_ladder_dictionary_pack_builder::CorpusConfig {
            directory: PathBuf::from("pack"),
            monolingual: true,
            bilingual_targets: vec!["en".to_owned()],
        }],
        audio: vec![PathBuf::from("audio")],
    };
    let release = directory.path().join("release");
    elephant_ladder_dictionary_pack_builder::assemble_catalog(&config, directory.path(), &release)
        .unwrap();
    let catalog_path = release.join("catalog-v1.json");
    let catalog = std::fs::read_to_string(&catalog_path)
        .unwrap()
        .replace("https://127.0.0.1:", "http://127.0.0.1:");
    std::fs::write(&catalog_path, catalog).unwrap();
    Release {
        pack_id: manifest["pack_id"].as_str().unwrap().to_owned(),
        pack_revision: manifest["pack_revision"].as_str().unwrap().to_owned(),
        audio_pack_id: audio_manifest["pack_id"].as_str().unwrap().to_owned(),
        directory,
    }
}

/// Serves a release directory, optionally corrupting one asset's bytes.
async fn serve_release(
    release: &Path,
    listener: tokio::net::TcpListener,
    corrupt: Option<String>,
) -> String {
    serve_release_recording(release, listener, corrupt, Arc::new(Mutex::new(Vec::new()))).await
}

/// Serves a release, recording every `Range` header the host sends.
async fn serve_release_recording(
    release: &Path,
    listener: tokio::net::TcpListener,
    corrupt: Option<String>,
    ranges: Arc<Mutex<Vec<String>>>,
) -> String {
    let release = release.to_path_buf();
    let port = listener.local_addr().unwrap().port();
    let router = Router::new()
        .route(
            "/catalog-v1.json",
            get({
                let release = release.clone();
                move || serve_file(release.join("catalog-v1.json"), None)
            }),
        )
        .route(
            "/assets/{name}",
            get({
                let release = release.clone();
                move |axum::extract::Path(name): axum::extract::Path<String>, headers: HeaderMap| {
                    let corrupt = corrupt.clone().filter(|value| *value == name);
                    let range = headers
                        .get("range")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    if let Some(range) = range.clone() {
                        ranges.lock().unwrap().push(range);
                    }
                    serve_range(release.join(name), corrupt, range)
                }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://127.0.0.1:{port}")
}

/// The loopback source, held for the duration of one test.
struct TestSource {
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl TestSource {
    fn set(guard: tokio::sync::MutexGuard<'static, ()>, url: String) -> Self {
        *crate::dictionary::catalog::TEST_SOURCE
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(url);
        Self { _guard: guard }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        *crate::dictionary::catalog::TEST_SOURCE
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

async fn serialized() -> tokio::sync::MutexGuard<'static, ()> {
    CATALOG_SOURCE.lock().await
}

/// Answers a byte-range request the way a release host does.
async fn serve_range(
    path: PathBuf,
    corrupt: Option<String>,
    range: Option<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let Some(offset) = range.and_then(|value| {
        value
            .strip_prefix("bytes=")
            .and_then(|rest| rest.strip_suffix('-'))
            .and_then(|start| start.parse::<usize>().ok())
    }) else {
        return serve_file(path, corrupt).await;
    };
    match std::fs::read(&path) {
        Ok(bytes) if offset <= bytes.len() => (
            StatusCode::PARTIAL_CONTENT,
            [(
                axum::http::header::CONTENT_RANGE,
                format!("bytes {offset}-{}/{}", bytes.len() - 1, bytes.len()),
            )],
            bytes[offset..].to_vec(),
        )
            .into_response(),
        Ok(_) => (StatusCode::RANGE_NOT_SATISFIABLE, "bad range").into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn serve_file(path: PathBuf, corrupt: Option<String>) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    match std::fs::read(&path) {
        Ok(mut bytes) => {
            if corrupt.is_some()
                && let Some(first) = bytes.first_mut()
            {
                *first ^= 0xff;
            }
            (StatusCode::OK, bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn management_request(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(crate::LIBRARY_DOCUMENT_HEADER, "b".repeat(64));
    if body.is_some() {
        request = request.header(CONTENT_TYPE, "application/json");
    }
    request
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap()
}

async fn await_operation(router: &Router, operation_id: &str) -> Value {
    for _ in 0..600 {
        let (status, value) = json_response(
            router,
            management_request(
                "GET",
                &format!("/native/dictionary/operations/{operation_id}"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let phase = value["phase"].as_str().unwrap_or_default().to_owned();
        if matches!(
            phase.as_str(),
            "completed" | "cancelled" | "failed" | "interrupted"
        ) {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the dictionary operation did not finish");
}

// Redirects the catalog through `TEST_SOURCE`, which a release build ignores.
#[tokio::test]
async fn installs_a_language_as_one_download_and_serves_it_to_readers() {
    let guard = serialized().await;
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let release = published_release(port).await;
    let served = serve_release(&release.directory.path().join("release"), listener, None).await;
    let _source = TestSource::set(guard, format!("{served}/catalog-v1.json"));

    let state = fixture_reader_state(directory.path());
    let dictionary = state.dictionary.clone();
    let router = reader_router(state, None);

    let (status, catalog) = json_response(
        &router,
        management_request("GET", "/native/dictionary/catalog", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{catalog}");
    assert_eq!(catalog["packs"][0]["packType"], "corpus");
    assert_eq!(catalog["packs"][0]["packId"], release.pack_id.as_str());
    assert_eq!(catalog["packs"][0]["installed"], json!(false));
    assert_eq!(catalog["packs"][0]["corpusLanguage"], "fr");
    assert_eq!(catalog["packs"][1]["packType"], "audio");
    assert_eq!(
        catalog["packs"][1]["packId"],
        release.audio_pack_id.as_str()
    );
    assert_eq!(catalog["packs"][1]["delivery"], "remote");
    assert_eq!(catalog["packs"][1]["recordings"], 1);

    // One install covers the language: its corpus and the pronunciations it comes with.
    let (status, started) = json_response(
        &router,
        management_request(
            "POST",
            "/native/dictionary/installations",
            Some(json!({"corpusLanguage": "fr", "audio": "streamed"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let operation_id = started["operationId"].as_str().unwrap().to_owned();
    let finished = await_operation(&router, &operation_id).await;
    assert_eq!(finished["phase"], "completed", "{finished}");
    assert_eq!(finished["corpusLanguage"], "fr");
    assert_eq!(finished["audio"], "streamed");
    // One total for the language, so it reads as the single download it is.
    let language_bytes = catalog["packs"][0]["installedBytes"].as_u64().unwrap()
        + catalog["packs"][1]["installedBytes"].as_u64().unwrap();
    assert_eq!(finished["bytesTotal"], language_bytes, "{finished}");
    assert_eq!(finished["bytesCompleted"], finished["bytesTotal"]);

    let (status, installations) = json_response(
        &router,
        management_request("GET", "/native/dictionary/installations", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{installations}");
    assert_eq!(installations["installations"][0]["corpusLanguage"], "fr");

    // Pronunciations arrive with the corpus and add no dictionary views of their own.
    let capability = serde_json::to_value(dictionary.lease().capability().unwrap()).unwrap();
    assert_eq!(capability["views"].as_array().unwrap().len(), 2);
    assert_eq!(capability["audioLanguages"], json!(["fr"]));

    // Asking again has nothing left to download, and a delivery this catalog does not
    // publish keeps the collection the language already has.
    let (status, again) = json_response(
        &router,
        management_request(
            "POST",
            "/native/dictionary/installations",
            Some(json!({"corpusLanguage": "fr", "audio": "stored"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let finished = await_operation(&router, again["operationId"].as_str().unwrap()).await;
    assert_eq!(finished["phase"], "completed", "{finished}");
    assert_eq!(finished["bytesTotal"], 0, "{finished}");
    let capability = serde_json::to_value(dictionary.lease().capability().unwrap()).unwrap();
    assert_eq!(capability["audioLanguages"], json!(["fr"]));
    let (_, installations) = json_response(
        &router,
        management_request("GET", "/native/dictionary/installations", None),
    )
    .await;
    let types: Vec<_> = installations["installations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|installation| installation["packType"].as_str().unwrap())
        .collect();
    assert!(
        types.contains(&"corpus") && types.contains(&"audio"),
        "{installations}"
    );
    // A panel with no catalog reads this listing, where only an opened collection can say
    // whether its recordings play from the device.
    let deliveries: Vec<_> = installations["installations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|installation| {
            (
                installation["packType"].as_str().unwrap(),
                installation["delivery"].clone(),
            )
        })
        .collect();
    assert!(
        deliveries.contains(&("audio", json!("remote"))),
        "{installations}"
    );
    assert!(
        deliveries.contains(&("corpus", Value::Null)),
        "{installations}"
    );

    // The library polls this listing, so a finished install must be visible there
    // without knowing the operation ID.
    let (status, listed) = json_response(
        &router,
        management_request("GET", "/native/dictionary/operations", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let listed_corpus = listed["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|operation| operation["operationId"] == operation_id.as_str())
        .unwrap();
    assert_eq!(listed_corpus["phase"], "completed");
    assert_eq!(listed_corpus["corpusLanguage"], "fr");

    let (status, removed) = json_response(
        &router,
        management_request(
            "DELETE",
            &format!(
                "/native/dictionary/installations/{}/{}",
                release.pack_id, release.pack_revision
            ),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert!(dictionary.lease().capability().is_none());
}

// Redirects the catalog through `TEST_SOURCE`, which a release build ignores.
#[tokio::test]
async fn a_corrupted_asset_fails_the_install_and_registers_nothing() {
    let guard = serialized().await;
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let release = published_release(port).await;
    let corrupted = std::fs::read_dir(release.directory.path().join("release"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| {
            name.starts_with(&format!("{}-", release.pack_id)) && name.ends_with("-index.eldict")
        })
        .unwrap();
    let served = serve_release(
        &release.directory.path().join("release"),
        listener,
        Some(corrupted),
    )
    .await;
    let _source = TestSource::set(guard, format!("{served}/catalog-v1.json"));
    let state = fixture_reader_state(directory.path());
    let dictionary = state.dictionary.clone();
    let storage = state.storage.clone();
    let router = reader_router(state, None);
    let (status, started) = json_response(
        &router,
        management_request(
            "POST",
            "/native/dictionary/installations",
            Some(json!({"corpusLanguage": "fr", "audio": "streamed"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let finished = await_operation(&router, started["operationId"].as_str().unwrap()).await;
    assert_eq!(finished["phase"], "failed", "{finished}");
    assert!(
        finished["failure"]
            .as_str()
            .unwrap()
            .contains("does not match the catalog"),
        "{finished}"
    );
    assert!(dictionary.lease().capability().is_none());
    let storage = storage.lock().unwrap();
    assert!(storage.dictionary_installations().unwrap().is_empty());
    // A failed install leaves nothing behind for the next one to trip over.
    let staging = storage.dictionaries_directory().join("staging");
    assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
}

// Redirects the catalog through `TEST_SOURCE`, which a release build ignores.
#[tokio::test]
async fn an_interrupted_install_resumes_from_its_partial_files() {
    let guard = serialized().await;
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let release = published_release(port).await;
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let served = serve_release_recording(
        &release.directory.path().join("release"),
        listener,
        None,
        ranges.clone(),
    )
    .await;
    let _source = TestSource::set(guard, format!("{served}/catalog-v1.json"));
    let state = fixture_reader_state(directory.path());
    let storage = state.storage.clone();

    // Stand in for a process that died mid-download: a recorded operation, its staging
    // directory, and one asset's first bytes already written.
    let operation_id = "c".repeat(32);
    let (staging, manifest_name, manifest_bytes) = {
        let mut storage = storage.lock().unwrap();
        storage
            .begin_dictionary_operation(
                &operation_id,
                "fr",
                crate::storage::DictionaryAudioChoice::Streamed,
                None,
            )
            .unwrap();
        storage
            .update_dictionary_operation(
                &operation_id,
                crate::storage::DictionaryOperationPhase::Interrupted,
                0,
                0,
                None,
            )
            .unwrap();
        let staging = crate::storage::dictionary_staging_directory(
            storage.dictionaries_directory(),
            &operation_id,
        )
        .unwrap()
        .join(format!("{}-{}", release.pack_id, release.pack_revision));
        let name = format!(
            "{}-{}-pack-manifest-v1.json",
            release.pack_id, release.pack_revision
        );
        let bytes = std::fs::read(release.directory.path().join("release").join(&name)).unwrap();
        (staging, PACK_MANIFEST_FILE.to_owned(), bytes)
    };
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(
        staging.join(format!("{manifest_name}.part")),
        &manifest_bytes[..manifest_bytes.len() / 2],
    )
    .unwrap();

    let router = reader_router(state, None);
    let (status, started) = json_response(
        &router,
        management_request(
            "POST",
            "/native/dictionary/installations",
            Some(json!({"corpusLanguage": "fr", "audio": "streamed"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(
        started["operationId"].as_str().unwrap(),
        operation_id,
        "the interrupted operation is resumed rather than restarted"
    );
    let finished = await_operation(&router, &operation_id).await;
    assert_eq!(finished["phase"], "completed", "{finished}");
    let ranges = ranges.lock().unwrap().clone();
    assert_eq!(
        ranges,
        vec![format!("bytes={}-", manifest_bytes.len() / 2)],
        "only the missing bytes are requested"
    );
    // The resume finishes the language, not whichever pack was in flight when it stopped.
    let installed = storage.lock().unwrap().dictionary_installations().unwrap();
    let mut packs = installed
        .iter()
        .map(|installation| installation.pack_id.as_str())
        .collect::<Vec<_>>();
    packs.sort_unstable();
    assert_eq!(
        packs,
        vec![release.audio_pack_id.as_str(), release.pack_id.as_str()],
        "{installed:?}"
    );
}

/// Fetches the real release over TLS. Ignored by default so the suite stays offline.
#[tokio::test]
#[ignore = "requires network access to the release host"]
async fn reaches_the_published_release_over_tls() {
    let client = crate::dictionary::catalog::client().unwrap();
    let catalog_url = crate::dictionary::catalog::configured_url().unwrap();
    let catalog = crate::dictionary::catalog::load_catalog(&client, &catalog_url)
        .await
        .unwrap();
    assert!(
        catalog
            .packs
            .iter()
            .any(|pack| pack.release().pack_id.as_str().starts_with("wiktionary"))
    );
}

#[tokio::test]
async fn management_routes_require_the_current_library_document() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let router = reader_router(state, None);
    for (method, path, body) in [
        ("GET", "/native/dictionary/catalog", None),
        ("GET", "/native/dictionary/installations", None),
        (
            "POST",
            "/native/dictionary/installations",
            Some(json!({"corpusLanguage": "fr", "audio": "streamed"})),
        ),
    ] {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "reader.test")
            .header(ORIGIN, TEST_READER_ORIGIN)
            .header(AUTHORIZATION, "Bearer secret");
        if body.is_some() {
            request = request.header(CONTENT_TYPE, "application/json");
        }
        let request = request
            .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
            .unwrap();
        let (status, value) = json_response(&router, request).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method} {path} {value}");
    }

    let (status, value) = json_response(
        &router,
        management_request(
            "POST",
            "/native/dictionary/installations",
            Some(json!({"corpusLanguage": "fr", "audio": "elsewhere"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");

    let (status, value) = json_response(
        &router,
        management_request(
            "POST",
            "/native/dictionary/installations",
            Some(json!({
                "corpusLanguage": "fr",
                "audio": "streamed",
                "url": "https://example.invalid/pack"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");
}

/// Admission outlives the operation that holds it, because the finished pack is reopened
/// after its operation already reports a terminal phase.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_install_waits_out_the_previous_one_rather_than_refusing_the_next_pack() {
    let guard = serialized().await;
    let directory = tempfile::tempdir().unwrap();
    // A refused loopback source keeps the installs this test starts offline.
    let _source = TestSource::set(guard, "http://127.0.0.1:1/catalog-v1.json".to_owned());
    let state = fixture_reader_state(directory.path());
    let admission = state.dictionary_admission.clone();
    let router = reader_router(state, None);
    let request = || {
        management_request(
            "POST",
            "/native/dictionary/installations",
            Some(json!({"corpusLanguage": "fr", "audio": "streamed"})),
        )
    };

    // An operation that is still running is refused rather than queued.
    let permit = admission.clone().acquire_owned().await.unwrap();
    let (status, refused) = json_response(&router, request()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(
        refused["message"],
        "a dictionary operation is already active"
    );

    // One that has finished and is only letting go is waited out, which is what the next
    // pack of a language runs into.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        drop(permit);
    });
    let (status, started) = json_response(&router, request()).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert!(
        started["operationId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{started}"
    );
}
