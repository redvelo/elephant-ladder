use super::support::*;

#[test]
fn website_data_purge_report_never_promotes_an_attempt_to_verified_purge() {
    let attempted = WebsiteDataPurgeReport::for_native_result(&Ok(()));
    assert_eq!(attempted.physical_status(), "partial");
    assert_eq!(
        attempted.android_web_storage,
        WebsiteDataPurgeCategoryStatus::Attempted
    );
    assert_eq!(
        attempted.cookies,
        WebsiteDataPurgeCategoryStatus::Unsupported
    );

    let unsupported =
        WebsiteDataPurgeReport::for_native_result(&Err(NativeActionError::Unsupported));
    assert_eq!(unsupported.physical_status(), "unsupported");
}

#[test]
fn product_revisions_have_explicit_identities() {
    assert_eq!(crate::PRODUCTION_READING_REVISION, "reading-v21");
    assert_eq!(crate::SEARCH_REVISION, "search-v4");
    assert_eq!(crate::NATIVE_MEASUREMENT_REVISION, "7");
    assert_ne!(crate::PRODUCTION_READING_REVISION, crate::SEARCH_REVISION);
    let limits = crate::search::search_analysis_limits();
    assert_eq!(limits.max_analyzed_resources, Some(10_000));
    assert_eq!(limits.max_resource_analysis_bytes, Some(128 * 1_048_576));
    assert_eq!(limits.max_total_analysis_bytes, Some(2 * 1024 * 1_048_576));
    assert_eq!(limits.max_total_fingerprint_bytes, Some(0));
    assert_eq!(limits.max_smil_nodes, Some(100_000));
    assert_eq!(limits.max_smil_nesting, Some(256));
}

#[test]
fn active_content_policy_resolver_matrix() {
    use crate::ResolvedActiveContentPolicy::{Disabled, Enabled};
    use crate::storage::{
        ActiveContentDefault, ActiveContentOverride, GlobalPublicationPolicy, PolicyRelevance,
        PublicationPolicy, PublicationPolicyOverrides, PublicationPolicyRelevance,
        PublicationPolicyRevisions, StoredPublicationPolicy,
    };

    for default in [
        ActiveContentDefault::Ask,
        ActiveContentDefault::Automatic,
        ActiveContentDefault::Disabled,
    ] {
        for relevance in [
            PolicyRelevance::Detected,
            PolicyRelevance::Unknown,
            PolicyRelevance::NotDetected,
        ] {
            for explicit in [
                None,
                Some(ActiveContentOverride::Enabled),
                Some(ActiveContentOverride::Disabled),
            ] {
                let policy = PublicationPolicy {
                    global: GlobalPublicationPolicy {
                        active_content: default,
                        ..GlobalPublicationPolicy::default()
                    },
                    publication: PublicationPolicyOverrides {
                        active_content: explicit,
                        ..PublicationPolicyOverrides::default()
                    },
                };
                let stored = StoredPublicationPolicy {
                    effective: policy.effective(),
                    policy,
                    revisions: PublicationPolicyRevisions {
                        global: 0,
                        publication: 0,
                    },
                };
                let resolved = crate::policy::resolve_publication_policy(
                    &stored,
                    PublicationPolicyRelevance {
                        active_content: relevance,
                        remote_resources: PolicyRelevance::NotDetected,
                    },
                );
                if explicit.is_none()
                    && default == ActiveContentDefault::Ask
                    && relevance != PolicyRelevance::NotDetected
                {
                    let required = resolved.unwrap_err();
                    assert!(required.active_content);
                    assert!(!required.remote_resources);
                } else {
                    let expected = match explicit {
                        Some(ActiveContentOverride::Enabled) => Enabled,
                        Some(ActiveContentOverride::Disabled) => Disabled,
                        None if default == ActiveContentDefault::Automatic
                            && relevance != PolicyRelevance::Unknown =>
                        {
                            Enabled
                        }
                        None => Disabled,
                    };
                    assert_eq!(resolved.unwrap().active_content, expected);
                }
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn publication_policy_get_includes_only_current_persisted_relevance() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state_with_actions(
        directory.path(),
        Arc::new(crate::UnsupportedNativeActions),
    );
    let router = reader_router(state, None);
    let imported = router.clone().oneshot(import_request()).await.unwrap();
    assert_eq!(imported.status(), StatusCode::CREATED);
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = imported["publication"]["publicationId"].as_str().unwrap();
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    for (assignment, active, remote) in [
        (
            "active_content_relevance = 'detected', remote_resources_relevance = 'not-detected'",
            "detected",
            "not-detected",
        ),
        (
            "active_content_relevance = 'not-detected', remote_resources_relevance = 'detected'",
            "not-detected",
            "detected",
        ),
        (
            "active_content_relevance = 'unknown'",
            "unknown",
            "detected",
        ),
        (
            "active_content_relevance = 'detected', remote_resources_relevance = 'unknown'",
            "detected",
            "unknown",
        ),
        ("reading_revision = 'stale'", "unknown", "unknown"),
        (
            "revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            "unknown",
            "unknown",
        ),
    ] {
        connection
            .execute(
                "UPDATE publication_snapshots SET reading_revision = ?1",
                [crate::PRODUCTION_READING_REVISION],
            )
            .unwrap();
        connection
            .execute_batch(&format!("UPDATE publication_snapshots SET {assignment}"))
            .unwrap();
        let response = router
            .clone()
            .oneshot(publication_policy_request(
                "GET",
                format!("/native/publications/{id}/policy"),
                None,
                true,
                false,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "policy": {
                    "global": { "activeContent": "ask", "remoteResources": "ask" },
                    "publication": { "activeContent": null, "remoteResources": null }
                },
                "effective": { "activeContent": "ask", "remoteResources": "ask" },
                "revisions": { "global": 0, "publication": 0 },
                "relevance": { "activeContent": active, "remoteResources": remote }
            }),
            "{assignment}"
        );
    }
    connection
        .execute_batch("UPDATE publications SET current_snapshot_id = NULL")
        .unwrap();
    let response = router
        .clone()
        .oneshot(publication_policy_request(
            "GET",
            format!("/native/publications/{id}/policy"),
            None,
            true,
            false,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        body["relevance"],
        serde_json::json!({ "activeContent": "unknown", "remoteResources": "unknown" })
    );
    connection
        .execute_batch("UPDATE publications SET deletion_state = 'deleting'")
        .unwrap();
    for unavailable in [id.to_owned(), "0".repeat(32)] {
        assert_eq!(
            router
                .clone()
                .oneshot(publication_policy_request(
                    "GET",
                    format!("/native/publications/{unavailable}/policy"),
                    None,
                    true,
                    false,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn publication_policy_routes_are_scoped_strict_and_revisioned() {
    let directory = tempfile::tempdir().unwrap();
    let router = reader_router(
        fixture_reader_state_with_actions(
            directory.path(),
            Arc::new(crate::UnsupportedNativeActions),
        ),
        None,
    );
    let imported = router.clone().oneshot(import_request()).await.unwrap();
    assert_eq!(imported.status(), StatusCode::CREATED);
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();

    let global = router
        .clone()
        .oneshot(publication_policy_request(
            "GET",
            "/native/publication-policy".to_owned(),
            None,
            true,
            false,
        ))
        .await
        .unwrap();
    assert_eq!(global.status(), StatusCode::OK);
    let global: serde_json::Value =
        serde_json::from_slice(&global.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(global["policy"]["global"]["activeContent"], "ask");
    assert_eq!(global["effective"]["activeContent"], "ask");
    assert_eq!(global["policy"]["global"]["remoteResources"], "ask");
    assert_eq!(global["revisions"]["global"], 0);

    let global_update = serde_json::json!({
        "baseRevision": 0,
        "policy": {
            "activeContent": "automatic",
            "remoteResources": "blocked"
        }
    });
    let saved_global = router
        .clone()
        .oneshot(publication_policy_request(
            "PUT",
            "/native/publication-policy".to_owned(),
            Some(global_update.clone()),
            true,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(saved_global.status(), StatusCode::OK);
    let saved_global: serde_json::Value =
        serde_json::from_slice(&saved_global.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(saved_global["status"], "saved");
    assert_eq!(saved_global["revisions"]["global"], 1);

    let policy_path = format!("/native/publications/{publication_id}/policy");
    let publication = router
        .clone()
        .oneshot(publication_policy_request(
            "GET",
            policy_path.clone(),
            None,
            true,
            false,
        ))
        .await
        .unwrap();
    assert_eq!(publication.status(), StatusCode::OK);
    let publication: serde_json::Value =
        serde_json::from_slice(&publication.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(publication["effective"]["activeContent"], "enabled");
    assert_eq!(publication["effective"]["remoteResources"], "blocked");

    let publication_update = serde_json::json!({
        "baseRevisions": { "global": 1, "publication": 0 },
        "policy": {
            "activeContent": "disabled",
            "remoteResources": "allowed"
        }
    });
    let saved_publication = router
        .clone()
        .oneshot(publication_policy_request(
            "PUT",
            policy_path.clone(),
            Some(publication_update.clone()),
            true,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(saved_publication.status(), StatusCode::OK);
    let saved_publication: serde_json::Value = serde_json::from_slice(
        &saved_publication
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(saved_publication["effective"]["activeContent"], "disabled");
    assert_eq!(saved_publication["effective"]["remoteResources"], "allowed");
    assert_eq!(saved_publication["revisions"]["publication"], 1);

    let stale = router
        .clone()
        .oneshot(publication_policy_request(
            "PUT",
            policy_path.clone(),
            Some(publication_update),
            true,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let stale: serde_json::Value =
        serde_json::from_slice(&stale.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(stale["reason"], "revision-mismatch");
    assert_eq!(stale["authoritative"]["revisions"]["publication"], 1);

    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    connection
        .execute(
            "INSERT INTO reading_positions (publication_id, position_json, position_revision)
             VALUES (?1, '{}', 7)",
            [publication_id],
        )
        .unwrap();
    let preserved_data = || {
        connection
            .query_row(
                "SELECT origin_id, lifecycle_state, purge_attempts,
                        position_json, position_revision, r.updated_at
                 FROM browser_origin_allocations b JOIN reading_positions r
                 ON b.publication_id = r.publication_id WHERE b.publication_id = ?1",
                [publication_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, u32>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap()
    };
    let before_reset = preserved_data();
    // Reset inherits defaults; repeating it succeeds without advancing the revision.
    for revision in [1, 2] {
        let reset = router
            .clone()
            .oneshot(publication_policy_request(
                "PUT",
                policy_path.clone(),
                Some(serde_json::json!({
                    "baseRevisions": { "global": 1, "publication": revision },
                    "policy": { "activeContent": null, "remoteResources": null }
                })),
                true,
                true,
            ))
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::OK);
        let reset: serde_json::Value =
            serde_json::from_slice(&reset.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(reset["status"], "saved");
        assert_eq!(
            reset["revisions"],
            serde_json::json!({ "global": 1, "publication": 2 })
        );
        assert_eq!(
            reset["policy"]["publication"],
            serde_json::json!({
                "activeContent": null, "remoteResources": null
            })
        );
        assert_eq!(
            reset["effective"],
            serde_json::json!({
                "activeContent": "enabled", "remoteResources": "blocked"
            })
        );
        assert_eq!(preserved_data(), before_reset);
    }
    let reset_body = serde_json::json!({
        "baseRevisions": { "global": 1, "publication": 2 },
        "policy": { "activeContent": null, "remoteResources": null }
    });
    let mut stale_document = publication_policy_request(
        "PUT",
        policy_path.clone(),
        Some(reset_body.clone()),
        true,
        true,
    );
    stale_document.headers_mut().insert(
        crate::LIBRARY_DOCUMENT_HEADER,
        HeaderValue::from_str(&"c".repeat(64)).unwrap(),
    );
    assert_eq!(
        router
            .clone()
            .oneshot(stale_document)
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );

    let missing_scope = router
        .clone()
        .oneshot(publication_policy_request(
            "PUT",
            "/native/publication-policy".to_owned(),
            Some(global_update.clone()),
            true,
            false,
        ))
        .await
        .unwrap();
    assert_eq!(missing_scope.status(), StatusCode::CONFLICT);
    let unauthorized = router
        .clone()
        .oneshot(publication_policy_request(
            "GET",
            policy_path,
            None,
            false,
            false,
        ))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let mut invalid = global_update;
    invalid["unexpected"] = serde_json::json!(true);
    let invalid = router
        .clone()
        .oneshot(publication_policy_request(
            "PUT",
            "/native/publication-policy".to_owned(),
            Some(invalid),
            true,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let other_bytes = synthetic_epub(
            br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata><title xmlns="http://purl.org/dc/elements/1.1/">Other</title><identifier xmlns="http://purl.org/dc/elements/1.1/">other</identifier><meta property="dcterms:modified">2026-09-12T00:00:00Z</meta></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
            &[("EPUB/chapter.xhtml", br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Other book</p></body></html>"#)],
        );
    let other = router
        .clone()
        .oneshot(import_request_with(other_bytes))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::CREATED);
    let other: serde_json::Value =
        serde_json::from_slice(&other.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let other_id = other["publication"]["publicationId"].as_str().unwrap();
    assert_ne!(other_id, publication_id);
    assert_eq!(
        router
            .clone()
            .oneshot(open_request(other_id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let denied = router
        .oneshot(publication_policy_request(
            "PUT",
            format!("/native/publications/{publication_id}/policy"),
            Some(reset_body),
            true,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::CONFLICT);
    let denied: serde_json::Value =
        serde_json::from_slice(&denied.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(denied["reason"], "publication is open");
    assert_eq!(preserved_data(), before_reset);
}
