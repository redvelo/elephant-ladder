use super::support::*;

#[test]
fn search_text_fragments_include_bounded_immediate_context() {
    assert_eq!(
        search_text_fragment(&(
            "...first ".to_owned(),
            "alpha".to_owned(),
            " second...".to_owned()
        )),
        "text=first%20-,alpha,-%20second"
    );
    assert_eq!(
        search_text_fragment(&(String::new(), "alpha".to_owned(), String::new())),
        "text=alpha"
    );
    assert_eq!(
        search_text_fragment(&(
            "012345678901234567890123456789012345".to_owned(),
            "alpha".to_owned(),
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJK".to_owned()
        )),
        "text=45678901234567890123456789012345-,alpha,-abcdefghijklmnopqrstuvwxyzABCDEF"
    );
}

#[test]
fn alice_production_analysis_resolves_default_ask_and_preserves_search() {
    let directory = tempfile::tempdir().unwrap();
    let mut storage = crate::storage::Storage::open(directory.path()).unwrap();
    let search_index = crate::search_index::SearchIndexStore::open(directory.path()).unwrap();
    let epub = EpubZip::from_reader(Cursor::new(ALICE_BYTES.to_vec()))
        .unwrap()
        .default_rendition()
        .unwrap();
    let publication_facts = epub_navigator::facts::PublicationFacts::of(&epub).unwrap();
    let reading_order = epub_navigator::kernel::project_reading_order(&publication_facts).unwrap();
    let facts = serde_json::to_value(&publication_facts).unwrap();
    // Use the same analysis configuration as production import, without a second full scan.
    let analysis = epub.analyze_with_limits(crate::search::search_analysis_limits());
    let relevance = crate::policy::publication_policy_relevance(&analysis);
    assert_eq!(
        relevance.active_content,
        crate::storage::PolicyRelevance::NotDetected
    );
    assert_eq!(
        relevance.remote_resources,
        crate::storage::PolicyRelevance::NotDetected
    );
    let policy = crate::storage::PublicationPolicy {
        global: crate::storage::GlobalPublicationPolicy::default(),
        publication: crate::storage::PublicationPolicyOverrides::default(),
    };
    let stored = crate::storage::StoredPublicationPolicy {
        effective: policy.effective(),
        policy,
        revisions: crate::storage::PublicationPolicyRevisions {
            global: 0,
            publication: 0,
        },
    };
    let resolved = crate::policy::resolve_publication_policy(&stored, relevance).unwrap();
    assert_eq!(
        resolved.active_content,
        crate::ResolvedActiveContentPolicy::Disabled
    );
    assert_eq!(
        resolved.remote_resources,
        crate::ResolvedRemoteResourcesPolicy::Blocked
    );

    let limited = epub.analyze_with_limits({
        let mut limits = crate::search::search_analysis_limits();
        limits.max_resource_analysis_bytes = Some(1);
        limits.max_total_analysis_bytes = Some(1);
        limits
    });
    let relevance = crate::policy::publication_policy_relevance(&limited);
    assert_eq!(
        relevance.active_content,
        crate::storage::PolicyRelevance::Unknown
    );
    assert_eq!(
        relevance.remote_resources,
        crate::storage::PolicyRelevance::Unknown
    );
    let required = crate::policy::resolve_publication_policy(&stored, relevance).unwrap_err();
    assert!(required.active_content);
    assert!(required.remote_resources);
    let snapshot = crate::search::derive_search_snapshot(&analysis, &reading_order).unwrap();
    let responses = serde_json::to_value(
        crate::snapshot::response_manifest(&analysis, &reading_order).unwrap(),
    )
    .unwrap();
    let alice = snapshot
        .entries
        .iter()
        .filter(|entry| entry.text.to_lowercase().contains("alice"))
        .collect::<Vec<_>>();
    let linked = alice
        .iter()
        .filter(|entry| !entry.reading_order_occurrence_indexes.is_empty())
        .count();

    assert!(!alice.is_empty());
    assert!(
        linked > 0,
        "Alice entries: {}, linked: {linked}",
        alice.len()
    );

    let blob = storage.install_blob(ALICE_BYTES).unwrap();
    let imported = storage
        .commit_import_snapshot(
            crate::storage::InstalledImport {
                archive: &blob,
                cover: None,
            },
            "reading-v1",
            "Alice",
            &facts,
            &responses,
        )
        .unwrap();
    let crate::storage::PublicationSelection::Ready(selected) = storage
        .select_publication(&imported.publication.publication_id, "reading-v1", None)
        .unwrap()
    else {
        panic!("Alice publication must be selectable");
    };
    search_index
        .write(&selected.snapshot_id, crate::SEARCH_REVISION, &snapshot)
        .unwrap();
    let crate::search_index::SearchStorageOutcome::Ready(ready) = search_index
        .search(crate::search_index::SearchStorageRequest {
            snapshot_id: &selected.snapshot_id,
            expected_revision: crate::SEARCH_REVISION,
            expression: &crate::search::compile_search_expression("alice").unwrap(),
            occurrence: None,
            resource_path: None,
            offset: 0,
            limit: 20,
        })
        .unwrap()
    else {
        panic!("Alice search index must be ready");
    };
    assert!(!ready.rows.is_empty());
}

#[test]
fn fallback_xhtml_drives_search_annotation_source_and_occurrence_identity() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata/><manifest><item id="audio" href="chapter.mp3" media-type="audio/mpeg" fallback="text"/><item id="text" href="fallback.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="audio" linear="no"/></spine></package>"#;
    let provider = MemoryResourceProvider::from_entries([
            ("EPUB/package.opf", package.to_vec()),
            ("EPUB/chapter.mp3", b"audio".to_vec()),
            (
                "EPUB/fallback.xhtml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="fallback">Fallback ladder text</p></body></html>"#.to_vec(),
            ),
        ])
        .unwrap();
    let epub = Epub::from_provider(provider, EpubPath::new("EPUB/package.opf").unwrap()).unwrap();
    let publication_facts = epub_navigator::facts::PublicationFacts::of(&epub).unwrap();
    let original_facts = serde_json::to_value(&publication_facts).unwrap();
    let reading_order = epub_navigator::kernel::project_reading_order(&publication_facts).unwrap();

    assert_eq!(reading_order[0].selected_declaration, Some(1));
    assert!(matches!(
        reading_order[0].linear,
        epub_navigator::kernel::OccurrenceLinear::No
    ));
    assert!(matches!(
        &reading_order[0].document,
        epub_navigator::kernel::OccurrenceDocumentTarget::Loadable { resource, .. }
            if resource == "EPUB/fallback.xhtml"
    ));
    assert_eq!(
        crate::reading::reading_order_local_resources(&reading_order),
        vec![Some("EPUB/fallback.xhtml".to_owned())]
    );
    assert_eq!(
        crate::search::reading_order_source(&original_facts, &reading_order, 0).as_deref(),
        Some("fallback.xhtml")
    );

    let analysis = epub.analyze_with_limits(crate::search::search_analysis_limits());
    let search = crate::search::derive_search_snapshot(&analysis, &reading_order).unwrap();
    let effective_reading_order_resources =
        crate::reading::reading_order_local_resources(&reading_order);
    let fallback = search
        .entries
        .iter()
        .find(|entry| entry.text == "Fallback ladder text")
        .unwrap();
    assert_eq!(fallback.resource_path, "EPUB/fallback.xhtml");
    assert_eq!(fallback.reading_order_occurrence_indexes, vec![0]);
    assert!(!search.entries.iter().any(|entry| {
        entry.resource_path == "EPUB/chapter.mp3"
            && !entry.reading_order_occurrence_indexes.is_empty()
    }));
    assert_eq!(
        serde_json::to_value(&publication_facts).unwrap(),
        original_facts
    );
    let original_resource = original_facts["resources"]["readingOrder"][0]["resource"]
        .as_u64()
        .unwrap() as usize;
    assert_eq!(
        original_facts["resources"]["resources"][original_resource]["address"]["value"],
        "EPUB/chapter.mp3"
    );
    assert!(
        crate::search::validate_search_product(
            &publication_facts,
            &search,
            &effective_reading_order_resources
        )
        .is_ok()
    );

    let mut mismatched = crate::search::derive_search_snapshot(&analysis, &reading_order).unwrap();
    for entry in &mut mismatched.entries {
        entry.reading_order_occurrence_indexes.clear();
    }
    mismatched.effective_reading_order_resources[0] = Some("EPUB/chapter.mp3".to_owned());
    assert!(crate::search::validate_search_snapshot(&mismatched).is_ok());
    assert!(
        crate::search::validate_search_product(
            &publication_facts,
            &mismatched,
            &effective_reading_order_resources
        )
        .is_err()
    );

    let directory = tempfile::tempdir().unwrap();
    let mut storage = crate::storage::Storage::open(directory.path()).unwrap();
    let search_index = crate::search_index::SearchIndexStore::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"fallback search archive").unwrap();
    let responses = serde_json::to_value(
        crate::snapshot::response_manifest(&analysis, &reading_order).unwrap(),
    )
    .unwrap();
    let imported = storage
        .commit_import_snapshot(
            crate::storage::InstalledImport {
                archive: &blob,
                cover: None,
            },
            "reading-fallback-test",
            "Fallback",
            &original_facts,
            &responses,
        )
        .unwrap();
    let crate::storage::PublicationSelection::Ready(selected) = storage
        .select_publication(
            &imported.publication.publication_id,
            "reading-fallback-test",
            None,
        )
        .unwrap()
    else {
        panic!("fallback publication must be selectable");
    };
    search_index
        .write(&selected.snapshot_id, crate::SEARCH_REVISION, &search)
        .unwrap();
    let crate::search_index::SearchStorageOutcome::Ready(result) = search_index
        .search(crate::search_index::SearchStorageRequest {
            snapshot_id: &selected.snapshot_id,
            expected_revision: crate::SEARCH_REVISION,
            expression: &crate::search::compile_search_expression("ladder").unwrap(),
            occurrence: Some(0),
            resource_path: Some("EPUB/fallback.xhtml"),
            offset: 0,
            limit: 10,
        })
        .unwrap()
    else {
        panic!("fallback occurrence search must be available");
    };
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].resource_path, "EPUB/fallback.xhtml");
}

#[test]
fn search_product_requires_the_exact_effective_reading_order_projection() {
    let epub = EpubZip::from_reader(Cursor::new(ALICE_BYTES.to_vec()))
        .unwrap()
        .default_rendition()
        .unwrap();
    let facts = epub_navigator::facts::PublicationFacts::of(&epub).unwrap();
    let reading_order = epub_navigator::kernel::project_reading_order(&facts).unwrap();
    let analysis = epub.analyze_with_limits(crate::search::search_analysis_limits());
    let snapshot = crate::search::derive_search_snapshot(&analysis, &reading_order).unwrap();
    let expected = snapshot.effective_reading_order_resources.clone();

    assert!(crate::search::validate_search_product(&facts, &snapshot, &expected).is_ok());
    let mut mismatched = snapshot;
    mismatched.effective_reading_order_resources.pop();
    assert!(crate::search::validate_search_product(&facts, &mismatched, &expected).is_err());
}

#[test]
fn fallback_svg_is_a_foreground_resource() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata/><manifest><item id="foreign" href="chapter.bin" media-type="application/octet-stream" fallback="svg"/><item id="svg" href="fallback.svg" media-type="image/svg+xml"/></manifest><spine><itemref idref="foreign"/></spine></package>"#;
    let provider = MemoryResourceProvider::from_entries([
        ("EPUB/package.opf", package.to_vec()),
        ("EPUB/chapter.bin", b"foreign".to_vec()),
        (
            "EPUB/fallback.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg"><text>Fallback</text></svg>"#.to_vec(),
        ),
    ])
    .unwrap();
    let epub = Epub::from_provider(provider, EpubPath::new("EPUB/package.opf").unwrap()).unwrap();
    let reading_order = epub_navigator::kernel::project_reading_order(
        &epub_navigator::facts::PublicationFacts::of(&epub).unwrap(),
    )
    .unwrap();
    let responses = crate::snapshot::response_manifest(&epub.analyze(), &reading_order).unwrap();

    assert!(matches!(
        responses.resources["EPUB/fallback.svg"].foreground,
        Foreground::Svg
    ));
    assert!(matches!(
        responses.resources["EPUB/chapter.bin"].foreground,
        Foreground::Unchanged
    ));
}

#[test]
fn fallback_cycle_and_unresolved_reference_fail_as_bounded_occurrences() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata/><manifest><item id="cycle-a" href="a.bin" media-type="application/octet-stream" fallback="cycle-b"/><item id="cycle-b" href="b.bin" media-type="application/octet-stream" fallback="cycle-a"/><item id="missing" href="missing.bin" media-type="application/octet-stream" fallback="absent"/></manifest><spine><itemref idref="cycle-a"/><itemref idref="missing"/></spine></package>"#;
    let provider = MemoryResourceProvider::from_entries([
        ("EPUB/package.opf", package.to_vec()),
        ("EPUB/a.bin", b"a".to_vec()),
        ("EPUB/b.bin", b"b".to_vec()),
        ("EPUB/missing.bin", b"missing".to_vec()),
    ])
    .unwrap();
    let epub = Epub::from_provider(provider, EpubPath::new("EPUB/package.opf").unwrap()).unwrap();
    let reading_order = epub_navigator::kernel::project_reading_order(
        &epub_navigator::facts::PublicationFacts::of(&epub).unwrap(),
    )
    .unwrap();

    assert_eq!(reading_order.len(), 2);
    assert!(matches!(
        reading_order[0].document,
        epub_navigator::kernel::OccurrenceDocumentTarget::Unavailable {
            reason: epub_navigator::kernel::OccurrenceUnavailableReason::ManifestFallbackCycle,
            ..
        }
    ));
    assert!(matches!(
        reading_order[1].document,
        epub_navigator::kernel::OccurrenceDocumentTarget::Unavailable {
            reason: epub_navigator::kernel::OccurrenceUnavailableReason::ManifestFallbackUnresolved,
            ..
        }
    ));
    assert_eq!(
        crate::reading::reading_order_local_resources(&reading_order),
        vec![None, None]
    );
}

#[test]
fn declared_resource_missing_from_the_archive_is_recorded_not_fatal() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata><meta property="dcterms:modified">2026-09-12T00:00:00Z</meta></metadata><manifest><item id="text" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="image" href="missing.png" media-type="image/png"/></manifest><spine><itemref idref="text"/></spine></package>"#;
    let bytes = synthetic_epub(
            package,
            &[(
                "EPUB/chapter.xhtml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="missing.png"/></body></html>"#,
            )],
        );
    let directory = tempfile::tempdir().unwrap();
    let storage = crate::storage::Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(&bytes).unwrap();
    let derived = crate::snapshot::derive_publication(
        &blob.path,
        directory.path(),
        u64::MAX,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    let rule = derived.responses.resources.get("EPUB/missing.png").unwrap();
    assert!(rule.missing);
    assert_eq!(rule.size_bytes, 0);
    assert!(!derived.responses.resources["EPUB/chapter.xhtml"].missing);
    let json = serde_json::to_value(&derived.responses).unwrap();
    assert_eq!(json["resources"]["EPUB/missing.png"]["missing"], true);
    assert!(
        json["resources"]["EPUB/chapter.xhtml"]
            .get("missing")
            .is_none()
    );
}

#[test]
fn search_index_is_derived_and_materialized_after_import() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata><meta property="dcterms:modified">2026-09-12T00:00:00Z</meta></metadata><manifest><item id="text" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="text"/></spine></package>"#;
    let bytes = synthetic_epub(
            package,
            &[(
                "EPUB/chapter.xhtml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>An elephant climbs the ladder.</p></body></html>"#,
            )],
        );
    let directory = tempfile::tempdir().unwrap();
    let mut storage = crate::storage::Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(&bytes).unwrap();
    let derived = crate::snapshot::derive_publication(
        &blob.path,
        directory.path(),
        u64::MAX,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    let imported = storage
        .commit_import_snapshot_with_policy_relevance(
            crate::storage::InstalledImport {
                archive: &blob,
                cover: None,
            },
            crate::PRODUCTION_READING_REVISION,
            "Elephant",
            crate::storage::PolicyAwareReadingProducts {
                facts: &derived.facts,
                package: &derived.package,
                responses: &derived.responses,
                media_overlay: &derived.media_overlay,
                media_overlay_timelines: &derived.media_overlay_timelines,
                prepared_sidecars: derived.prepared_sidecars,
                policy_relevance: derived.policy_relevance,
            },
        )
        .unwrap();
    let snapshot_id = imported.snapshot_id.clone().unwrap();
    assert_eq!(imported.blob_path, blob.path);
    assert!(storage.snapshot_is_current(&snapshot_id).unwrap());
    assert!(
        !storage
            .snapshot_is_current("0".repeat(32).as_str())
            .unwrap()
    );
    assert_eq!(
        storage.live_snapshot_ids().unwrap(),
        std::collections::HashSet::from([snapshot_id.clone()])
    );
    let search_index = crate::search_index::SearchIndexStore::open(directory.path()).unwrap();
    assert!(
        !search_index
            .exists(&snapshot_id, crate::SEARCH_REVISION)
            .unwrap()
    );
    let query = |expression: &str| {
        search_index
            .search(crate::search_index::SearchStorageRequest {
                snapshot_id: &snapshot_id,
                expected_revision: crate::SEARCH_REVISION,
                expression,
                occurrence: None,
                resource_path: None,
                offset: 0,
                limit: 10,
            })
            .unwrap()
    };
    assert!(matches!(
        query("elephant"),
        crate::search_index::SearchStorageOutcome::IndexUnavailable
    ));

    let search = crate::search::derive_search_index(&blob.path).unwrap();
    search_index
        .write(&snapshot_id, crate::SEARCH_REVISION, &search)
        .unwrap();
    assert!(
        search_index
            .exists(&snapshot_id, crate::SEARCH_REVISION)
            .unwrap()
    );
    assert!(matches!(
        query("elephant"),
        crate::search_index::SearchStorageOutcome::Ready(_)
    ));
}

#[test]
fn stored_immutable_epub_recomputes_the_effective_fallback_projection() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata><meta property="dcterms:modified">2026-09-12T00:00:00Z</meta></metadata><manifest><item id="foreign" href="chapter.bin" media-type="application/octet-stream" fallback="text"/><item id="text" href="fallback.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="foreign"/></spine></package>"#;
    let bytes = synthetic_epub(
            package,
            &[
                ("EPUB/chapter.bin", b"foreign"),
                (
                    "EPUB/fallback.xhtml",
                    br#"<html xmlns="http://www.w3.org/1999/xhtml"><body>Stored fallback</body></html>"#,
                ),
            ],
        );
    let directory = tempfile::tempdir().unwrap();
    let mut storage = crate::storage::Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(&bytes).unwrap();
    let derived = crate::snapshot::derive_publication(
        &blob.path,
        directory.path(),
        u64::MAX,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    let imported = storage
        .commit_import_snapshot_with_policy_relevance(
            crate::storage::InstalledImport {
                archive: &blob,
                cover: None,
            },
            crate::PRODUCTION_READING_REVISION,
            "Stored fallback",
            crate::storage::PolicyAwareReadingProducts {
                facts: &derived.facts,
                package: &derived.package,
                responses: &derived.responses,
                media_overlay: &derived.media_overlay,
                media_overlay_timelines: &derived.media_overlay_timelines,
                prepared_sidecars: derived.prepared_sidecars,
                policy_relevance: derived.policy_relevance,
            },
        )
        .unwrap();
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    let (facts_json, responses_json): (String, String) = connection
        .query_row(
            "SELECT s.facts_json, s.responses_json
                 FROM publications p
                 JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                 WHERE p.publication_id = ?1",
            [&imported.publication.publication_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(facts_json, serde_json::to_string(&derived.facts).unwrap());
    assert_eq!(
        responses_json,
        serde_json::to_string(&derived.responses).unwrap()
    );
    let crate::storage::PublicationSelection::Ready(selected) = storage
        .select_publication(
            &imported.publication.publication_id,
            crate::PRODUCTION_READING_REVISION,
            None,
        )
        .unwrap()
    else {
        panic!("stored fallback publication must be ready");
    };

    let loaded = crate::snapshot::load_snapshot_entry(selected).unwrap();
    assert_eq!(
        crate::reading::reading_order_local_resources(&loaded.entry.reading_order),
        vec![Some("EPUB/fallback.xhtml".to_owned())]
    );
    assert_eq!(
        loaded.entry.facts["resources"]["readingOrder"][0]["target"]["state"],
        "declaration"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn alice_search_survives_delete_and_reimport() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let search_indexing = state.search_indexing.clone();
    let active_reader_session = state.lifecycle.active_view();
    let router = reader_router(state, None);

    for cycle in 0..2 {
        let imported = router
            .clone()
            .oneshot(import_request_with(ALICE_BYTES.to_vec()))
            .await
            .unwrap();
        assert_eq!(imported.status(), StatusCode::CREATED);
        let imported: serde_json::Value =
            serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let publication_id = imported["publication"]["publicationId"].as_str().unwrap();
        assert_eq!(
            router
                .clone()
                .oneshot(open_request_with_operation(publication_id, cycle + 1))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let reader_session_id = active_reader_session
            .load()
            .as_ref()
            .unwrap()
            .reader_session_id
            .to_string();
        wait_for_search_indexing(&search_indexing).await;
        let searched = router
            .clone()
            .oneshot(search_request(&reader_session_id, "alice", None))
            .await
            .unwrap();
        assert_eq!(searched.status(), StatusCode::OK);
        let searched: serde_json::Value =
            serde_json::from_slice(&searched.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(searched["status"], "completed");
        assert!(
            !searched["batch"]["results"].as_array().unwrap().is_empty(),
            "Alice search returned no results on import cycle {cycle}"
        );

        assert_eq!(
            router
                .clone()
                .oneshot(delete_request(publication_id))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn empty_analyzed_document_has_complete_occurrence_search() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata><title xmlns="http://purl.org/dc/elements/1.1/">Empty search</title><identifier xmlns="http://purl.org/dc/elements/1.1/">empty-search</identifier><meta property="dcterms:modified">2026-09-12T00:00:00Z</meta></metadata><manifest><item id="empty" href="empty.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="empty"/></spine></package>"#;
    let bytes = synthetic_epub(
        package,
        &[(
            "EPUB/empty.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body/></html>"#,
        )],
    );
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let search_indexing = state.search_indexing.clone();
    let active_reader_session = state.lifecycle.active_view();
    let router = reader_router(state, None);

    let imported = router
        .clone()
        .oneshot(import_request_with(bytes))
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::CREATED);
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();
    assert_eq!(
        router
            .clone()
            .oneshot(open_request(publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let reader_session_id = active_reader_session
        .load()
        .unwrap()
        .reader_session_id
        .to_string();

    wait_for_search_indexing(&search_indexing).await;
    let searched = router
        .oneshot(occurrence_search_request(
            &reader_session_id,
            "absent",
            0,
            "EPUB/empty.xhtml",
        ))
        .await
        .unwrap();
    assert_eq!(searched.status(), StatusCode::OK);
    let searched: serde_json::Value =
        serde_json::from_slice(&searched.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(searched["status"], "completed", "{searched}");
    assert_eq!(searched["batch"]["results"], serde_json::json!([]));
    assert_eq!(
        searched["batch"]["coverage"],
        serde_json::json!({
            "state": "complete",
            "searchedOccurrences": 1,
            "totalOccurrences": 1
        })
    );
}
