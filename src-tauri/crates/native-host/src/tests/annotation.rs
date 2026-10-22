use super::support::*;

#[tokio::test]
async fn annotation_links_open_their_publication_at_the_linked_target() {
    use base64::Engine as _;

    let directory = tempfile::tempdir().unwrap();
    let actions = Arc::new(LinkActions(Mutex::new(Vec::new())));
    let state = fixture_reader_state_with_actions(directory.path(), actions.clone());
    state
        .storage
        .lock()
        .unwrap()
        .compare_and_swap_global_publication_policy(
            0,
            crate::storage::GlobalPublicationPolicy {
                active_content: crate::storage::ActiveContentDefault::Disabled,
                remote_resources: crate::storage::RemoteResourcesDefault::Blocked,
            },
        )
        .unwrap();
    let lifecycle = state.lifecycle.clone();
    let router = reader_router(state, None);
    let (_, imported) =
        response_json(router.clone().oneshot(import_request()).await.unwrap()).await;
    let id = imported["publication"]["publicationId"]
        .as_str()
        .unwrap()
        .to_owned();
    let pending_request = || {
        Request::builder()
            .uri("/native/annotation-links/pending")
            .header("host", "reader.test")
            .header("x-elephant-request", "trusted-reader")
            .header(AUTHORIZATION, "Bearer secret")
            .body(Body::empty())
            .unwrap()
    };

    let epub = EpubZip::from_reader(std::io::Cursor::new(ALICE_BYTES.to_vec()))
        .unwrap()
        .default_rendition()
        .unwrap();
    let facts = epub_navigator::facts::PublicationFacts::of(&epub).unwrap();
    let resources = crate::reading::reading_order_local_resources(
        &epub_navigator::kernel::project_reading_order(&facts).unwrap(),
    );
    let (occurrence, resource) = resources
        .iter()
        .enumerate()
        .find_map(|(index, resource)| resource.as_ref().map(|resource| (index, resource)))
        .unwrap();
    let package_directory = facts
        .resources
        .package_path()
        .as_str()
        .rsplit_once('/')
        .map_or(String::new(), |(directory, _)| format!("{directory}/"));
    let target =
        serde_json::json!({ "source": resource.strip_prefix(&package_directory).unwrap() });
    let hash = blake3::hash(ALICE_BYTES).to_hex().to_string();
    actions.0.lock().unwrap().extend([
        "elephant-ladder://annotation?id=not-a-link".to_owned(),
        format!(
            "elephant-ladder://annotation?h={hash}&target={}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(target.to_string())
        ),
    ]);

    let (status, pending) =
        response_json(router.clone().oneshot(pending_request()).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pending["status"], "open");
    assert_eq!(pending["publicationId"], id.as_str());
    let (_, repeated) =
        response_json(router.clone().oneshot(pending_request()).await.unwrap()).await;
    assert_eq!(repeated["linkId"], pending["linkId"]);

    assert_eq!(
        router
            .clone()
            .oneshot(open_request(&id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let bootstrap = pending_reading_bootstrap(&lifecycle);
    assert_eq!(
        bootstrap["initialReadingPosition"]["targetJson"],
        target.to_string()
    );
    assert_eq!(
        bootstrap["initialReadingPosition"]["occurrenceHint"],
        serde_json::json!({ "occurrence": occurrence, "resource": resource })
    );

    actions.0.lock().unwrap().push(format!(
        "elephant-ladder://annotation?h={}&target={}",
        "0".repeat(64),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(target.to_string())
    ));
    let (_, missing) =
        response_json(router.clone().oneshot(pending_request()).await.unwrap()).await;
    assert_eq!(missing["status"], "missing");
}

#[tokio::test]
async fn annotation_sets_import_merge_and_export_per_publication() {
    let directory = tempfile::tempdir().unwrap();
    let router = reader_router(fixture_reader_state(directory.path()), None);
    let (_, imported) =
        response_json(router.clone().oneshot(import_request()).await.unwrap()).await;
    let id = imported["publication"]["publicationId"]
        .as_str()
        .unwrap()
        .to_owned();

    let epub = EpubZip::from_reader(std::io::Cursor::new(ALICE_BYTES.to_vec()))
        .unwrap()
        .default_rendition()
        .unwrap();
    let facts = epub_navigator::facts::PublicationFacts::of(&epub).unwrap();
    let resources = crate::reading::reading_order_local_resources(
        &epub_navigator::kernel::project_reading_order(&facts).unwrap(),
    );
    let resource = resources.iter().flatten().next().unwrap();
    let package_directory = facts
        .resources
        .package_path()
        .as_str()
        .rsplit_once('/')
        .map_or(String::new(), |(directory, _)| format!("{directory}/"));
    let source = resource.strip_prefix(&package_directory).unwrap();
    let set = |identifiers: serde_json::Value| {
        serde_json::json!({
            "@context": "http://www.idpf.org/epub/annotations/v1",
            "type": "AnnotationSet",
            "about": { "dc:identifier": identifiers },
            "items": [{
                "id": "urn:uuid:11111111-1111-4111-8111-111111111111",
                "type": "Annotation",
                "motivation": "commenting",
                "created": "2024-01-01T00:00:00Z",
                "target": {
                    "source": source,
                    "selector": [{
                        "type": "FragmentSelector",
                        "conformsTo": "https://wicg.github.io/scroll-to-text-fragment/",
                        "value": "text=Alice"
                    }]
                },
                "body": { "type": "TextualBody", "value": "A note", "format": "text/plain" }
            }, {
                "id": "urn:uuid:22222222-2222-4222-8222-222222222222",
                "type": "Annotation",
                "motivation": "highlighting",
                "created": "2024-01-01T00:00:00Z",
                "target": { "source": source }
            }]
        })
        .to_string()
    };

    let (status, body) = response_json(
        router
            .clone()
            .oneshot(annotation_import_request(
                &id,
                "",
                set(serde_json::json!([])),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["report"]["added"], 2);

    let exported = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/native/publications/{id}/annotations/export"))
                .header("host", "reader.test")
                .header("x-elephant-request", "trusted-reader")
                .header(AUTHORIZATION, "Bearer secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, exported) = response_json(exported).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(exported["items"].as_array().unwrap().len(), 2);
    assert!(
        exported["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["target"]["meta"]["text"]["highlight"] == "Alice")
    );
    assert!(
        exported["about"]["dc:identifier"]
            .as_array()
            .unwrap()
            .iter()
            .any(|identifier| identifier.as_str().unwrap().starts_with("urn:blake3:"))
    );

    let (status, body) = response_json(
        router
            .clone()
            .oneshot(annotation_import_request(&id, "", exported.to_string()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["report"]["unchanged"], 2);

    let foreign = set(serde_json::json!(["urn:isbn:0"]));
    let (status, body) = response_json(
        router
            .clone()
            .oneshot(annotation_import_request(&id, "", foreign.clone()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["status"], "publication-mismatch");
    let (status, _) = response_json(
        router
            .clone()
            .oneshot(annotation_import_request(
                &id,
                "?allowPublicationMismatch=true",
                foreign,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut unauthorized = annotation_import_request(&id, "", set(serde_json::json!([])));
    unauthorized
        .headers_mut()
        .remove(crate::LIBRARY_DOCUMENT_HEADER);
    assert_eq!(
        router.clone().oneshot(unauthorized).await.unwrap().status(),
        StatusCode::CONFLICT
    );

    assert_eq!(
        router
            .clone()
            .oneshot(open_request(&id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
