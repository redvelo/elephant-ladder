// The wire and the storage layer both mutate the pagination cache in batches.
// These express one record as a batch of one so per-record tests stay readable
// without a second persistence path existing to test.
impl crate::storage::Storage {
    pub(crate) fn read_pagination_record(
        &mut self,
        publication_id: &str,
        snapshot_id: &str,
        basis_hash: &str,
        record_hash: &str,
    ) -> Result<PaginationCacheReadOutcome, String> {
        let records = [record_hash.to_owned()];
        Ok(
            match self.read_pagination_records(publication_id, snapshot_id, basis_hash, &records)? {
                PaginationCacheRecordsReadOutcome::Unavailable => {
                    PaginationCacheReadOutcome::Unavailable
                }
                PaginationCacheRecordsReadOutcome::Records(mut values) => {
                    match values.pop().flatten() {
                        Some(value) => PaginationCacheReadOutcome::Found(value),
                        None => PaginationCacheReadOutcome::Missing,
                    }
                }
            },
        )
    }

    pub(crate) fn write_pagination_record(
        &mut self,
        publication_id: &str,
        snapshot_id: &str,
        basis_hash: &str,
        record_hash: &str,
        value: &[u8],
    ) -> Result<PaginationCacheMutationOutcome, String> {
        self.mutate_pagination_records(
            publication_id,
            snapshot_id,
            basis_hash,
            &[PaginationCacheMutation::Write {
                record_hash: record_hash.to_owned(),
                value: value.to_vec(),
            }],
        )
    }

    pub(crate) fn remove_pagination_record(
        &mut self,
        publication_id: &str,
        snapshot_id: &str,
        basis_hash: &str,
        record_hash: &str,
    ) -> Result<PaginationCacheMutationOutcome, String> {
        self.mutate_pagination_records(
            publication_id,
            snapshot_id,
            basis_hash,
            &[PaginationCacheMutation::Remove {
                record_hash: record_hash.to_owned(),
            }],
        )
    }
}

#[test]
fn unusable_stored_media_overlay_degrades_instead_of_failing_the_load() {
    use crate::media_overlay::{
        MediaOverlayAvailabilityIssue, MediaOverlayPackageFacts, MediaOverlayPackageIssue,
        MediaOverlayPlayback,
    };

    let valid = super::test_absent_media_overlay_json();
    let absent_capability =
        serde_json::to_string(&crate::media_overlay::MediaOverlayCapability::Absent {}).unwrap();
    let (description, capability) = super::load_stored_media_overlay(&valid, &absent_capability);
    assert!(matches!(
        description.playback,
        MediaOverlayPlayback::Absent {}
    ));
    assert!(matches!(
        capability,
        crate::media_overlay::MediaOverlayCapability::Absent {}
    ));

    let degraded = |description_json: &str, capability_json: &str| {
        let (description, capability) =
            super::load_stored_media_overlay(description_json, capability_json);
        assert!(matches!(
            description.package,
            MediaOverlayPackageFacts::Unavailable {
                reason: MediaOverlayPackageIssue::RepresentationLimitExceeded
            }
        ));
        assert!(matches!(
            description.playback,
            MediaOverlayPlayback::Unavailable {
                reason: MediaOverlayAvailabilityIssue::RepresentationLimitExceeded
            }
        ));
        assert!(matches!(
            capability,
            crate::media_overlay::MediaOverlayCapability::Unavailable {}
        ));
        description
    };

    degraded("not json at all", &absent_capability);
    degraded(&valid, "not json at all");
    // A capability that disagrees with its description degrades rather than being trusted.
    let mismatched =
        serde_json::to_string(&crate::media_overlay::MediaOverlayCapability::Available {
            playable_occurrence_indexes: vec![0],
            unavailable_occurrence_count: 0,
        })
        .unwrap();
    let description = degraded(&valid, &mismatched);

    // The degraded description must itself validate, or opening would reject it in turn.
    crate::media_overlay::validate_media_overlay_description(&description).unwrap();
}

use rusqlite::{Connection, params};

use super::{
    ActiveContentDefault, ActiveContentOverride, AnnotationMutation,
    BeginPublicationDeletionOutcome, EffectiveActiveContentPolicy, EffectiveRemoteResourcesPolicy,
    InstalledImport, PaginationCacheMutation, PaginationCacheMutationOutcome,
    PaginationCacheReadOutcome, PaginationCacheRecordsReadOutcome, PublicationPolicy,
    PublicationPolicyOverrides, RemoteResourcesDefault, RemoteResourcesOverride,
    ReserveImportOutcome, ResetPublicationWebsiteDataOutcome, SaveAnnotationOutcome,
    SavePreferencesOutcome, SavePublicationPolicyOutcome, SaveReadingPositionOutcome, Storage,
    sync_directory,
};

#[test]
fn storage_has_one_exclusive_host_owner() {
    let directory = tempfile::tempdir().unwrap();
    let first = Storage::open(directory.path()).unwrap();
    assert!(Storage::open(directory.path()).is_err());
    drop(first);
    Storage::open(directory.path()).unwrap();
}

#[test]
fn installation_origin_ports_are_initialized_once_and_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    {
        let mut storage = Storage::open(directory.path()).unwrap();
        assert_eq!(storage.installation_origin_config().unwrap(), None);
        storage.initialize_origin_ports(43123, 43124).unwrap();
        storage.initialize_origin_ports(43123, 43124).unwrap();
        assert!(storage.initialize_origin_ports(43125, 43124).is_err());
        assert!(storage.initialize_origin_ports(43123, 43125).is_err());
    }
    let storage = Storage::open(directory.path()).unwrap();
    assert_eq!(
        storage.installation_origin_config().unwrap(),
        Some(super::InstallationOriginConfig {
            foreground_port: 43123,
            reader_port: Some(43124),
        })
    );
}

#[test]
fn import_reservations_enforce_quota_and_cancel_releases_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open_for_test(directory.path(), 10).unwrap();
    let reservation = match storage.reserve_import(6).unwrap() {
        ReserveImportOutcome::Reserved(reservation) => reservation,
        ReserveImportOutcome::QuotaExceeded => panic!("first reservation must fit"),
    };
    assert!(matches!(
        storage.reserve_import(5).unwrap(),
        ReserveImportOutcome::QuotaExceeded
    ));
    storage
        .cancel_import_reservation(&reservation.id, None)
        .unwrap();
    assert!(matches!(
        storage.reserve_import(5).unwrap(),
        ReserveImportOutcome::Reserved(_)
    ));
}

#[test]
fn expired_reservation_is_reaped_before_new_quota_admission() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"expired reservation";
    let mut storage = Storage::open_for_test(directory.path(), bytes.len() as u64).unwrap();
    let reservation = match storage.reserve_import(bytes.len() as u64).unwrap() {
        ReserveImportOutcome::Reserved(reservation) => reservation,
        ReserveImportOutcome::QuotaExceeded => panic!("reservation must fit"),
    };
    std::fs::write(&reservation.staging_path, bytes).unwrap();
    let hash = blake3::hash(bytes).to_hex().to_string();
    let installed = storage
        .install_staged_blob(&reservation.id, &hash, bytes.len() as u64)
        .unwrap();
    storage
        .connection
        .execute(
            "UPDATE import_reservations
                 SET created_at = '2000-01-01 00:00:00', expires_at = '2000-01-01 01:00:00'
                 WHERE reservation_id = ?1",
            [&reservation.id],
        )
        .unwrap();

    assert!(matches!(
        storage.reserve_import(bytes.len() as u64).unwrap(),
        ReserveImportOutcome::Reserved(_)
    ));
    assert!(!installed.path.exists());
}

#[test]
fn reserved_import_commit_consumes_reservation_and_deduplicates_quota() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"reserved archive";
    let quota = u64::try_from(bytes.len() * 2).unwrap();
    let mut storage = Storage::open_for_test(directory.path(), quota).unwrap();
    let reservation = match storage.reserve_import(bytes.len() as u64).unwrap() {
        ReserveImportOutcome::Reserved(reservation) => reservation,
        ReserveImportOutcome::QuotaExceeded => panic!("reservation must fit"),
    };
    std::fs::write(&reservation.staging_path, bytes).unwrap();
    let hash = blake3::hash(bytes).to_hex().to_string();
    let blob = storage
        .install_staged_blob(&reservation.id, &hash, bytes.len() as u64)
        .unwrap();
    let (facts, responses, _) = snapshot_values();
    storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            "reading-reservation-test",
            "Reserved title",
            &facts,
            &responses,
        )
        .unwrap();
    assert_eq!(
        storage
            .connection
            .query_row("SELECT COUNT(*) FROM import_reservations", [], |row| {
                row.get::<_, u32>(0)
            })
            .unwrap(),
        0
    );
    let duplicate_reservation = match storage.reserve_import(bytes.len() as u64).unwrap() {
        ReserveImportOutcome::Reserved(reservation) => reservation,
        ReserveImportOutcome::QuotaExceeded => panic!("duplicate staging must fit"),
    };
    std::fs::write(&duplicate_reservation.staging_path, bytes).unwrap();
    let duplicate_blob = storage
        .install_staged_blob(
            &duplicate_reservation.id,
            &hash,
            u64::try_from(bytes.len()).unwrap(),
        )
        .unwrap();
    let duplicate = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &duplicate_blob,
                cover: None,
            },
            "reading-reservation-test",
            "Ignored duplicate title",
            &facts,
            &responses,
        )
        .unwrap();
    assert!(!duplicate.created);
    assert_eq!(
        storage
            .connection
            .query_row("SELECT COUNT(*) FROM archive_blobs", [], |row| {
                row.get::<_, u32>(0)
            })
            .unwrap(),
        1
    );
    assert_eq!(
        storage
            .connection
            .query_row("SELECT COUNT(*) FROM import_reservations", [], |row| {
                row.get::<_, u32>(0)
            })
            .unwrap(),
        0
    );
    assert!(matches!(
        storage.reserve_import(bytes.len() as u64).unwrap(),
        ReserveImportOutcome::Reserved(_)
    ));
}

#[test]
fn reserved_staging_cannot_be_installed_under_the_wrong_digest() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"digest-bound archive";
    let mut storage = Storage::open(directory.path()).unwrap();
    let reservation = match storage.reserve_import(bytes.len() as u64).unwrap() {
        ReserveImportOutcome::Reserved(reservation) => reservation,
        ReserveImportOutcome::QuotaExceeded => panic!("reservation must fit"),
    };
    std::fs::write(&reservation.staging_path, bytes).unwrap();
    assert!(
        storage
            .install_staged_blob(
                &reservation.id,
                &"0".repeat(64),
                u64::try_from(bytes.len()).unwrap(),
            )
            .is_err()
    );
    assert!(reservation.staging_path.exists());
    storage
        .cancel_import_reservation(&reservation.id, None)
        .unwrap();
}

#[test]
fn startup_reconciliation_removes_abandoned_import_artifacts() {
    let directory = tempfile::tempdir().unwrap();
    let (staging_path, blob_path) = {
        let mut storage = Storage::open(directory.path()).unwrap();
        let bytes = b"renamed before commit";
        let reservation = match storage.reserve_import(bytes.len() as u64).unwrap() {
            ReserveImportOutcome::Reserved(reservation) => reservation,
            ReserveImportOutcome::QuotaExceeded => panic!("reservation must fit"),
        };
        std::fs::write(&reservation.staging_path, bytes).unwrap();
        let hash = blake3::hash(bytes).to_hex().to_string();
        let blob = storage
            .install_staged_blob(&reservation.id, &hash, bytes.len() as u64)
            .unwrap();
        std::fs::write(storage.staging_path().join("orphan.part"), b"partial").unwrap();
        (reservation.staging_path, blob.path)
    };

    assert!(!staging_path.exists());
    assert!(blob_path.exists());
    let storage = Storage::open(directory.path()).unwrap();
    assert!(!blob_path.exists());
    assert_eq!(
        storage
            .connection
            .query_row("SELECT COUNT(*) FROM import_reservations", [], |row| {
                row.get::<_, u32>(0)
            })
            .unwrap(),
        0
    );
    assert!(storage.staging_path().read_dir().unwrap().next().is_none());
}

#[test]
fn startup_reconciliation_marks_missing_and_wrong_sized_blobs_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    let (missing, corrupt) = {
        let mut storage = Storage::open(directory.path()).unwrap();
        let missing = storage.install_blob(b"missing archive").unwrap();
        let corrupt = storage.install_blob(b"wrong sized archive").unwrap();
        let (facts, responses, _) = snapshot_values();
        for (blob, title) in [(&missing, "Missing"), (&corrupt, "Corrupt")] {
            storage
                .commit_import_snapshot(
                    InstalledImport {
                        archive: blob,
                        cover: None,
                    },
                    "reading-health-test",
                    title,
                    &facts,
                    &responses,
                )
                .unwrap();
        }
        std::fs::remove_file(&missing.path).unwrap();
        std::fs::write(&corrupt.path, b"x").unwrap();
        (missing, corrupt)
    };

    let storage = Storage::open(directory.path()).unwrap();
    for (hash, expected) in [(missing.hash, "missing"), (corrupt.hash, "corrupt")] {
        let health = storage
            .connection
            .query_row(
                "SELECT health_state FROM archive_blobs WHERE blob_hash = ?1",
                [hash],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(health, expected);
    }
}

#[test]
fn publication_deletion_detaches_state_and_reclaims_its_blob() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"deleted archive");
    let (snapshot_id, blob_path) = publication_snapshot(directory.path(), &publication_id);
    let mut storage = Storage::open(directory.path()).unwrap();
    let origin_id = browser_origin_id(&storage, &publication_id);
    storage
        .compare_and_swap_reading_position(
            &publication_id,
            0,
            r#"{"targetJson":"{}","occurrenceHint":null}"#,
        )
        .unwrap();
    let mut preferences = storage
        .load_publication_preferences(&publication_id)
        .unwrap()
        .preferences;
    preferences.presentation_mode = "document-scroll".to_owned();
    storage
        .compare_and_swap_preferences(&publication_id, 0, 0, &preferences)
        .unwrap();
    storage
        .write_pagination_record(
            &publication_id,
            &snapshot_id,
            &"a".repeat(64),
            &"b".repeat(64),
            b"pages",
        )
        .unwrap();

    let snapshot_ids = match storage.begin_publication_deletion(&publication_id).unwrap() {
        BeginPublicationDeletionOutcome::Started { snapshot_ids } => snapshot_ids,
        BeginPublicationDeletionOutcome::NotFound => panic!("publication must exist"),
    };
    assert_eq!(snapshot_ids, vec![snapshot_id]);
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT lifecycle_state FROM browser_origin_allocations WHERE origin_id = ?1",
                [&origin_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "purge_pending"
    );
    assert!(storage.list_publications().unwrap().is_empty());
    storage
        .complete_publication_deletion(&publication_id)
        .unwrap();

    assert!(!blob_path.exists());
    for table in [
        "publications",
        "publication_snapshots",
        "reading_positions",
        "publication_reader_preferences",
        "archive_blobs",
        "deletion_operations",
        "blob_cleanup_operations",
    ] {
        let count = storage
            .connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} must be empty");
    }
    // No foreign key reaches across files, so the explicit collection has to have
    // taken the deleted publication's cached pages with it.
    assert_eq!(
        storage
            .pagination
            .query_row("SELECT COUNT(*) FROM pagination_bases", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        0,
        "pagination cache must be empty"
    );
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT publication_id, lifecycle_state FROM browser_origin_allocations
                     WHERE origin_id = ?1",
                [&origin_id],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap(),
        (None, "purge_pending".to_owned())
    );
    assert!(storage.trash.read_dir().unwrap().next().is_none());
}

#[test]
fn reimport_after_deletion_gets_a_new_browser_origin() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"deleted and reimported archive";
    let first_publication_id = create_test_publication(directory.path(), bytes);
    let mut storage = Storage::open(directory.path()).unwrap();
    let first_origin_id = browser_origin_id(&storage, &first_publication_id);
    storage
        .begin_publication_deletion(&first_publication_id)
        .unwrap();
    storage
        .complete_publication_deletion(&first_publication_id)
        .unwrap();

    let blob = storage.install_blob(bytes).unwrap();
    let (facts, responses, _) = snapshot_values();
    let reimported = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            "reading-v1",
            "Reimported",
            &facts,
            &responses,
        )
        .unwrap();
    let second_publication_id = reimported.publication.publication_id;
    let second_origin_id = browser_origin_id(&storage, &second_publication_id);
    assert_ne!(second_publication_id, first_publication_id);
    assert_ne!(second_origin_id, first_origin_id);
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM browser_origin_allocations",
                [],
                |row| row.get::<_, u32>(0),
            )
            .unwrap(),
        2
    );
}

#[test]
fn website_data_reset_rotates_only_the_browser_origin() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"website data reset");
    let (snapshot_id, blob_path) = publication_snapshot(directory.path(), &publication_id);
    let blob_bytes = std::fs::read(&blob_path).unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    storage
        .compare_and_swap_reading_position(
            &publication_id,
            0,
            r#"{"targetJson":"{}","occurrenceHint":null}"#,
        )
        .unwrap();
    let mut preferences = storage
        .load_publication_preferences(&publication_id)
        .unwrap()
        .preferences;
    preferences.presentation_mode = "document-scroll".to_owned();
    storage
        .compare_and_swap_preferences(&publication_id, 0, 0, &preferences)
        .unwrap();
    assert!(matches!(
        save_annotation(
            &mut storage,
            &publication_id,
            serde_json::json!({
                "type": "create-highlight", "requestId": "request-1",
                "basedOnRevision": 0, "selectionRevision": 1, "occurrence": 0,
                "targetJson": "{}", "excerpt": "selected", "sectionLabel": "One"
            }),
            &[Some("EPUB/chapter.xhtml".to_owned())],
        ),
        SaveAnnotationOutcome::Saved { .. }
    ));
    storage
        .write_pagination_record(
            &publication_id,
            &snapshot_id,
            &"a".repeat(64),
            &"b".repeat(64),
            b"pages",
        )
        .unwrap();
    let publication_state_before = storage
        .connection
        .query_row(
            "SELECT current_snapshot_id, imported_at, title_summary
                 FROM publications WHERE publication_id = ?1",
            [&publication_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .unwrap();
    let first_origin = browser_origin_id(&storage, &publication_id);

    assert!(matches!(
        storage
            .reset_publication_website_data(&publication_id)
            .unwrap(),
        ResetPublicationWebsiteDataOutcome::Reset { .. }
    ));
    let replacement = browser_origin_id(&storage, &publication_id);
    assert_ne!(replacement, first_origin);
    assert_eq!(browser_origin_id(&storage, &publication_id), replacement);
    assert_eq!(
        storage
            .load_reading_position(&publication_id)
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        storage
            .load_publication_preferences(&publication_id)
            .unwrap()
            .preferences
            .presentation_mode,
        "document-scroll"
    );
    assert_eq!(
        storage.load_annotations(&publication_id).unwrap().revision,
        1
    );
    assert!(matches!(
        storage
            .read_pagination_record(
                &publication_id,
                &snapshot_id,
                &"a".repeat(64),
                &"b".repeat(64),
            )
            .unwrap(),
        PaginationCacheReadOutcome::Found(value) if value == b"pages"
    ));
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT current_snapshot_id, imported_at, title_summary
                     FROM publications WHERE publication_id = ?1",
                [&publication_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap(),
        publication_state_before
    );
    assert_eq!(std::fs::read(&blob_path).unwrap(), blob_bytes);
    assert_eq!(storage.list_publications().unwrap().len(), 1);
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT lifecycle_state FROM browser_origin_allocations WHERE origin_id = ?1",
                [&first_origin],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "purge_pending"
    );

    assert!(matches!(
        storage
            .reset_publication_website_data(&publication_id)
            .unwrap(),
        ResetPublicationWebsiteDataOutcome::Reset { .. }
    ));
    let second_replacement = browser_origin_id(&storage, &publication_id);
    assert_ne!(second_replacement, first_origin);
    assert_ne!(second_replacement, replacement);
    drop(storage);

    let storage = Storage::open(directory.path()).unwrap();
    assert_eq!(
        browser_origin_id(&storage, &publication_id),
        second_replacement
    );
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM browser_origin_allocations
                     WHERE publication_id = ?1 AND lifecycle_state = 'purge_pending'",
                [&publication_id],
                |row| row.get::<_, u32>(0),
            )
            .unwrap(),
        2
    );
}

#[test]
fn website_data_reset_rejects_missing_and_deleting_publications() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"reset unavailable");
    let mut storage = Storage::open(directory.path()).unwrap();
    assert!(matches!(
        storage
            .reset_publication_website_data("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap(),
        ResetPublicationWebsiteDataOutcome::NotFound
    ));
    storage.begin_publication_deletion(&publication_id).unwrap();
    assert!(matches!(
        storage
            .reset_publication_website_data(&publication_id)
            .unwrap(),
        ResetPublicationWebsiteDataOutcome::NotFound
    ));
}

#[test]
fn startup_resumes_deletion_after_transactional_detach() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"interrupted deletion");
    let (_, blob_path) = publication_snapshot(directory.path(), &publication_id);
    {
        let mut storage = Storage::open(directory.path()).unwrap();
        assert!(matches!(
            storage.begin_publication_deletion(&publication_id).unwrap(),
            BeginPublicationDeletionOutcome::Started { .. }
        ));
        let cleanup = storage.detach_publication(&publication_id).unwrap();
        assert_eq!(cleanup.len(), 1);
        assert!(blob_path.exists());
    }

    let storage = Storage::open(directory.path()).unwrap();
    assert!(storage.list_publications().unwrap().is_empty());
    assert!(!blob_path.exists());
    assert_eq!(
        storage
            .connection
            .query_row("SELECT COUNT(*) FROM deletion_operations", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn startup_resumes_blob_cleanup_after_trash_rename() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"trashed interruption");
    let (_, blob_path) = publication_snapshot(directory.path(), &publication_id);
    let trash_path = {
        let mut storage = Storage::open(directory.path()).unwrap();
        storage.begin_publication_deletion(&publication_id).unwrap();
        let cleanup = storage.detach_publication(&publication_id).unwrap();
        let trash_path = storage.trash.join(&cleanup[0]);
        std::fs::rename(&blob_path, &trash_path).unwrap();
        sync_directory(&storage.blobs).unwrap();
        sync_directory(&storage.trash).unwrap();
        trash_path
    };

    assert!(trash_path.exists());
    let storage = Storage::open(directory.path()).unwrap();
    assert!(!trash_path.exists());
    assert!(!blob_path.exists());
    assert!(storage.list_publications().unwrap().is_empty());
    assert_eq!(
        storage
            .connection
            .query_row("SELECT COUNT(*) FROM archive_blobs", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn blob_cleanup_stops_before_touching_a_new_reference() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"referenced cleanup");
    let (_, blob_path) = publication_snapshot(directory.path(), &publication_id);
    let mut storage = Storage::open(directory.path()).unwrap();
    storage.begin_publication_deletion(&publication_id).unwrap();
    let cleanup = storage.detach_publication(&publication_id).unwrap();
    let blob_hash = cleanup[0].clone();
    storage
        .connection
        .execute(
            "INSERT INTO publications
                    (publication_id, imported_blob_hash, title_summary, deletion_state)
                 VALUES (?1, ?2, 'Replacement', 'live')",
            params!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", blob_hash],
        )
        .unwrap();

    storage.complete_blob_cleanup(&blob_hash).unwrap();
    assert!(blob_path.exists());
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT lifecycle_state FROM archive_blobs WHERE blob_hash = ?1",
                [&blob_hash],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "live"
    );
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM blob_cleanup_operations WHERE blob_hash = ?1",
                [&blob_hash],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn late_preparation_cannot_recreate_a_deleted_publication() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"late preparation");
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage
        .load_publication_blob(&publication_id)
        .unwrap()
        .unwrap();
    let (facts, responses, _) = snapshot_values();
    storage.begin_publication_deletion(&publication_id).unwrap();
    storage
        .complete_publication_deletion(&publication_id)
        .unwrap();

    assert!(
        storage
            .commit_import_snapshot(
                InstalledImport {
                    archive: &blob,
                    cover: None,
                },
                "reading-v2",
                "Must not return",
                &facts,
                &responses,
            )
            .is_err()
    );
    assert!(storage.list_publications().unwrap().is_empty());
    assert_eq!(
        storage
            .connection
            .query_row("SELECT COUNT(*) FROM publications", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn rejects_corrupt_existing_blob() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"fixture archive").unwrap();
    std::fs::write(&blob.path, b"corrupt archive").unwrap();
    assert!(storage.install_blob(b"fixture archive").is_err());
}

#[test]
fn creates_a_durable_imported_publication() {
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"imported archive").unwrap();
    let (facts, responses, _) = snapshot_values();
    let outcome = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"1".repeat(64),
            "Imported title",
            &facts,
            &responses,
        )
        .unwrap();
    assert!(outcome.created);
    assert_eq!(outcome.publication.title, "Imported title");
    assert_eq!(storage.list_publications().unwrap().len(), 1);
}

#[test]
fn duplicate_import_returns_the_existing_publication() {
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"duplicate archive").unwrap();
    let (facts, responses, _) = snapshot_values();
    let first = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"2".repeat(64),
            "Original title",
            &facts,
            &responses,
        )
        .unwrap();
    let first_origin = browser_origin_id(&storage, &first.publication.publication_id);
    let duplicate = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"2".repeat(64),
            "Replacement title",
            &facts,
            &responses,
        )
        .unwrap();
    assert!(!duplicate.created);
    assert_eq!(
        duplicate.publication.publication_id,
        first.publication.publication_id
    );
    assert_eq!(duplicate.publication.title, "Original title");
    assert_eq!(
        browser_origin_id(&storage, &duplicate.publication.publication_id),
        first_origin
    );
    assert_eq!(storage.list_publications().unwrap().len(), 1);
}

#[test]
fn duplicate_import_replaces_a_stale_derivation() {
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"stale derivation archive").unwrap();
    let (facts, responses, _) = snapshot_values();
    let first = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"1".repeat(64),
            "Old derivation",
            &facts,
            &responses,
        )
        .unwrap();
    let replacement = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"2".repeat(64),
            "Current derivation",
            &facts,
            &responses,
        )
        .unwrap();

    assert!(!replacement.created);
    assert_eq!(
        replacement.publication.publication_id,
        first.publication.publication_id
    );
    assert_eq!(replacement.publication.title, "Current derivation");
    let current_digest = storage
        .connection
        .query_row(
            "SELECT s.reading_revision FROM publications p
                 JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                 WHERE p.publication_id = ?1",
            [&first.publication.publication_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(current_digest, "2".repeat(64));
    assert_eq!(
        storage
            .connection
            .query_row("SELECT count(*) FROM publication_snapshots", [], |row| row
                .get::<_, u32>(
                0
            ))
            .unwrap(),
        2
    );
}

#[test]
fn imported_publication_reopens_with_its_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let reading_revision = "reading-test";
    let publication_id = {
        let mut storage = Storage::open(directory.path()).unwrap();
        let blob = storage.install_blob(b"reopen archive").unwrap();
        let (facts, responses, _) = snapshot_values();
        let outcome = storage
            .commit_import_snapshot(
                InstalledImport {
                    archive: &blob,
                    cover: None,
                },
                reading_revision,
                "Reopened title",
                &facts,
                &responses,
            )
            .unwrap();
        outcome.publication.publication_id
    };
    let storage = Storage::open(directory.path()).unwrap();
    let listed = storage.list_publications().unwrap();
    assert_eq!(listed[0].publication_id, publication_id);
    let stored_revision: String = storage
        .connection
        .query_row(
            "SELECT s.reading_revision
                 FROM publications p
                 JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                 WHERE p.publication_id = ?1",
            [&publication_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_revision, reading_revision);
}

#[test]
fn imported_cover_is_content_addressed_and_survives_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let cover_bytes = b"original cover bytes";
    let publication_id = {
        let mut storage = Storage::open(directory.path()).unwrap();
        let blob = storage.install_blob(b"cover archive").unwrap();
        let cover_hash = blake3::hash(cover_bytes).to_hex().to_string();
        let cover_staging = storage.staging_path().join("cover.part");
        std::fs::write(&cover_staging, cover_bytes).unwrap();
        let cover = storage
            .install_staged_cover(
                &cover_staging,
                &cover_hash,
                cover_bytes.len() as u64,
                "image/avif",
            )
            .unwrap();
        let (facts, responses, _) = snapshot_values();
        let outcome = storage
            .commit_import_snapshot(
                InstalledImport {
                    archive: &blob,
                    cover: Some(&cover),
                },
                &"5".repeat(64),
                "Covered title",
                &facts,
                &responses,
            )
            .unwrap();
        assert!(outcome.publication.has_cover);
        outcome.publication.publication_id
    };

    let storage = Storage::open(directory.path()).unwrap();
    let cover = storage
        .load_publication_cover(&publication_id)
        .unwrap()
        .unwrap();
    assert_eq!(cover.mime, "image/avif");
    assert_eq!(cover.size, cover_bytes.len() as u64);
    assert_eq!(std::fs::read(cover.path).unwrap(), cover_bytes);
    assert!(storage.list_publications().unwrap()[0].has_cover);
}

#[test]
fn duplicate_import_backfills_a_legacy_cover() {
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"legacy cover archive").unwrap();
    let (facts, responses, _) = snapshot_values();
    let first = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"6".repeat(64),
            "Legacy title",
            &facts,
            &responses,
        )
        .unwrap();
    assert!(!first.publication.has_cover);

    let cover_bytes = b"legacy cover";
    let cover_hash = blake3::hash(cover_bytes).to_hex().to_string();
    let cover_staging = storage.staging_path().join("legacy-cover.part");
    std::fs::write(&cover_staging, cover_bytes).unwrap();
    let cover = storage
        .install_staged_cover(
            &cover_staging,
            &cover_hash,
            cover_bytes.len() as u64,
            "image/jxl",
        )
        .unwrap();
    let duplicate = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: Some(&cover),
            },
            &"6".repeat(64),
            "Ignored title",
            &facts,
            &responses,
        )
        .unwrap();
    assert!(!duplicate.created);
    assert!(duplicate.publication.has_cover);
    assert_eq!(
        duplicate.publication.publication_id,
        first.publication.publication_id
    );
    assert_eq!(
        storage
            .load_publication_cover(&first.publication.publication_id)
            .unwrap()
            .unwrap()
            .mime,
        "image/jxl"
    );
}

#[test]
fn reading_position_compare_and_swap_persists_across_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = {
        let mut storage = Storage::open(directory.path()).unwrap();
        let blob = storage.install_blob(b"position archive").unwrap();
        let (facts, responses, _) = snapshot_values();
        let publication_id = storage
            .commit_import_snapshot(
                InstalledImport {
                    archive: &blob,
                    cover: None,
                },
                &"4".repeat(64),
                "Position title",
                &facts,
                &responses,
            )
            .unwrap()
            .publication
            .publication_id;
        assert!(matches!(
            storage
                .compare_and_swap_reading_position(
                    &publication_id,
                    0,
                    r#"{"targetJson":"one","occurrenceHint":null}"#
                )
                .unwrap(),
            SaveReadingPositionOutcome::Saved { revision: 1 }
        ));
        assert!(matches!(
            storage
                .compare_and_swap_reading_position(
                    &publication_id,
                    0,
                    r#"{"targetJson":"stale","occurrenceHint":null}"#
                )
                .unwrap(),
            SaveReadingPositionOutcome::RevisionMismatch {
                current_revision: 1
            }
        ));
        publication_id
    };

    let mut storage = Storage::open(directory.path()).unwrap();
    let stored = storage
        .load_reading_position(&publication_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.revision, 1);
    assert!(stored.position_json.contains("one"));
    assert!(matches!(
        storage
            .compare_and_swap_reading_position(
                &publication_id,
                1,
                r#"{"targetJson":"two","occurrenceHint":null}"#
            )
            .unwrap(),
        SaveReadingPositionOutcome::Saved { revision: 2 }
    ));
}

#[test]
fn reflection_drafts_keep_their_original_location_across_reopen() {
    use super::{JournalError, JournalMutation, ReadingSummary};
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"journal archive").unwrap();
    let (facts, responses, _) = snapshot_values();
    let publication_id = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"4".repeat(64),
            "Journal title",
            &facts,
            &responses,
        )
        .unwrap()
        .publication
        .publication_id;
    let first = ReadingSummary {
        chapter: Some("Chapter one".into()),
        pages: None,
    };
    storage
        .save_reading_position_summary(
            &publication_id,
            0,
            r#"{"targetJson":"one","occurrenceHint":null}"#,
            Some(&first),
        )
        .unwrap();
    let draft = storage
        .mutate_journal(&publication_id, JournalMutation::Begin)
        .unwrap()
        .draft
        .unwrap();
    storage
        .mutate_journal(
            &publication_id,
            JournalMutation::Update {
                id: draft.id.clone(),
                text: "A thought worth keeping".into(),
            },
        )
        .unwrap();
    let second = ReadingSummary {
        chapter: Some("Chapter two".into()),
        pages: None,
    };
    storage
        .save_reading_position_summary(
            &publication_id,
            1,
            r#"{"targetJson":"two","occurrenceHint":null}"#,
            Some(&second),
        )
        .unwrap();
    drop(storage);
    let mut storage = Storage::open(directory.path()).unwrap();
    let restored = storage
        .mutate_journal(&publication_id, JournalMutation::Begin)
        .unwrap();
    assert_eq!(restored.summary, Some(second.clone()));
    let restored = restored.draft.unwrap();
    assert_eq!(restored.id, draft.id);
    assert_eq!(restored.context, Some(first));
    assert_eq!(restored.text, "A thought worth keeping");
    let position: String = storage
        .connection
        .query_row(
            "SELECT position_json FROM reflections WHERE id = ?1",
            [&draft.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(position.contains("one"));
    let published = storage
        .mutate_journal(&publication_id, JournalMutation::Publish { id: draft.id })
        .unwrap();
    assert!(published.draft.is_none());
    assert_eq!(published.entries.len(), 1);
    let next = storage
        .mutate_journal(&publication_id, JournalMutation::Begin)
        .unwrap()
        .draft
        .unwrap();
    assert_eq!(next.prompt, 1);
    assert_eq!(next.context, Some(second));
    assert!(matches!(
        storage.mutate_journal(&publication_id, JournalMutation::Publish { id: next.id }),
        Err(JournalError::Invalid)
    ));
    storage.begin_publication_deletion(&publication_id).unwrap();
    storage.maintain_deletions().unwrap();
    let count: u32 = storage
        .connection
        .query_row("SELECT count(*) FROM reflections", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn app_settings_preserve_book_layouts_and_survive_restart() {
    use super::ReaderPreferences;
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"settings archive").unwrap();
    let (facts, responses, _) = snapshot_values();
    let id = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"4".repeat(64),
            "Settings book",
            &facts,
            &responses,
        )
        .unwrap()
        .publication
        .publication_id;
    let mut defaults = ReaderPreferences {
        presentation_mode: "document-scroll".into(),
        font_scale: "125".into(),
        ..Default::default()
    };
    assert!(
        storage
            .save_app_settings(0, &defaults, keeps_volume())
            .unwrap()
    );
    assert!(
        !storage
            .save_app_settings(0, &defaults, keeps_volume())
            .unwrap()
    );
    let inherited = storage.load_publication_preferences(&id).unwrap();
    assert_eq!(inherited.preferences, defaults);
    let mut book = inherited.preferences;
    book.presentation_mode = "paginated".into();
    book.font_scale = "140".into();
    storage
        .compare_and_swap_preferences(&id, inherited.revisions.global, 0, &book)
        .unwrap();
    let settings = storage.load_app_settings(false).unwrap();
    assert_eq!(settings.preferences.presentation_mode, "document-scroll");
    defaults.font_scale = "160".into();
    storage
        .save_app_settings(settings.revision, &defaults, keeps_volume())
        .unwrap();
    drop(storage);
    let storage = Storage::open(directory.path()).unwrap();
    let book = storage.load_publication_preferences(&id).unwrap();
    assert_eq!(book.preferences.presentation_mode, "paginated");
    assert_eq!(book.preferences.font_scale, "160");
}

#[test]
fn a_cleared_book_layout_follows_the_application_defaults_again() {
    use super::ReaderPreferences;
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"layout archive").unwrap();
    let (facts, responses, _) = snapshot_values();
    let id = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"5".repeat(64),
            "Layout book",
            &facts,
            &responses,
        )
        .unwrap()
        .publication
        .publication_id;
    let defaults = ReaderPreferences {
        presentation_mode: "document-scroll".into(),
        ..Default::default()
    };
    assert!(
        storage
            .save_app_settings(0, &defaults, keeps_volume())
            .unwrap()
    );
    let inherited = storage.load_publication_preferences(&id).unwrap();
    let mut book = inherited.preferences.clone();
    book.presentation_mode = "paginated".into();
    storage
        .compare_and_swap_preferences(&id, inherited.revisions.global, 0, &book)
        .unwrap();
    assert_eq!(
        storage
            .load_publication_preferences(&id)
            .unwrap()
            .preferences
            .presentation_mode,
        "paginated"
    );

    storage.clear_publication_preferences(&id).unwrap();

    let followed = storage.load_publication_preferences(&id).unwrap();
    assert_eq!(followed.preferences.presentation_mode, "document-scroll");
    assert_eq!(followed.revisions.publication, 0);
    // A second clear is not an error: the book already follows the defaults.
    storage.clear_publication_preferences(&id).unwrap();
    assert!(
        storage
            .clear_publication_preferences(&"0".repeat(32))
            .is_err()
    );
}

#[test]
fn a_reset_restores_default_reading_preferences_and_keeps_the_device_answer() {
    use super::ReaderPreferences;
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    let chosen = ReaderPreferences {
        font_scale: "200".into(),
        presentation_mode: "document-scroll".into(),
        keyboard_mode: "vim".into(),
        click_zones: true,
        ..Default::default()
    };
    let turns_pages = crate::storage::DeviceSettings {
        side_buttons_turn_pages: true,
    };
    assert!(storage.save_app_settings(0, &chosen, turns_pages).unwrap());
    let saved = storage.load_app_settings(false).unwrap();
    assert_eq!(saved.preferences.font_scale, "200");
    assert!(saved.preferences.click_zones);

    assert!(!storage.reset_app_reading_preferences(0).unwrap());
    assert!(
        storage
            .reset_app_reading_preferences(saved.revision)
            .unwrap()
    );

    let reset = storage.load_app_settings(false).unwrap();
    assert_eq!(reset.preferences, ReaderPreferences::default());
    // The shipped text size and margins, pinned here because they are product choices rather
    // than neutral ones.
    assert_eq!(reset.preferences.font_scale, "125");
    assert_eq!(reset.preferences.measure, "wide");
    // The device answer describes the hardware, so a reading reset leaves it alone.
    assert!(reset.device.side_buttons_turn_pages);
    assert!(reset.revision > saved.revision);
}

/// A device setting these tests do not exercise, spelled once.
fn keeps_volume() -> crate::storage::DeviceSettings {
    crate::storage::DeviceSettings {
        side_buttons_turn_pages: false,
    }
}

#[test]
fn book_description_is_read_from_persisted_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"description archive").unwrap();
    let (mut facts, responses, _) = snapshot_values();
    facts["package"]["metadata"]["description"] = serde_json::json!([
        { "content": " " }, { "content": "<p>A book&#8217;s description.</p>" }
    ]);
    let id = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            &"4".repeat(64),
            "Described book",
            &facts,
            &responses,
        )
        .unwrap()
        .publication
        .publication_id;
    drop(storage);
    let storage = Storage::open(directory.path()).unwrap();
    assert_eq!(
        storage.load_publication_description(&id).unwrap(),
        Some(Some("<p>A book&#8217;s description.</p>".into()))
    );
    assert_eq!(
        storage
            .load_publication_description(&"0".repeat(32))
            .unwrap(),
        None
    );
    storage.connection.execute("UPDATE publication_snapshots SET facts_json = json_remove(facts_json, '$.package.metadata.description') WHERE publication_id = ?1", [&id]).unwrap();
    assert_eq!(
        storage.load_publication_description(&id).unwrap(),
        Some(None)
    );
}

#[test]
fn roll_reading_positions_round_trip_storage_and_validate_on_decode() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"roll position archive");
    let ordinary = serde_json::json!({
        "targetJson": "{\"source\":\"EPUB/chapter.xhtml\"}",
        "occurrenceHint": { "occurrence": 0, "resource": "EPUB/chapter.xhtml" }
    });
    for (revision, progression) in [None, Some(0.0), Some(0.375), Some(1.0)]
        .into_iter()
        .enumerate()
    {
        let mut position = ordinary.clone();
        if let Some(progression) = progression {
            position["rollProgression"] = serde_json::json!(progression);
        }
        {
            let mut storage = Storage::open(directory.path()).unwrap();
            assert!(matches!(
                storage
                    .compare_and_swap_reading_position(
                        &publication_id,
                        revision as u32,
                        &position.to_string()
                    )
                    .unwrap(),
                SaveReadingPositionOutcome::Saved { .. }
            ));
        }
        let storage = Storage::open(directory.path()).unwrap();
        let stored = storage
            .load_reading_position(&publication_id)
            .unwrap()
            .unwrap();
        let (decoded, stored_revision) = super::decode_reading_position(
            Some(stored.position_json),
            Some(i64::from(stored.revision)),
        )
        .unwrap();
        assert_eq!(stored_revision, revision as u32 + 1);
        assert_eq!(serde_json::to_value(decoded.unwrap()).unwrap(), position);
    }
    for invalid in [
        serde_json::json!(-0.001),
        serde_json::json!(1.001),
        serde_json::json!("0.5"),
    ] {
        let mut position = ordinary.clone();
        position["rollProgression"] = invalid;
        assert!(super::decode_reading_position(Some(position.to_string()), Some(1)).is_err());
    }
    let mut unknown = ordinary.clone();
    unknown["extra"] = serde_json::json!(true);
    assert!(super::decode_reading_position(Some(unknown.to_string()), Some(1)).is_err());
    for progression in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut position: super::ReadingPosition =
            serde_json::from_value(ordinary.clone()).unwrap();
        position.roll_progression = Some(progression);
        assert!(super::validate_reading_position(&position).is_err());
    }
}

#[test]
fn one_annotation_mutation_writes_one_row_whatever_the_reader_already_owns() {
    fn highlight(
        storage: &mut Storage,
        publication_id: &str,
        revision: u32,
    ) -> SaveAnnotationOutcome {
        save_annotation(
            storage,
            publication_id,
            serde_json::json!({
                "type": "create-highlight", "requestId": format!("request-{revision}"),
                "basedOnRevision": revision, "selectionRevision": 1, "occurrence": 0,
                "targetJson": "{}", "excerpt": "selected", "sectionLabel": "One"
            }),
            &[Some("EPUB/chapter.xhtml".to_owned())],
        )
    }

    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"annotation cost");
    let mut storage = Storage::open(directory.path()).unwrap();
    for revision in 0..8 {
        assert!(matches!(
            highlight(&mut storage, &publication_id, revision),
            SaveAnnotationOutcome::Saved { .. }
        ));
    }

    // The ninth highlight must cost the same as the first: one annotation row plus
    // the projection revision. A JSON projection would rewrite all nine.
    let before = storage.connection.total_changes();
    assert!(matches!(
        highlight(&mut storage, &publication_id, 8),
        SaveAnnotationOutcome::Saved { .. }
    ));
    assert_eq!(storage.connection.total_changes() - before, 2);
    assert_eq!(
        storage
            .load_annotations(&publication_id)
            .unwrap()
            .records
            .len(),
        9
    );
}

#[test]
fn annotation_mutations_cover_the_complete_contract_and_persist_empty_revision() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"annotation archive");
    let resources = [Some("EPUB/chapter.xhtml".to_owned())];
    let mut storage = Storage::open(directory.path()).unwrap();
    assert_eq!(
        storage.load_annotations(&publication_id).unwrap().revision,
        0
    );

    let highlight = save_annotation(
        &mut storage,
        &publication_id,
        serde_json::json!({
            "type": "create-highlight", "requestId": "request-1",
            "basedOnRevision": 0, "selectionRevision": 4, "occurrence": 0,
            "targetJson": "{}", "excerpt": "selected", "sectionLabel": "One"
        }),
        &resources,
    );
    let highlight_id = saved_record_id(highlight);
    assert_eq!(highlight_id.len(), 36);
    assert_eq!(&highlight_id[14..15], "4");
    let created = storage.load_annotations(&publication_id).unwrap().records[0]
        .created()
        .to_owned();
    assert!(super::parse_annotation_timestamp(&created).is_some());
    let converted = save_annotation(
        &mut storage,
        &publication_id,
        serde_json::json!({
            "type": "convert-highlight-to-note", "requestId": "request-2",
            "basedOnRevision": 1, "recordId": highlight_id, "text": "note"
        }),
        &resources,
    );
    assert!(matches!(converted, SaveAnnotationOutcome::Saved { .. }));
    assert!(matches!(
        save_annotation(
            &mut storage,
            &publication_id,
            serde_json::json!({
                "type": "update-note", "requestId": "request-3",
                "basedOnRevision": 2, "recordId": highlight_id, "text": "updated"
            }),
            &resources,
        ),
        SaveAnnotationOutcome::Saved { .. }
    ));
    assert!(matches!(
        save_annotation(
            &mut storage,
            &publication_id,
            serde_json::json!({
                "type": "retarget-annotation", "requestId": "request-4",
                "basedOnRevision": 3, "recordId": highlight_id,
                "targetJson": "{\"changed\":true}", "excerpt": "changed"
            }),
            &resources,
        ),
        SaveAnnotationOutcome::Saved { .. }
    ));
    assert!(matches!(
        save_annotation(
            &mut storage,
            &publication_id,
            serde_json::json!({
                "type": "delete-annotation", "requestId": "request-5",
                "basedOnRevision": 4, "recordId": highlight_id
            }),
            &resources,
        ),
        SaveAnnotationOutcome::Saved { .. }
    ));
    let bookmark = save_annotation(
        &mut storage,
        &publication_id,
        serde_json::json!({
            "type": "create-bookmark", "requestId": "request-6",
            "basedOnRevision": 5, "locationRevision": 8, "occurrence": 0,
            "targetJson": "{}", "sectionLabel": null, "label": "mark"
        }),
        &resources,
    );
    let bookmark_id = saved_record_id(bookmark);
    assert!(matches!(
        save_annotation(
            &mut storage,
            &publication_id,
            serde_json::json!({
                "type": "delete-annotation", "requestId": "request-7",
                "basedOnRevision": 6, "recordId": bookmark_id
            }),
            &resources,
        ),
        SaveAnnotationOutcome::Saved { .. }
    ));
    assert!(matches!(
        save_annotation(
            &mut storage,
            &publication_id,
            serde_json::json!({
                "type": "create-note", "requestId": "request-8",
                "basedOnRevision": 7, "selectionRevision": 9, "occurrence": 0,
                "targetJson": "{}", "excerpt": null, "sectionLabel": null,
                "text": "standalone"
            }),
            &resources,
        ),
        SaveAnnotationOutcome::Saved { .. }
    ));

    drop(storage);
    let storage = Storage::open(directory.path()).unwrap();
    let projection = storage.load_annotations(&publication_id).unwrap();
    assert_eq!(projection.revision, 8);
    assert_eq!(projection.records.len(), 1);
}

#[test]
fn annotation_conflict_and_bounds_do_not_change_projection() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"annotation bounds");
    let resources = [Some("EPUB/chapter.xhtml".to_owned())];
    let mut storage = Storage::open(directory.path()).unwrap();
    let first = save_annotation(
        &mut storage,
        &publication_id,
        serde_json::json!({
            "type": "create-highlight", "requestId": "first", "basedOnRevision": 0,
            "selectionRevision": 0, "occurrence": 0, "targetJson": "{}",
            "excerpt": null, "sectionLabel": null
        }),
        &resources,
    );
    let first_id = saved_record_id(first);
    assert!(matches!(
        save_annotation(
            &mut storage,
            &publication_id,
            serde_json::json!({
                "type": "create-highlight", "requestId": "stale", "basedOnRevision": 0,
                "selectionRevision": 0, "occurrence": 0, "targetJson": "{}",
                "excerpt": null, "sectionLabel": null
            }),
            &resources,
        ),
        SaveAnnotationOutcome::RevisionMismatch { .. }
    ));
    assert_eq!(
        storage.load_annotations(&publication_id).unwrap().revision,
        1
    );
    let oversized = "x".repeat(4_097);
    let invalid: AnnotationMutation = serde_json::from_value(serde_json::json!({
        "type": "convert-highlight-to-note", "requestId": "oversized",
        "basedOnRevision": 1, "recordId": first_id, "text": oversized
    }))
    .unwrap();
    assert!(
        storage
            .compare_and_swap_annotations(&publication_id, invalid, &resources)
            .is_err()
    );
    assert_eq!(
        storage.load_annotations(&publication_id).unwrap().revision,
        1
    );
    let missing_resources = [None];
    let invalid_target: AnnotationMutation = serde_json::from_value(serde_json::json!({
        "type": "create-bookmark", "requestId": "missing-resource",
        "basedOnRevision": 1, "locationRevision": 0, "occurrence": 0,
        "targetJson": "{}", "sectionLabel": null, "label": null
    }))
    .unwrap();
    assert!(
        storage
            .compare_and_swap_annotations(&publication_id, invalid_target, &missing_resources)
            .is_err()
    );
    assert_eq!(
        storage.load_annotations(&publication_id).unwrap().revision,
        1
    );
}

#[test]
fn final_annotation_delete_retains_an_empty_row_across_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"annotation delete");
    let resources = [Some("EPUB/chapter.xhtml".to_owned())];
    let record_id = {
        let mut storage = Storage::open(directory.path()).unwrap();
        saved_record_id(save_annotation(
            &mut storage,
            &publication_id,
            serde_json::json!({
                "type": "create-bookmark", "requestId": "create", "basedOnRevision": 0,
                "locationRevision": 0, "occurrence": 0, "targetJson": "{}",
                "sectionLabel": null, "label": null
            }),
            &resources,
        ))
    };
    let mut storage = Storage::open(directory.path()).unwrap();
    save_annotation(
        &mut storage,
        &publication_id,
        serde_json::json!({
            "type": "delete-annotation", "requestId": "delete",
            "basedOnRevision": 1, "recordId": record_id
        }),
        &resources,
    );
    drop(storage);
    let storage = Storage::open(directory.path()).unwrap();
    let projection = storage.load_annotations(&publication_id).unwrap();
    assert_eq!(projection.revision, 2);
    assert!(projection.records.is_empty());
    assert_eq!(
        storage
            .connection
            .query_row("SELECT count(*) FROM annotation_projections", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
}

#[test]
fn annotation_uses_the_effective_fallback_resource_hint() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"fallback annotation");
    let resources = [Some("EPUB/fallback.xhtml".to_owned())];
    let mut storage = Storage::open(directory.path()).unwrap();

    save_annotation(
        &mut storage,
        &publication_id,
        serde_json::json!({
            "type": "create-bookmark", "requestId": "fallback", "basedOnRevision": 0,
            "locationRevision": 0, "occurrence": 0, "targetJson": "{}",
            "sectionLabel": null, "label": null
        }),
        &resources,
    );

    let projection = storage.load_annotations(&publication_id).unwrap();
    assert_eq!(projection.records[0].resource_hint(), "EPUB/fallback.xhtml");
    projection.validate_resource_hints(&resources).unwrap();
    assert!(
        projection
            .validate_resource_hints(&[Some("EPUB/chapter.mp3".to_owned())])
            .is_err()
    );
}

#[test]
fn public_annotation_projection_omits_storage_resource_hints() {
    let projection = super::AnnotationProjection {
        revision: 7,
        records: vec![
            super::AnnotationRecord::Highlight {
                id: "highlight".to_owned(),
                target_json: "{}".to_owned(),
                occurrence: 0,
                excerpt: Some("excerpt".to_owned()),
                section_label: Some("Section".to_owned()),
                created: "2026-09-16T00:00:00Z".to_owned(),
                modified: "2026-09-16T00:00:00Z".to_owned(),
                creator: None,
                resource_hint: "EPUB/chapter.xhtml".to_owned(),
            },
            super::AnnotationRecord::Note {
                id: "note".to_owned(),
                target_json: "{}".to_owned(),
                occurrence: 0,
                excerpt: None,
                section_label: None,
                text: "Text".to_owned(),
                created: "2026-09-16T00:00:00Z".to_owned(),
                modified: "2026-09-16T00:00:00Z".to_owned(),
                creator: None,
                resource_hint: "EPUB/chapter.xhtml".to_owned(),
            },
            super::AnnotationRecord::Bookmark {
                id: "bookmark".to_owned(),
                target_json: "{}".to_owned(),
                occurrence: 0,
                section_label: None,
                label: Some("Label".to_owned()),
                created: "2026-09-16T00:00:00Z".to_owned(),
                modified: "2026-09-16T00:00:00Z".to_owned(),
                creator: None,
                resource_hint: "EPUB/chapter.xhtml".to_owned(),
            },
        ],
    };

    assert_eq!(
        serde_json::to_value(projection.public()).unwrap(),
        serde_json::json!({
            "revision": 7,
            "records": [
                {
                    "type": "highlight", "id": "highlight", "targetJson": "{}",
                    "occurrence": 0, "excerpt": "excerpt", "sectionLabel": "Section"
                },
                {
                    "type": "note", "id": "note", "targetJson": "{}",
                    "occurrence": 0, "excerpt": null, "sectionLabel": null, "text": "Text"
                },
                {
                    "type": "bookmark", "id": "bookmark", "targetJson": "{}",
                    "occurrence": 0, "sectionLabel": null, "label": "Label"
                }
            ]
        })
    );
}

#[test]
fn pagination_cache_persists_and_supports_record_and_basis_removal() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"pagination persistence");
    let (snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
    let basis = "a".repeat(64);
    let first_record = "b".repeat(64);
    let second_record = "c".repeat(64);
    {
        let mut storage = Storage::open(directory.path()).unwrap();
        assert_eq!(
            storage
                .write_pagination_record(
                    &publication_id,
                    &snapshot_id,
                    &basis,
                    &first_record,
                    b"opaque\0measurement",
                )
                .unwrap(),
            PaginationCacheMutationOutcome::Applied
        );
    }

    let mut storage = Storage::open(directory.path()).unwrap();
    assert_eq!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &basis, &first_record)
            .unwrap(),
        PaginationCacheReadOutcome::Found(b"opaque\0measurement".to_vec())
    );
    assert!(
        storage
            .write_pagination_record(
                &publication_id,
                &snapshot_id,
                &"A".repeat(64),
                &first_record,
                b"invalid",
            )
            .is_err()
    );
    assert!(
        storage
            .write_pagination_record(
                &publication_id,
                &snapshot_id,
                &basis,
                &second_record,
                &vec![0; super::MAX_PAGINATION_RECORD_BYTES + 1],
            )
            .is_err()
    );
    assert_eq!(
        storage
            .remove_pagination_record(&publication_id, &snapshot_id, &basis, &first_record,)
            .unwrap(),
        PaginationCacheMutationOutcome::Applied
    );
    assert_eq!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &basis, &first_record)
            .unwrap(),
        PaginationCacheReadOutcome::Missing
    );
    storage
        .write_pagination_record(
            &publication_id,
            &snapshot_id,
            &basis,
            &second_record,
            b"second",
        )
        .unwrap();
    storage
        .remove_pagination_basis(&publication_id, &snapshot_id, &basis)
        .unwrap();
    assert_eq!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &basis, &second_record)
            .unwrap(),
        PaginationCacheReadOutcome::Missing
    );
}

#[test]
fn pagination_cache_bulk_reads_are_ordered_equivalent_and_touch_once() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"pagination bulk read");
    let (snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
    let basis = "a".repeat(64);
    let first_record = "b".repeat(64);
    let missing_record = "c".repeat(64);
    let second_record = "d".repeat(64);
    let mut storage = Storage::open(directory.path()).unwrap();
    for (record, value) in [
        (&first_record, &b"first"[..]),
        (&second_record, &b"second"[..]),
    ] {
        storage
            .write_pagination_record(&publication_id, &snapshot_id, &basis, record, value)
            .unwrap();
    }
    storage
        .write_pagination_record(
            &publication_id,
            &snapshot_id,
            &"e".repeat(64),
            &first_record,
            b"newer",
        )
        .unwrap();
    let previous_access_order = storage
        .pagination
        .query_row(
            "SELECT MAX(access_order) FROM pagination_bases",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();

    assert_eq!(
        storage
            .read_pagination_records(
                &publication_id,
                &snapshot_id,
                &basis,
                &[first_record.clone(), missing_record, second_record.clone()],
            )
            .unwrap(),
        PaginationCacheRecordsReadOutcome::Records(vec![
            Some(b"first".to_vec()),
            None,
            Some(b"second".to_vec()),
        ])
    );
    assert_eq!(
        storage
            .pagination
            .query_row(
                "SELECT access_order FROM pagination_bases
                     WHERE snapshot_id = ?1 AND basis_hash = ?2",
                params![snapshot_id, basis],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        previous_access_order + 1
    );
    assert_eq!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &basis, &first_record)
            .unwrap(),
        PaginationCacheReadOutcome::Found(b"first".to_vec())
    );
    assert_eq!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &basis, &second_record)
            .unwrap(),
        PaginationCacheReadOutcome::Found(b"second".to_vec())
    );
}

#[test]
fn pagination_cache_bulk_mutations_are_atomic_and_touch_once() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"pagination bulk mutation");
    let (snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
    let basis = "a".repeat(64);
    let removed = "b".repeat(64);
    let first = "c".repeat(64);
    let second = "d".repeat(64);
    let mut storage = Storage::open(directory.path()).unwrap();
    storage
        .write_pagination_record(&publication_id, &snapshot_id, &basis, &removed, b"old")
        .unwrap();
    storage
        .write_pagination_record(
            &publication_id,
            &snapshot_id,
            &"e".repeat(64),
            &removed,
            b"newer",
        )
        .unwrap();
    let previous_access_order = storage
        .pagination
        .query_row(
            "SELECT MAX(access_order) FROM pagination_bases",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();

    assert_eq!(
        storage
            .mutate_pagination_records(
                &publication_id,
                &snapshot_id,
                &basis,
                &[
                    PaginationCacheMutation::Remove {
                        record_hash: removed.clone(),
                    },
                    PaginationCacheMutation::Write {
                        record_hash: first.clone(),
                        value: b"first".to_vec(),
                    },
                    PaginationCacheMutation::Write {
                        record_hash: second.clone(),
                        value: b"second".to_vec(),
                    },
                ],
            )
            .unwrap(),
        PaginationCacheMutationOutcome::Applied
    );
    assert_eq!(
        storage
            .read_pagination_records(
                &publication_id,
                &snapshot_id,
                &basis,
                &[removed, first, second],
            )
            .unwrap(),
        PaginationCacheRecordsReadOutcome::Records(vec![
            None,
            Some(b"first".to_vec()),
            Some(b"second".to_vec()),
        ])
    );
    assert_eq!(
        storage
            .pagination
            .query_row(
                "SELECT access_order FROM pagination_bases
                     WHERE snapshot_id = ?1 AND basis_hash = ?2",
                params![snapshot_id, basis],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        previous_access_order + 1
    );
}

#[test]
fn pagination_cache_bulk_mutation_rejects_atomically_and_preserves_unavailable_data() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"pagination bulk safety");
    let (snapshot_id, blob_path) = publication_snapshot(directory.path(), &publication_id);
    let basis = "a".repeat(64);
    let value = vec![7; super::MAX_PAGINATION_RECORD_BYTES];
    let mut storage = Storage::open(directory.path()).unwrap();
    for index in 0..64 {
        storage
            .write_pagination_record(
                &publication_id,
                &snapshot_id,
                &basis,
                &format!("{index:064x}"),
                &value,
            )
            .unwrap();
    }
    let removed = format!("{:064x}", 0);
    let first_new = format!("{:064x}", 64);
    let second_new = format!("{:064x}", 65);
    assert_eq!(
        storage
            .mutate_pagination_records(
                &publication_id,
                &snapshot_id,
                &basis,
                &[
                    PaginationCacheMutation::Remove {
                        record_hash: removed.clone(),
                    },
                    PaginationCacheMutation::Write {
                        record_hash: first_new.clone(),
                        value: value.clone(),
                    },
                    PaginationCacheMutation::Write {
                        record_hash: second_new.clone(),
                        value,
                    },
                ],
            )
            .unwrap(),
        PaginationCacheMutationOutcome::SnapshotLimitExceeded
    );
    assert!(matches!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &basis, &removed)
            .unwrap(),
        PaginationCacheReadOutcome::Found(_)
    ));
    for record in [&first_new, &second_new] {
        assert_eq!(
            storage
                .read_pagination_record(&publication_id, &snapshot_id, &basis, record)
                .unwrap(),
            PaginationCacheReadOutcome::Missing
        );
    }

    let duplicate = PaginationCacheMutation::Remove {
        record_hash: removed.clone(),
    };
    assert!(
        storage
            .mutate_pagination_records(
                &publication_id,
                &snapshot_id,
                &basis,
                &[
                    duplicate,
                    PaginationCacheMutation::Write {
                        record_hash: removed.clone(),
                        value: b"duplicate".to_vec(),
                    },
                ],
            )
            .is_err()
    );
    assert!(
        storage
            .mutate_pagination_records(
                &publication_id,
                &snapshot_id,
                &basis,
                &[PaginationCacheMutation::Write {
                    record_hash: "f".repeat(64),
                    value: vec![0; super::MAX_PAGINATION_RECORD_BYTES + 1],
                }],
            )
            .is_err()
    );

    std::fs::remove_file(blob_path).unwrap();
    assert_eq!(
        storage
            .mutate_pagination_records(
                &publication_id,
                &snapshot_id,
                &basis,
                &[PaginationCacheMutation::Remove {
                    record_hash: removed,
                }],
            )
            .unwrap(),
        PaginationCacheMutationOutcome::Unavailable
    );
    assert_eq!(
        storage
            .pagination
            .query_row("SELECT COUNT(*) FROM pagination_records", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        64
    );
}

#[test]
fn pagination_cache_evicts_whole_least_recently_used_bases() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"pagination lru");
    let (snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
    let mut storage = Storage::open(directory.path()).unwrap();
    let record = "f".repeat(64);
    for digit in ['1', '2', '3'] {
        storage
            .write_pagination_record(
                &publication_id,
                &snapshot_id,
                &digit.to_string().repeat(64),
                &record,
                digit.to_string().as_bytes(),
            )
            .unwrap();
    }
    assert!(matches!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &"1".repeat(64), &record,)
            .unwrap(),
        PaginationCacheReadOutcome::Found(_)
    ));
    storage
        .write_pagination_record(
            &publication_id,
            &snapshot_id,
            &"4".repeat(64),
            &record,
            b"four",
        )
        .unwrap();

    assert_eq!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &"2".repeat(64), &record,)
            .unwrap(),
        PaginationCacheReadOutcome::Missing
    );
    assert!(matches!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &"1".repeat(64), &record,)
            .unwrap(),
        PaginationCacheReadOutcome::Found(_)
    ));
    assert_eq!(
        storage
            .pagination
            .query_row(
                "SELECT COUNT(*) FROM pagination_bases WHERE snapshot_id = ?1",
                [&snapshot_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        3
    );
}

#[test]
fn pagination_cache_does_not_retouch_the_most_recent_basis() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"pagination read access");
    let (snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
    let basis = "a".repeat(64);
    let first_record = "b".repeat(64);
    let second_record = "c".repeat(64);
    let mut storage = Storage::open(directory.path()).unwrap();
    for record in [&first_record, &second_record] {
        storage
            .write_pagination_record(
                &publication_id,
                &snapshot_id,
                &basis,
                record,
                b"measurement",
            )
            .unwrap();
    }
    let access_order = storage
        .pagination
        .query_row(
            "SELECT access_order FROM pagination_bases
                 WHERE snapshot_id = ?1 AND basis_hash = ?2",
            params![snapshot_id, basis],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();

    for record in [&first_record, &second_record] {
        assert!(matches!(
            storage
                .read_pagination_record(&publication_id, &snapshot_id, &basis, record)
                .unwrap(),
            PaginationCacheReadOutcome::Found(_)
        ));
    }

    assert_eq!(
        storage
            .pagination
            .query_row(
                "SELECT access_order FROM pagination_bases
                     WHERE snapshot_id = ?1 AND basis_hash = ?2",
                params![snapshot_id, basis],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        access_order
    );
}

#[test]
fn pagination_cache_enforces_record_and_snapshot_byte_limits() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"pagination limits");
    let (snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
    let basis = "a".repeat(64);
    let value = vec![7; super::MAX_PAGINATION_RECORD_BYTES];
    let mut storage = Storage::open(directory.path()).unwrap();
    for index in 0..64 {
        storage
            .write_pagination_record(
                &publication_id,
                &snapshot_id,
                &basis,
                &format!("{index:064x}"),
                &value,
            )
            .unwrap();
    }
    assert_eq!(
        storage
            .write_pagination_record(
                &publication_id,
                &snapshot_id,
                &basis,
                &format!("{:064x}", 64),
                &value,
            )
            .unwrap(),
        PaginationCacheMutationOutcome::SnapshotLimitExceeded
    );
    assert_eq!(
        storage
            .pagination
            .query_row(
                "SELECT SUM(length(value)) FROM pagination_records WHERE snapshot_id = ?1",
                [&snapshot_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        super::MAX_PAGINATION_SNAPSHOT_BYTES
    );

    let replacement_basis = "d".repeat(64);
    storage
        .write_pagination_record(
            &publication_id,
            &snapshot_id,
            &replacement_basis,
            &"e".repeat(64),
            b"replacement",
        )
        .unwrap();
    assert_eq!(
        storage
            .pagination
            .query_row(
                "SELECT COUNT(*) FROM pagination_bases WHERE snapshot_id = ?1",
                [&snapshot_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        storage
            .read_pagination_record(
                &publication_id,
                &snapshot_id,
                &replacement_basis,
                &"e".repeat(64),
            )
            .unwrap(),
        PaginationCacheReadOutcome::Found(b"replacement".to_vec())
    );
}

#[test]
fn pagination_cache_enforces_the_global_limit_by_whole_basis() {
    let directory = tempfile::tempdir().unwrap();
    let mut publications = Vec::new();
    for index in 0..16 {
        let publication_id =
            create_test_publication(directory.path(), format!("global {index}").as_bytes());
        let (snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
        publications.push((publication_id, snapshot_id));
    }
    let target_publication = create_test_publication(directory.path(), b"global eviction target");
    let (target_snapshot, _) = publication_snapshot(directory.path(), &target_publication);
    let mut storage = Storage::open(directory.path()).unwrap();
    let transaction = storage.pagination.transaction().unwrap();
    for (basis_index, (_, snapshot_id)) in publications.iter().enumerate() {
        let basis_hash = format!("{:064x}", basis_index + 1);
        transaction
            .execute(
                "INSERT INTO pagination_bases (snapshot_id, basis_hash, access_order)
                     VALUES (?1, ?2, ?3)",
                params![snapshot_id, basis_hash, basis_index + 1],
            )
            .unwrap();
        for record_index in 0..64 {
            transaction
                .execute(
                    "INSERT INTO pagination_records
                            (snapshot_id, basis_hash, record_hash, value)
                         VALUES (?1, ?2, ?3, zeroblob(65536))",
                    params![snapshot_id, basis_hash, format!("{record_index:064x}")],
                )
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    assert_eq!(
        storage
            .pagination
            .query_row(
                "SELECT SUM(length(value)) FROM pagination_records",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        super::MAX_PAGINATION_GLOBAL_BYTES
    );

    let target_basis = "f".repeat(64);
    let target_record = "e".repeat(64);
    storage
        .write_pagination_record(
            &target_publication,
            &target_snapshot,
            &target_basis,
            &target_record,
            b"target",
        )
        .unwrap();
    let (oldest_publication, oldest_snapshot) = &publications[0];
    assert_eq!(
        storage
            .read_pagination_record(
                oldest_publication,
                oldest_snapshot,
                &format!("{:064x}", 1),
                &format!("{:064x}", 0),
            )
            .unwrap(),
        PaginationCacheReadOutcome::Missing
    );
    assert!(
        storage
            .pagination
            .query_row(
                "SELECT SUM(length(value)) <= ?1 FROM pagination_records",
                [super::MAX_PAGINATION_GLOBAL_BYTES],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
    assert_eq!(
        storage
            .read_pagination_record(
                &target_publication,
                &target_snapshot,
                &target_basis,
                &target_record,
            )
            .unwrap(),
        PaginationCacheReadOutcome::Found(b"target".to_vec())
    );
}

#[test]
fn pagination_cache_unavailable_operations_do_not_mutate_records() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"pagination safety");
    let other_id = create_test_publication(directory.path(), b"pagination isolation");
    let (snapshot_id, blob_path) = publication_snapshot(directory.path(), &publication_id);
    let (other_snapshot_id, _) = publication_snapshot(directory.path(), &other_id);
    let basis = "a".repeat(64);
    let record = "b".repeat(64);
    let mut storage = Storage::open(directory.path()).unwrap();
    storage
        .write_pagination_record(&publication_id, &snapshot_id, &basis, &record, b"preserve")
        .unwrap();

    assert_eq!(
        storage
            .write_pagination_record(
                &publication_id,
                &other_snapshot_id,
                &basis,
                &record,
                b"isolated",
            )
            .unwrap(),
        PaginationCacheMutationOutcome::Unavailable
    );
    storage
        .connection
        .execute(
            "UPDATE publications SET current_snapshot_id = NULL WHERE publication_id = ?1",
            [&publication_id],
        )
        .unwrap();
    assert_eq!(
        storage
            .remove_pagination_basis(&publication_id, &snapshot_id, &basis)
            .unwrap(),
        PaginationCacheMutationOutcome::Unavailable
    );
    storage
        .connection
        .execute(
            "UPDATE publications SET current_snapshot_id = ?2 WHERE publication_id = ?1",
            params![publication_id, snapshot_id],
        )
        .unwrap();
    storage
        .connection
        .execute(
            "UPDATE publication_snapshots SET revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE snapshot_id = ?1",
            [&snapshot_id],
        )
        .unwrap();
    assert_eq!(
        storage
            .remove_pagination_record(&publication_id, &snapshot_id, &basis, &record)
            .unwrap(),
        PaginationCacheMutationOutcome::Unavailable
    );
    storage
        .connection
        .execute(
            "UPDATE publication_snapshots SET revoked_at = NULL WHERE snapshot_id = ?1",
            [&snapshot_id],
        )
        .unwrap();
    storage
        .connection
        .execute(
            "UPDATE publications SET deletion_state = 'deleting' WHERE publication_id = ?1",
            [&publication_id],
        )
        .unwrap();
    assert_eq!(
        storage
            .read_pagination_record(&publication_id, &snapshot_id, &basis, &record)
            .unwrap(),
        PaginationCacheReadOutcome::Unavailable
    );
    assert_eq!(
        storage
            .read_pagination_records(
                &publication_id,
                &snapshot_id,
                &basis,
                std::slice::from_ref(&record),
            )
            .unwrap(),
        PaginationCacheRecordsReadOutcome::Unavailable
    );
    storage
        .connection
        .execute(
            "UPDATE publications SET deletion_state = 'live' WHERE publication_id = ?1",
            [&publication_id],
        )
        .unwrap();
    std::fs::remove_file(blob_path).unwrap();
    assert_eq!(
        storage
            .remove_pagination_basis(&publication_id, &snapshot_id, &basis)
            .unwrap(),
        PaginationCacheMutationOutcome::Unavailable
    );
    assert_eq!(
        storage
            .pagination
            .query_row("SELECT COUNT(*) FROM pagination_records", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
}

#[test]
fn a_snapshot_that_goes_away_loses_its_cached_pages() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"pagination cascade");
    let (snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
    let mut storage = Storage::open(directory.path()).unwrap();
    storage
        .write_pagination_record(
            &publication_id,
            &snapshot_id,
            &"a".repeat(64),
            &"b".repeat(64),
            b"value",
        )
        .unwrap();
    storage
        .connection
        .execute(
            "UPDATE publications SET current_snapshot_id = NULL WHERE publication_id = ?1",
            [&publication_id],
        )
        .unwrap();
    storage
        .connection
        .execute(
            "DELETE FROM publication_snapshots WHERE snapshot_id = ?1",
            [&snapshot_id],
        )
        .unwrap();

    // The cache is its own file, so nothing cascades. Until it is collected the
    // orphan is unreachable, because every read authorizes its snapshot first.
    assert!(matches!(
        storage
            .read_pagination_record(
                &publication_id,
                &snapshot_id,
                &"a".repeat(64),
                &"b".repeat(64),
            )
            .unwrap(),
        PaginationCacheReadOutcome::Unavailable
    ));

    storage.retain_pagination_snapshots().unwrap();
    assert_eq!(
        storage
            .pagination
            .query_row("SELECT COUNT(*) FROM pagination_records", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn reader_preferences_are_revisioned_no_op_and_scoped() {
    let directory = tempfile::tempdir().unwrap();
    let first_id = create_test_publication(directory.path(), b"first preference publication");
    let second_id = create_test_publication(directory.path(), b"second preference publication");
    let mut storage = Storage::open(directory.path()).unwrap();
    let defaults = storage.load_publication_preferences(&first_id).unwrap();
    assert_eq!(defaults.revisions.global, 0);
    assert_eq!(defaults.revisions.publication, 0);

    let mut changed = defaults.preferences;
    changed.color_scheme = "dark".to_owned();
    changed.presentation_mode = "document-scroll".to_owned();
    let saved = match storage
        .compare_and_swap_preferences(&first_id, 0, 0, &changed)
        .unwrap()
    {
        SavePreferencesOutcome::Saved(saved) => saved,
        _ => panic!("initial preference update must save"),
    };
    assert_eq!(saved.revisions.global, 1);
    assert_eq!(saved.revisions.publication, 1);

    let unchanged = match storage
        .compare_and_swap_preferences(&first_id, 1, 1, &changed)
        .unwrap()
    {
        SavePreferencesOutcome::Saved(saved) => saved,
        _ => panic!("identical preference update must be a successful no-op"),
    };
    assert_eq!(unchanged.revisions, saved.revisions);

    let second = storage.load_publication_preferences(&second_id).unwrap();
    assert_eq!(second.preferences.color_scheme, "dark");
    assert_eq!(second.preferences.presentation_mode, "auto");
    assert_eq!(second.revisions.global, 1);
    assert_eq!(second.revisions.publication, 0);
    assert!(matches!(
        storage
            .compare_and_swap_preferences(&first_id, 0, 1, &changed)
            .unwrap(),
        SavePreferencesOutcome::RevisionMismatch(current)
            if current.revisions == saved.revisions
    ));
    drop(storage);

    let storage = Storage::open(directory.path()).unwrap();
    assert_eq!(
        storage.load_publication_preferences(&first_id).unwrap(),
        saved
    );
}

#[test]
fn malformed_stored_reader_preferences_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"malformed preferences");
    let storage = Storage::open(directory.path()).unwrap();
    storage
        .connection
        .execute(
            "INSERT INTO publication_reader_preferences
                    (publication_id, preferences_json, preferences_revision)
                 VALUES (?1, '{\"version\":1}', 1)",
            [&publication_id],
        )
        .unwrap();
    assert!(
        storage
            .load_publication_preferences(&publication_id)
            .is_err()
    );
}

#[test]
fn narration_preferences_persist_and_default_for_older_global_rows() {
    use super::{GlobalReaderPreferences, ReaderPreferences};

    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"narration preferences");
    let mut storage = Storage::open(directory.path()).unwrap();
    let defaults = storage
        .load_publication_preferences(&publication_id)
        .unwrap();
    assert_eq!(defaults.preferences.narration_rate, "1");
    assert!(defaults.preferences.narration_skipped_types.is_empty());

    let mut changed: ReaderPreferences = serde_json::from_value(serde_json::json!({
        "colorScheme": "system",
        "presentationMode": "paginated",
        "paginationColumns": "auto",
        "publicationTheme": "original",
        "publicationTypeface": "reader-serif",
        "fontScale": "100",
        "lineHeight": "original",
        "letterSpacing": "0.06",
        "wordSpacing": "0.08",
        "paragraphSpacing": "1",
        "textIndent": "0",
        "measure": "full-width",
        "alignment": "original",
        "hyphenation": "original",
        "keyboardMode": "standard",
        "touchZones": true,
        "narrationRate": "1.25",
        "narrationSkippedTypes": ["pagebreak"]
    }))
    .unwrap();
    assert!(matches!(
        storage
            .compare_and_swap_preferences(&publication_id, 0, 0, &changed)
            .unwrap(),
        SavePreferencesOutcome::Saved(_)
    ));
    let serialized = serde_json::to_value(
        storage
            .load_publication_preferences(&publication_id)
            .unwrap()
            .preferences,
    )
    .unwrap();
    assert_eq!(serialized["narrationRate"], serde_json::json!("1.25"));
    assert_eq!(
        serialized["narrationSkippedTypes"],
        serde_json::json!(["pagebreak"])
    );

    // Presentation follows the kernel, which admits the publication's own flow.
    changed.presentation_mode = "auto".to_owned();
    changed.validate().unwrap();
    changed.presentation_mode = "scrolled-doc".to_owned();
    assert!(changed.validate().is_err());
    changed.presentation_mode = "document-scroll".to_owned();

    changed.narration_rate = "1.1".to_owned();
    assert!(changed.validate().is_err());
    changed.narration_rate = "1.25".to_owned();
    changed.narration_skipped_types = vec!["pagebreak".to_owned(), "pagebreak".to_owned()];
    assert!(changed.validate().is_err());
    changed.narration_skipped_types = vec!["two words".to_owned()];
    assert!(changed.validate().is_err());

    let older_global: GlobalReaderPreferences = serde_json::from_value(serde_json::json!({
        "version": 1,
        "colorScheme": "dark",
        "publicationTheme": "original",
        "publicationTypeface": "reader-serif",
        "fontScale": "original",
        "lineHeight": "original",
        "measure": "full-width",
        "alignment": "original",
        "hyphenation": "original",
        "keyboardMode": "standard",
        "touchZones": true
    }))
    .unwrap();
    assert_eq!(older_global.narration_rate, "1");
    assert!(!older_global.click_zones);
    assert!(older_global.narration_skipped_types.is_empty());
    // A row written before the spacing preferences existed reads as no imposed spacing and
    // the authored paragraph shape, not as an invalid row.
    assert_eq!(older_global.letter_spacing, "0");
    assert_eq!(older_global.word_spacing, "0");
    assert_eq!(older_global.paragraph_spacing, "original");
    assert_eq!(older_global.text_indent, "original");
    // The removed authored-root text size becomes the percentage it already rendered as.
    let mut migrated = older_global;
    migrated.migrate();
    assert_eq!(migrated.font_scale, "100");
    let composed = ReaderPreferences {
        font_scale: "original".to_owned(),
        ..Default::default()
    };
    assert!(composed.validate().is_err());
    // Spacing is bounded to the values the panel offers, which reach the WCAG 1.4.12 overrides.
    for rejected in [
        ReaderPreferences {
            letter_spacing: "0.5".to_owned(),
            ..Default::default()
        },
        ReaderPreferences {
            word_spacing: "0.2".to_owned(),
            ..Default::default()
        },
        ReaderPreferences {
            paragraph_spacing: "3".to_owned(),
            ..Default::default()
        },
        ReaderPreferences {
            text_indent: "1.5".to_owned(),
            ..Default::default()
        },
    ] {
        assert!(rejected.validate().is_err());
    }
    for accepted in ["0", "0.03", "0.06", "0.12"] {
        assert!(
            ReaderPreferences {
                letter_spacing: accepted.to_owned(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
    // Every face and margin the panel offers must survive the validator: a value the reader can
    // choose but the host refuses is reported to the reader as a failed save.
    for typeface in ["original", "reader-serif", "reader-sans"] {
        assert!(
            ReaderPreferences {
                publication_typeface: typeface.to_owned(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
    for measure in ["narrow", "medium", "wide", "full-width"] {
        assert!(
            ReaderPreferences {
                measure: measure.to_owned(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
}

#[test]
fn publication_policy_defaults_are_ask_with_zero_revisions() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(directory.path()).unwrap();
    let global = storage.load_global_publication_policy().unwrap();
    assert_eq!(
        global.policy.global.active_content,
        ActiveContentDefault::Ask
    );
    assert_eq!(
        global.policy.global.remote_resources,
        RemoteResourcesDefault::Ask
    );
    assert_eq!(
        global.policy.publication,
        PublicationPolicyOverrides::default()
    );
    assert_eq!(global.revisions.global, 0);
    assert_eq!(global.revisions.publication, 0);
    assert_eq!(
        global.effective.active_content,
        EffectiveActiveContentPolicy::Ask
    );
    assert_eq!(
        global.effective.remote_resources,
        EffectiveRemoteResourcesPolicy::Ask
    );
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM publication_policy_defaults",
                [],
                |row| { row.get::<_, u32>(0) }
            )
            .unwrap(),
        0
    );
}

#[test]
fn annotation_projections_have_no_record_count_or_aggregate_size_limit() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"many annotations");
    let resources = [Some("EPUB/chapter.xhtml".to_owned())];
    let mut storage = Storage::open(directory.path()).unwrap();
    let target = format!(
        r#"{{"source":"chapter.xhtml","padding":"{}"}}"#,
        "x".repeat(4_096)
    );
    let records = (0..1_200)
        .map(|index| super::AnnotationRecord::Note {
            id: format!("note-{index}"),
            target_json: target.clone(),
            occurrence: 0,
            excerpt: None,
            section_label: None,
            text: "n".repeat(1_024),
            created: "2026-01-01T00:00:00Z".to_owned(),
            modified: "2026-01-01T00:00:00Z".to_owned(),
            creator: None,
            resource_hint: "EPUB/chapter.xhtml".to_owned(),
        })
        .collect::<Vec<_>>();
    let (projection, ()) = storage
        .update_annotation_records(&publication_id, &resources, |_| (Some(records), ()))
        .unwrap();
    assert_eq!(projection.revision, 1_200);
    drop(storage);

    let storage = Storage::open(directory.path()).unwrap();
    assert_eq!(
        storage
            .load_annotations(&publication_id)
            .unwrap()
            .records
            .len(),
        1_200
    );
}

#[test]
fn publication_policy_cas_is_revisioned_no_op_scoped_and_persistent() {
    let directory = tempfile::tempdir().unwrap();
    let first_id = create_test_publication(directory.path(), b"first policy publication");
    let second_id = create_test_publication(directory.path(), b"second policy publication");
    let mut storage = Storage::open(directory.path()).unwrap();
    let policy = PublicationPolicy {
        global: super::GlobalPublicationPolicy {
            active_content: ActiveContentDefault::Automatic,
            remote_resources: RemoteResourcesDefault::Blocked,
        },
        publication: PublicationPolicyOverrides {
            active_content: Some(ActiveContentOverride::Disabled),
            remote_resources: Some(RemoteResourcesOverride::Allowed),
        },
    };
    let global_saved = match storage
        .compare_and_swap_global_publication_policy(0, policy.global)
        .unwrap()
    {
        SavePublicationPolicyOutcome::Saved(saved) => saved,
        _ => panic!("initial global publication policy update must save"),
    };
    assert_eq!(global_saved.revisions.global, 1);
    let saved = match storage
        .compare_and_swap_publication_policy(&first_id, 1, 0, policy.publication)
        .unwrap()
    {
        SavePublicationPolicyOutcome::Saved(saved) => saved,
        _ => panic!("initial publication policy update must save"),
    };
    assert_eq!(saved.revisions.global, 1);
    assert_eq!(saved.revisions.publication, 1);
    assert_eq!(
        saved.effective.active_content,
        EffectiveActiveContentPolicy::Disabled
    );
    assert_eq!(
        saved.effective.remote_resources,
        EffectiveRemoteResourcesPolicy::Allowed
    );

    let unchanged = match storage
        .compare_and_swap_publication_policy(&first_id, 1, 1, policy.publication)
        .unwrap()
    {
        SavePublicationPolicyOutcome::Saved(saved) => saved,
        _ => panic!("identical publication policy update must be a no-op"),
    };
    assert_eq!(unchanged, saved);
    let second = storage.load_publication_policy(&second_id).unwrap();
    assert_eq!(second.revisions.global, 1);
    assert_eq!(second.revisions.publication, 0);
    assert_eq!(
        second.effective.active_content,
        EffectiveActiveContentPolicy::Enabled
    );
    assert_eq!(
        second.effective.remote_resources,
        EffectiveRemoteResourcesPolicy::Blocked
    );
    assert!(matches!(
        storage
            .compare_and_swap_publication_policy(&first_id, 0, 1, policy.publication)
            .unwrap(),
        SavePublicationPolicyOutcome::RevisionMismatch(current) if current == saved
    ));
    drop(storage);

    let mut storage = Storage::open(directory.path()).unwrap();
    assert_eq!(storage.load_publication_policy(&first_id).unwrap(), saved);
    assert_eq!(storage.load_publication_policy(&second_id).unwrap(), second);
    storage
        .connection
        .execute_batch(
            "UPDATE publication_policy_defaults SET policy_revision = 4294967295;
                 UPDATE publication_policy_overrides SET policy_revision = 4294967295;",
        )
        .unwrap();
    let mut exhausted_global = policy.global;
    exhausted_global.active_content = ActiveContentDefault::Ask;
    assert!(matches!(
        storage
            .compare_and_swap_global_publication_policy(u32::MAX, exhausted_global)
            .unwrap(),
        SavePublicationPolicyOutcome::RevisionExhausted(current)
            if current.revisions.global == u32::MAX
    ));
    let mut exhausted_publication = policy.publication;
    exhausted_publication.active_content = Some(ActiveContentOverride::Enabled);
    match storage
        .compare_and_swap_publication_policy(&first_id, u32::MAX, u32::MAX, exhausted_publication)
        .unwrap()
    {
        SavePublicationPolicyOutcome::RevisionExhausted(current) => {
            assert_eq!(current.revisions.global, u32::MAX);
            assert_eq!(current.revisions.publication, u32::MAX);
        }
        _ => panic!("publication policy revisions must not wrap"),
    }
}

#[test]
fn publication_policy_storage_rejects_values_outside_the_typed_domains() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"strict policy");
    let storage = Storage::open(directory.path()).unwrap();
    assert!(
        storage
            .connection
            .execute(
                "INSERT INTO publication_policy_defaults
                    (singleton_key, active_content, remote_resources, policy_revision)
                 VALUES (1, 'enabled', 'ask', 1)",
                [],
            )
            .is_err()
    );
    assert!(
        storage
            .connection
            .execute(
                "INSERT INTO publication_policy_overrides
                    (publication_id, active_content, remote_resources, policy_revision)
                 VALUES (?1, 'automatic', NULL, 1)",
                [&publication_id],
            )
            .is_err()
    );
    assert!(serde_json::from_str::<PublicationPolicy>(
            r#"{"global":{"activeContent":"ask","remoteResources":"ask","extra":true},"publication":{"activeContent":null,"remoteResources":null}}"#
        )
        .is_err());
}

#[test]
fn publication_policy_relevance_is_conservative_read_only_and_independent() {
    use super::{PolicyRelevance::*, PublicationPolicyRelevance};

    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"policy relevance");
    let other_id = create_test_publication(directory.path(), b"other owner");
    let (_, blob_path) = publication_snapshot(directory.path(), &publication_id);
    let mut storage = Storage::open(directory.path()).unwrap();
    let revision = crate::PRODUCTION_READING_REVISION;
    storage
        .connection
        .execute(
            "UPDATE publication_snapshots SET reading_revision = ?1,
             active_content_relevance = 'detected', remote_resources_relevance = 'not-detected',
             facts_json = 'not JSON', responses_json = 'not JSON' WHERE publication_id = ?2",
            params![revision, publication_id],
        )
        .unwrap();
    let read = |storage: &Storage| {
        storage
            .load_publication_policy_relevance(&publication_id, revision)
            .unwrap()
    };
    let expected = PublicationPolicyRelevance {
        active_content: Detected,
        remote_resources: NotDetected,
    };
    for overrides in [
        PublicationPolicyOverrides {
            active_content: Some(ActiveContentOverride::Enabled),
            remote_resources: Some(RemoteResourcesOverride::Blocked),
        },
        PublicationPolicyOverrides {
            active_content: Some(ActiveContentOverride::Disabled),
            remote_resources: Some(RemoteResourcesOverride::Allowed),
        },
    ] {
        let policy = storage.load_publication_policy(&publication_id).unwrap();
        storage
            .compare_and_swap_publication_policy(
                &publication_id,
                policy.revisions.global,
                policy.revisions.publication,
                overrides,
            )
            .unwrap();
        let before = storage.load_publication_policy(&publication_id).unwrap();
        let changes = storage.connection.total_changes();
        assert_eq!(read(&storage), expected);
        assert_eq!(read(&storage), expected);
        assert_eq!(storage.connection.total_changes(), changes);
        assert_eq!(
            storage.load_publication_policy(&publication_id).unwrap(),
            before
        );
    }
    storage
        .connection
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    for (assignment, expected) in [
        (
            "active_content_relevance = 'unknown'",
            PublicationPolicyRelevance {
                active_content: Unknown,
                remote_resources: NotDetected,
            },
        ),
        (
            "remote_resources_relevance = 'unknown'",
            PublicationPolicyRelevance {
                active_content: Detected,
                remote_resources: Unknown,
            },
        ),
        (
            "active_content_relevance = 'not-detected', remote_resources_relevance = 'detected'",
            PublicationPolicyRelevance {
                active_content: NotDetected,
                remote_resources: Detected,
            },
        ),
        (
            "active_content_relevance = 'malformed'",
            PublicationPolicyRelevance {
                active_content: Unknown,
                remote_resources: NotDetected,
            },
        ),
        (
            "remote_resources_relevance = 'malformed'",
            PublicationPolicyRelevance {
                active_content: Detected,
                remote_resources: Unknown,
            },
        ),
        (
            "reading_revision = 'stale'",
            PublicationPolicyRelevance::unknown(),
        ),
        (
            "revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            PublicationPolicyRelevance::unknown(),
        ),
    ] {
        storage
            .connection
            .execute_batch("SAVEPOINT relevance_test")
            .unwrap();
        storage
            .connection
            .execute(
                &format!("UPDATE publication_snapshots SET {assignment} WHERE publication_id = ?1"),
                [&publication_id],
            )
            .unwrap();
        assert_eq!(read(&storage), expected, "{assignment}");
        storage
            .connection
            .execute_batch("ROLLBACK TO relevance_test; RELEASE relevance_test")
            .unwrap();
    }
    for sql in [
        "UPDATE publications SET current_snapshot_id = NULL WHERE publication_id = ?1",
        "UPDATE publications SET imported_blob_hash = NULL WHERE publication_id = ?1",
        "UPDATE publications SET deletion_state = 'deleting' WHERE publication_id = ?1",
        "UPDATE publication_snapshots SET publication_id = ?2 WHERE publication_id = ?1",
        "UPDATE archive_blobs SET health_state = 'corrupt'",
        "UPDATE archive_blobs SET lifecycle_state = 'gc_pending'",
        "UPDATE archive_blobs SET storage_name = '../invalid' WHERE blob_hash = (SELECT imported_blob_hash FROM publications WHERE publication_id = ?1)",
        "UPDATE archive_blobs SET size_bytes = size_bytes + 1",
        "UPDATE archive_blobs SET size_bytes = -1",
    ] {
        storage
            .connection
            .execute_batch("SAVEPOINT relevance_test")
            .unwrap();
        let mut statement = storage.connection.prepare(sql).unwrap();
        match statement.parameter_count() {
            0 => statement.execute([]).unwrap(),
            1 => statement.execute([&publication_id]).unwrap(),
            _ => statement
                .execute(params![publication_id, other_id])
                .unwrap(),
        };
        assert_eq!(
            read(&storage),
            PublicationPolicyRelevance::unknown(),
            "{sql}"
        );
        storage
            .connection
            .execute_batch("ROLLBACK TO relevance_test; RELEASE relevance_test")
            .unwrap();
    }
    assert_eq!(
        storage
            .load_publication_policy_relevance(&"0".repeat(32), revision)
            .unwrap(),
        PublicationPolicyRelevance::unknown()
    );
    std::fs::remove_file(blob_path).unwrap();
    assert_eq!(read(&storage), PublicationPolicyRelevance::unknown());
}

#[test]
fn publication_policy_survives_snapshot_replacement_and_website_data_reset() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"policy snapshot replacement";
    let publication_id = create_test_publication(directory.path(), bytes);
    let (first_snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
    let mut storage = Storage::open(directory.path()).unwrap();
    let mut policy = storage
        .load_publication_policy(&publication_id)
        .unwrap()
        .policy;
    policy.publication.active_content = Some(ActiveContentOverride::Enabled);
    let saved = match storage
        .compare_and_swap_publication_policy(&publication_id, 0, 0, policy.publication)
        .unwrap()
    {
        SavePublicationPolicyOutcome::Saved(saved) => saved,
        _ => panic!("publication policy must save"),
    };

    let blob = storage.install_blob(bytes).unwrap();
    let (facts, responses, _) = snapshot_values();
    let replacement = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            "reading-v2",
            "Replacement",
            &facts,
            &responses,
        )
        .unwrap();
    assert!(!replacement.created);
    let second_snapshot_id = storage
        .connection
        .query_row(
            "SELECT current_snapshot_id FROM publications WHERE publication_id = ?1",
            [&publication_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_ne!(second_snapshot_id, first_snapshot_id);
    assert_eq!(
        storage.load_publication_policy(&publication_id).unwrap(),
        saved
    );

    assert!(matches!(
        storage
            .reset_publication_website_data(&publication_id)
            .unwrap(),
        ResetPublicationWebsiteDataOutcome::Reset { .. }
    ));
    assert_eq!(
        storage.load_publication_policy(&publication_id).unwrap(),
        saved
    );
}

#[test]
fn publication_policy_cascades_on_deletion_and_is_absent_after_reimport() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"deleted policy publication";
    let publication_id = create_test_publication(directory.path(), bytes);
    let mut storage = Storage::open(directory.path()).unwrap();
    let mut policy = storage
        .load_publication_policy(&publication_id)
        .unwrap()
        .policy;
    policy.publication.remote_resources = Some(RemoteResourcesOverride::Allowed);
    storage
        .compare_and_swap_publication_policy(&publication_id, 0, 0, policy.publication)
        .unwrap();
    storage.begin_publication_deletion(&publication_id).unwrap();
    storage
        .complete_publication_deletion(&publication_id)
        .unwrap();
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM publication_policy_overrides",
                [],
                |row| { row.get::<_, u32>(0) }
            )
            .unwrap(),
        0
    );

    let blob = storage.install_blob(bytes).unwrap();
    let (facts, responses, _) = snapshot_values();
    let reimported = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            "reading-v1",
            "Reimported",
            &facts,
            &responses,
        )
        .unwrap();
    assert_ne!(reimported.publication.publication_id, publication_id);
    let reimported_policy = storage
        .load_publication_policy(&reimported.publication.publication_id)
        .unwrap();
    assert_eq!(
        reimported_policy.policy.publication,
        PublicationPolicyOverrides::default()
    );
    assert_eq!(reimported_policy.revisions.publication, 0);
}

fn snapshot_values() -> (serde_json::Value, serde_json::Value, serde_json::Value) {
    (
        serde_json::json!({ "resources": { "resources": [], "readingOrder": [] } }),
        serde_json::json!({ "resources": {} }),
        serde_json::json!({
            "entries": [],
            "documents": {},
            "effectiveReadingOrderResources": []
        }),
    )
}

#[test]
fn startup_rejects_a_live_publication_without_an_origin() {
    let directory = tempfile::tempdir().unwrap();
    create_test_publication(directory.path(), b"missing origin");
    let database = directory.path().join(super::LIBRARY_DATABASE);
    let connection = Connection::open(database).unwrap();
    connection
        .execute("DELETE FROM browser_origin_allocations", [])
        .unwrap();
    drop(connection);

    let error = Storage::open(directory.path()).err().unwrap();
    assert!(error.contains("live publication has no active browser origin"));
}

fn create_test_publication(directory: &std::path::Path, bytes: &[u8]) -> String {
    let mut storage = Storage::open(directory).unwrap();
    let blob = storage.install_blob(bytes).unwrap();
    let (facts, responses, _) = snapshot_values();
    storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            "reading-v1",
            "Annotations",
            &facts,
            &responses,
        )
        .unwrap()
        .publication
        .publication_id
}

fn publication_snapshot(
    directory: &std::path::Path,
    publication_id: &str,
) -> (String, std::path::PathBuf) {
    let storage = Storage::open(directory).unwrap();
    storage
        .connection
        .query_row(
            "SELECT s.snapshot_id, b.storage_name
                 FROM publications p
                 JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                 JOIN archive_blobs b ON b.blob_hash = s.blob_hash
                 WHERE p.publication_id = ?1",
            [publication_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    storage.blobs.join(row.get::<_, String>(1)?),
                ))
            },
        )
        .unwrap()
}

fn browser_origin_id(storage: &Storage, publication_id: &str) -> String {
    storage
        .connection
        .query_row(
            "SELECT origin_id FROM browser_origin_allocations
                 WHERE publication_id = ?1 AND lifecycle_state = 'active'",
            [publication_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
}

fn save_annotation(
    storage: &mut Storage,
    publication_id: &str,
    value: serde_json::Value,
    resources: &[Option<String>],
) -> SaveAnnotationOutcome {
    storage
        .compare_and_swap_annotations(
            publication_id,
            serde_json::from_value(value).unwrap(),
            resources,
        )
        .unwrap()
}

fn saved_record_id(outcome: SaveAnnotationOutcome) -> String {
    match outcome {
        SaveAnnotationOutcome::Saved { record_id, .. } => record_id,
        _ => panic!("annotation mutation was not saved"),
    }
}

/// Golden database fixtures.
///
/// A ledger step is only proven by a database a real build wrote. Each release
/// leaves one behind here, and every fixture is migrated to head and checked for
/// its content, so a step that silently drops or corrupts stored rows fails the
/// suite rather than a user's install.
///
/// Regenerate the current release's fixture with:
///
/// ```sh
/// ELEPHANT_WRITE_GOLDEN=1 cargo test -p elephant-ladder-native-host golden
/// ```
mod golden {
    use super::{Storage, save_annotation};
    use crate::storage::migrations::CURRENT_SCHEMA;
    use rusqlite::Connection;

    /// One fixture per shipped schema version. Append, never replace: an older
    /// fixture is the only evidence that an older install still upgrades.
    const FIXTURES: &[&str] = &["library-v1.sqlite"];

    fn fixture_directory() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/storage/fixtures")
    }

    /// Writes the rows a golden fixture is expected to carry, through the same
    /// APIs the application uses, so the fixture reflects real stored shapes.
    fn populate(directory: &std::path::Path) {
        let publication_id = super::create_test_publication(directory, b"golden fixture");
        let mut storage = Storage::open(directory).unwrap();

        storage
            .compare_and_swap_reading_position(
                &publication_id,
                0,
                r#"{"kind":"resource","resource":"EPUB/chapter.xhtml"}"#,
            )
            .unwrap();

        save_annotation(
            &mut storage,
            &publication_id,
            serde_json::json!({
                "type": "create-highlight", "requestId": "golden-1",
                "basedOnRevision": 0, "selectionRevision": 1, "occurrence": 0,
                "targetJson": "{}", "excerpt": "a golden excerpt", "sectionLabel": "One"
            }),
            &[Some("EPUB/chapter.xhtml".to_owned())],
        );

        storage
            .mutate_journal(&publication_id, crate::storage::JournalMutation::Begin)
            .unwrap();

        storage
            .save_reading_activity(&crate::storage::ActivityRecord {
                recording_id: "3f2504e0-4f89-11d3-9a0c-0305e82c3301".to_owned(),
                day: "2026-09-25".to_owned(),
                milliseconds: 90_000,
                pages: 12,
            })
            .unwrap();
    }

    /// The rows every fixture must still have after migrating to head, as
    /// `(table, expected count)`. A migration that loses user data fails here.
    const EXPECTED: &[(&str, u32)] = &[
        ("publications", 1),
        ("publication_snapshots", 1),
        ("archive_blobs", 1),
        ("browser_origin_allocations", 1),
        ("reading_positions", 1),
        ("annotation_projections", 1),
        ("annotations", 1),
        ("reflections", 1),
        ("reading_activity", 1),
    ];

    fn schema_objects(connection: &Connection) -> Vec<(String, String)> {
        let mut statement = connection
            .prepare(
                "SELECT name, COALESCE(sql, '') FROM sqlite_master
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn count(connection: &Connection, table: &str) -> u32 {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn every_golden_fixture_migrates_to_head_with_its_content_intact() {
        for fixture in FIXTURES {
            let path = fixture_directory().join(fixture);
            assert!(path.is_file(), "missing golden fixture {fixture}");
            let directory = tempfile::tempdir().unwrap();
            let working = directory.path().join(crate::storage::LIBRARY_DATABASE);
            std::fs::copy(&path, &working).unwrap();

            let connection = Connection::open(&working).unwrap();
            connection
                .execute_batch("PRAGMA foreign_keys = ON;")
                .unwrap();
            crate::storage::migrate(&connection)
                .unwrap_or_else(|error| panic!("{fixture} did not migrate: {error}"));

            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                    .unwrap(),
                CURRENT_SCHEMA,
                "{fixture} did not reach head"
            );
            for (table, expected) in EXPECTED {
                assert_eq!(count(&connection, table), *expected, "{fixture}: {table}");
            }
            let (excerpt, record_type): (String, String) = connection
                .query_row("SELECT excerpt, record_type FROM annotations", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap();
            assert_eq!(excerpt, "a golden excerpt", "{fixture}");
            assert_eq!(record_type, "highlight", "{fixture}");
            // Stored timestamps must stay comparable as strings across releases.
            let stamps: u32 = connection
                .query_row(
                    "SELECT COUNT(*) FROM annotations
                     WHERE created GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T*Z'
                       AND modified >= created",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                stamps, 1,
                "{fixture}: annotation timestamps are not RFC 3339"
            );
            let integrity: String = connection
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .unwrap();
            assert_eq!(integrity, "ok", "{fixture}");

            // Migrating any fixture must land on exactly the schema a new install
            // gets. This is what proves the ledger converges, and it is also what
            // catches a fixture left behind by an older baseline.
            let head = Connection::open_in_memory().unwrap();
            crate::storage::migrate(&head).unwrap();
            assert_eq!(
                schema_objects(&connection),
                schema_objects(&head),
                "{fixture} does not migrate to the current schema"
            );
        }
    }

    /// Writes this release's fixture. Off by default; the assertion above is what
    /// runs in an ordinary test pass.
    #[test]
    fn write_the_current_golden_fixture() {
        if std::env::var_os("ELEPHANT_WRITE_GOLDEN").is_none() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        populate(directory.path());
        let fixtures = fixture_directory();
        std::fs::create_dir_all(&fixtures).unwrap();
        let target = fixtures.join(format!("library-v{CURRENT_SCHEMA}.sqlite"));
        // `VACUUM INTO` refuses to overwrite, and regenerating the current
        // release's fixture is the normal case while that release is unreleased.
        if target.exists() {
            std::fs::remove_file(&target).unwrap();
        }
        let source = directory.path().join(crate::storage::LIBRARY_DATABASE);
        Connection::open(&source)
            .unwrap()
            .execute_batch(&format!(
                "VACUUM INTO '{}'",
                target.display().to_string().replace('\'', "''")
            ))
            .unwrap();
        println!("wrote {}", target.display());
    }
}

/// The pagination cache is derived data in its own file, so it is replaced rather
/// than repaired. A cache the host does not recognise must never stop a reader
/// from opening their library.
#[test]
fn an_unrecognised_pagination_cache_is_discarded_rather_than_opened() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"cache identity");
    let (snapshot_id, _) = publication_snapshot(directory.path(), &publication_id);
    let cache = directory.path().join("pagination.sqlite");
    assert!(cache.is_file(), "the cache is its own file");

    {
        let mut storage = Storage::open(directory.path()).unwrap();
        storage
            .write_pagination_record(
                &publication_id,
                &snapshot_id,
                &"a".repeat(64),
                &"b".repeat(64),
                b"value",
            )
            .unwrap();
        assert!(matches!(
            storage
                .read_pagination_record(
                    &publication_id,
                    &snapshot_id,
                    &"a".repeat(64),
                    &"b".repeat(64),
                )
                .unwrap(),
            PaginationCacheReadOutcome::Found(_)
        ));
    }

    // A cache written by something else, carrying no recognisable identity.
    let foreign = Connection::open(&cache).unwrap();
    foreign
        .execute_batch("PRAGMA user_version = 0; CREATE TABLE intruder (x INTEGER);")
        .unwrap();
    drop(foreign);

    let mut storage = Storage::open(directory.path()).unwrap();
    assert!(matches!(
        storage
            .read_pagination_record(
                &publication_id,
                &snapshot_id,
                &"a".repeat(64),
                &"b".repeat(64),
            )
            .unwrap(),
        PaginationCacheReadOutcome::Missing
    ));
    assert_eq!(
        storage
            .pagination
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'intruder'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .unwrap(),
        0,
        "the unrecognised cache must have been replaced"
    );
    // The library itself is untouched by any of this.
    assert_eq!(storage.list_publications().unwrap().len(), 1);
}

/// Stored annotation timestamps are compared as strings, by the `modified >= created`
/// constraint and by anything that later merges or exchanges them, so their width
/// has to be fixed. `Rfc3339` formatting drops trailing zeros, which made a
/// same-second edit intermittently sort before the record it was editing.
#[test]
fn annotation_timestamps_sort_as_strings_in_chronological_order() {
    let base = time::OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap();
    let mut previous: Option<String> = None;
    for milliseconds in [0_i64, 1, 10, 100, 500, 501, 990, 999] {
        let formatted =
            super::format_annotation_timestamp(base + time::Duration::milliseconds(milliseconds))
                .unwrap();
        assert_eq!(formatted.len(), 24, "{formatted} is not a fixed width");
        assert!(
            super::parse_annotation_timestamp(&formatted).is_some(),
            "{formatted}"
        );
        if let Some(previous) = previous {
            assert!(
                previous < formatted,
                "{previous} must sort before {formatted}"
            );
        }
        previous = Some(formatted);
    }
}

/// A deletion that can never succeed must stop waking the host every minute.
/// `attempts` is what sets the delay, so the counter is load-bearing rather than
/// a value nothing reads.
#[test]
fn a_failing_deletion_backs_its_retry_off_to_an_hour() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"deletion backoff");
    let mut storage = Storage::open(directory.path()).unwrap();
    assert!(matches!(
        storage.begin_publication_deletion(&publication_id).unwrap(),
        BeginPublicationDeletionOutcome::Started { .. }
    ));

    let mut delays = Vec::new();
    for _ in 0..8 {
        storage
            .record_deletion_failure(&publication_id, "disk on fire")
            .unwrap();
        let minutes: f64 = storage
            .connection
            .query_row(
                "SELECT (julianday(next_retry_at) - julianday(updated_at)) * 1440
                 FROM deletion_operations WHERE publication_id = ?1",
                [&publication_id],
                |row| row.get(0),
            )
            .unwrap();
        delays.push(minutes.round() as i64);
    }
    assert_eq!(delays, vec![1, 2, 4, 8, 16, 32, 64, 64]);

    let attempts: u32 = storage
        .connection
        .query_row(
            "SELECT attempts FROM deletion_operations WHERE publication_id = ?1",
            [&publication_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 8);
    // The failure stays visible rather than being retried silently.
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT deletion_state FROM publications WHERE publication_id = ?1",
                [&publication_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "delete_failed"
    );
}

/// `lifecycle_state` says where a blob's file belongs and `health_state` says what
/// the filesystem found there. Reconciliation has to read the location the
/// lifecycle implies, or a blob that is intact in the trash gets reported missing.
#[test]
fn reconciliation_reads_health_at_the_location_the_lifecycle_implies() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"trashed blob health");
    let storage = Storage::open(directory.path()).unwrap();
    let (blob_hash, storage_name): (String, String) = storage
        .connection
        .query_row(
            "SELECT blob_hash, storage_name FROM archive_blobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let blobs = directory.path().join("archives/blobs");
    let trash = directory.path().join("archives/trash");

    // Put the blob where a trashed blob lives, and say so.
    std::fs::rename(blobs.join(&storage_name), trash.join(&storage_name)).unwrap();
    storage
        .connection
        .execute(
            "UPDATE archive_blobs SET lifecycle_state = 'trashed' WHERE blob_hash = ?1",
            [&blob_hash],
        )
        .unwrap();
    // A file in the trash that no row claims.
    std::fs::write(trash.join("f".repeat(64)), b"orphan").unwrap();
    drop(storage);

    let storage = Storage::open(directory.path()).unwrap();
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT health_state FROM archive_blobs WHERE blob_hash = ?1",
                [&blob_hash],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "available",
        "a trashed blob that is intact must not be reported missing"
    );
    assert!(trash.join(&storage_name).is_file(), "its file stays");
    assert!(
        !trash.join("f".repeat(64)).exists(),
        "an unclaimed trash file is swept"
    );
    assert_eq!(storage.list_publications().unwrap().len(), 1);
    let _ = publication_id;
}

/// A damaged library is set aside and replaced rather than migrated, because
/// migrating corrupt pages cannot repair them and refusing would leave an
/// application that will not start. The bytes are kept, and the archives with them.
#[test]
fn a_corrupt_library_is_quarantined_and_replaced_on_upgrade() {
    use std::io::{Seek, SeekFrom, Write};

    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"corrupt library");
    let database = directory.path().join(super::LIBRARY_DATABASE);
    let blob_count = std::fs::read_dir(directory.path().join("archives/blobs"))
        .unwrap()
        .count();
    assert_eq!(blob_count, 1);

    // Leave the file needing an upgrade, then scribble over pages past the header.
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("PRAGMA journal_mode = DELETE; PRAGMA user_version = 0;")
            .unwrap();
    }
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&database)
            .unwrap();
        file.seek(SeekFrom::Start(4096)).unwrap();
        file.write_all(&[0x5a; 8192]).unwrap();
        file.flush().unwrap();
    }

    let storage = Storage::open(directory.path()).unwrap();
    assert!(
        storage.list_publications().unwrap().is_empty(),
        "the replacement library starts empty"
    );
    let quarantined = std::fs::read_dir(directory.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".unopenable-"))
        .count();
    assert_eq!(quarantined, 1, "the damaged bytes are kept, not deleted");
    // The archives move with the quarantined database instead of being swept by
    // the replacement library, so the publication can be imported again.
    let kept = std::fs::read_dir(directory.path().join("archives"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("unopenable-")
        })
        .expect("the archives are set aside");
    assert_eq!(std::fs::read_dir(kept.path()).unwrap().count(), 1);
    let _ = publication_id;
}

/// An annotation id is only unique within its publication: an imported set carries
/// the foreign ids, so the same id can legitimately belong to two publications. A
/// globally keyed table would let one publication's write overwrite the other's row.
#[test]
fn two_publications_can_hold_the_same_annotation_id() {
    let directory = tempfile::tempdir().unwrap();
    let first = create_test_publication(directory.path(), b"annotation id first");
    let second = create_test_publication(directory.path(), b"annotation id second");
    let storage = Storage::open(directory.path()).unwrap();
    let shared = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";

    for (publication_id, excerpt) in [(&first, "first excerpt"), (&second, "second excerpt")] {
        storage
            .connection
            .execute(
                "INSERT INTO annotation_projections (publication_id, projection_revision)
                 VALUES (?1, 1)",
                [publication_id],
            )
            .unwrap();
        storage
            .connection
            .execute(
                "INSERT INTO annotations
                    (annotation_id, publication_id, record_order, record_type, occurrence,
                     target_json, resource_hint, excerpt, created, modified)
                 VALUES (?1, ?2, 1, 'highlight', 0, '{}', 'EPUB/chapter.xhtml', ?3,
                         '2026-09-26T00:00:00.000Z', '2026-09-26T00:00:00.000Z')",
                rusqlite::params![shared, publication_id, excerpt],
            )
            .unwrap();
    }

    let load = |storage: &Storage, publication_id: &str| {
        storage
            .connection
            .query_row(
                "SELECT excerpt FROM annotations WHERE publication_id = ?1",
                [publication_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };
    assert_eq!(load(&storage, &first), "first excerpt");
    assert_eq!(load(&storage, &second), "second excerpt");
    assert!(storage.load_annotations(&first).unwrap().records.len() == 1);
    assert!(storage.load_annotations(&second).unwrap().records.len() == 1);
}

/// A pagination cache that is not even readable as a database must be replaced, not
/// treated as a fatal error: it holds nothing that cannot be derived again, and
/// refusing to launch over it would trade a rebuild for an unusable application.
#[test]
fn an_unreadable_pagination_cache_does_not_stop_the_library_opening() {
    let directory = tempfile::tempdir().unwrap();
    let publication_id = create_test_publication(directory.path(), b"rubble cache");
    std::fs::write(
        directory.path().join("pagination.sqlite"),
        b"this is not a database at all, it is rubble",
    )
    .unwrap();

    let storage = Storage::open(directory.path()).unwrap();
    assert_eq!(storage.list_publications().unwrap().len(), 1);
    assert_eq!(
        storage
            .pagination
            .query_row("SELECT COUNT(*) FROM pagination_bases", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
    let _ = publication_id;
}

/// Quitting and relaunching races the departing host's shutdown: the storage lock
/// is released when its descriptor closes, and until then a new host sees the same
/// `EAGAIN` a genuinely running second instance produces. The wait tells them apart
/// instead of refusing to start over a condition that clears in milliseconds.
#[test]
fn a_relaunch_waits_for_a_departing_host_to_release_the_lock() {
    let directory = tempfile::tempdir().unwrap();
    let held = Storage::open(directory.path()).unwrap();
    let path = directory.path().to_owned();

    // A predecessor that lets go shortly after the relaunch begins.
    let departing = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(250));
        drop(held);
    });
    let opened = std::time::Instant::now();
    let storage = Storage::open(&path).expect("the relaunch waits rather than refusing");
    assert!(
        opened.elapsed() >= std::time::Duration::from_millis(200),
        "it waited for the predecessor"
    );
    drop(storage);
    departing.join().unwrap();
}

/// A second instance that is genuinely running is still refused, just not instantly.
#[test]
fn a_running_second_host_is_still_refused() {
    let directory = tempfile::tempdir().unwrap();
    let _held = Storage::open(directory.path()).unwrap();
    let error = Storage::open(directory.path())
        .err()
        .expect("a running host keeps the lock");
    assert!(error.contains("already owned by another host"), "{error}");
}

mod metadata_direction {
    use crate::storage::library::resolved_direction;
    use epub_stack::semantics::TextDirection;

    /// EPUB resolves a metadata element's direction from the element, then the package.
    /// An explicit `auto` is a declaration and wins over the package, where silence does
    /// not; the corpus distinguishes the two.
    #[test]
    fn an_element_decides_its_own_direction_before_the_package_does() {
        assert_eq!(
            resolved_direction(Some(TextDirection::Rtl), Some(TextDirection::Ltr)).as_deref(),
            Some("rtl")
        );
        assert_eq!(
            resolved_direction(Some(TextDirection::Rtl), None).as_deref(),
            Some("rtl")
        );
        assert_eq!(
            resolved_direction(Some(TextDirection::Auto), Some(TextDirection::Rtl)).as_deref(),
            Some("auto")
        );
    }

    #[test]
    fn an_element_that_says_nothing_inherits_the_package() {
        assert_eq!(
            resolved_direction(None, Some(TextDirection::Rtl)).as_deref(),
            Some("rtl")
        );
        assert_eq!(
            resolved_direction(None, Some(TextDirection::Ltr)).as_deref(),
            Some("ltr")
        );
    }

    /// Silence on both is not `auto`: nothing is declared, so nothing is rendered, and
    /// the reading system is free to choose.
    #[test]
    fn silence_everywhere_states_no_direction() {
        assert_eq!(resolved_direction(None, None), None);
    }
}

/// The library shows a title and creators as text, so the direction each reads has to
/// survive the database and come back with the summary. A right-to-left title rendered
/// left-to-right reorders visibly, so this is a rendering fact rather than a nicety.
#[test]
fn metadata_directions_survive_the_database_and_a_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let facts = serde_json::json!({
        "resources": { "resources": [], "readingOrder": [] },
        "package": {
            "dir": "rtl",
            "metadata": {
                // The title declares nothing, so it inherits the package.
                "title": [{ "content": "CSS: הרפתקה חדשה!", "dir": null }],
                "creator": [
                    { "content": "Dave Cramer", "dir": "rtl" },
                    { "content": "Ivan Herman", "dir": "ltr" },
                    { "content": "Unstated", "dir": null }
                ],
                "language": [{ "content": "he" }]
            },
            "spine": { "pageProgressionDirection": "rtl" }
        }
    });
    let responses = serde_json::json!({ "resources": {} });
    let publication_id = {
        let mut storage = Storage::open(directory.path()).unwrap();
        let blob = storage.install_blob(b"direction archive").unwrap();
        storage
            .commit_import_snapshot(
                InstalledImport {
                    archive: &blob,
                    cover: None,
                },
                "reading-test",
                "CSS: הרפתקה חדשה!",
                &facts,
                &responses,
            )
            .unwrap()
            .publication
            .publication_id
    };

    let storage = Storage::open(directory.path()).unwrap();
    let listed = storage.list_publications().unwrap();
    let summary = listed
        .iter()
        .find(|summary| summary.publication_id == publication_id)
        .expect("the imported publication is listed");
    assert_eq!(summary.title_direction.as_deref(), Some("rtl"));
    assert_eq!(
        summary.creator_directions,
        vec![
            Some("rtl".to_owned()),
            Some("ltr".to_owned()),
            // Stated nowhere on the element, so it inherits the package.
            Some("rtl".to_owned()),
        ]
    );
    assert_eq!(summary.creators.len(), summary.creator_directions.len());
    // The page progression direction is a separate fact and must not be confused with it.
    assert_eq!(summary.reading_direction.as_deref(), Some("rtl"));
}

/// A publication that declares nothing anywhere states no direction, which is not the
/// same as asking for it to be inferred.
#[test]
fn a_publication_that_declares_no_direction_stores_none() {
    let directory = tempfile::tempdir().unwrap();
    let facts = serde_json::json!({
        "resources": { "resources": [], "readingOrder": [] },
        "package": {
            "dir": null,
            "metadata": {
                "title": [{ "content": "Plain", "dir": null }],
                "creator": [{ "content": "Someone", "dir": null }],
                "language": []
            },
            "spine": {}
        }
    });
    let responses = serde_json::json!({ "resources": {} });
    let mut storage = Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(b"plain archive").unwrap();
    let outcome = storage
        .commit_import_snapshot(
            InstalledImport {
                archive: &blob,
                cover: None,
            },
            "reading-test",
            "Plain",
            &facts,
            &responses,
        )
        .unwrap();
    assert_eq!(outcome.publication.title_direction, None);
    assert_eq!(outcome.publication.creator_directions, vec![None]);
}
