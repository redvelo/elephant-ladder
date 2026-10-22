//! Reader preferences and application settings.
//!
//! Global and per-publication preferences are stored separately and composed on
//! read, so a publication override stays distinguishable from an inherited
//! value. Preferences load before the reader is created and are written under
//! revision compare-and-swap.

use super::{Storage, compose_preferences};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

impl Storage {
    /// `side_buttons_default` is what an unanswered device setting means on this device, which
    /// only the host knows: an e-reader's side buttons are page buttons, a phone's are volume.
    pub(crate) fn load_app_settings(
        &self,
        side_buttons_default: bool,
    ) -> Result<AppSettings, String> {
        let (global, revision) = load_global_preferences(&self.connection)?;
        let defaults = global.reading_defaults.clone().unwrap_or_default();
        let device = DeviceSettings {
            side_buttons_turn_pages: global
                .side_buttons_turn_pages
                .unwrap_or(side_buttons_default),
        };
        Ok(AppSettings {
            revision,
            preferences: compose_preferences(global, defaults)?,
            device,
        })
    }

    pub(crate) fn save_app_settings(
        &mut self,
        revision: u32,
        preferences: &ReaderPreferences,
        device: DeviceSettings,
    ) -> Result<bool, String> {
        let (mut global, defaults) = preferences.split()?;
        global.reading_defaults = Some(defaults);
        global.side_buttons_turn_pages = Some(device.side_buttons_turn_pages);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let (current, current_revision) = load_global_preferences(&transaction)?;
        if revision != current_revision {
            return Ok(false);
        }
        if current != global {
            let next_revision = current_revision
                .checked_add(1)
                .ok_or("preference revision exhausted")?;
            let json = serde_json::to_string(&global).map_err(|error| error.to_string())?;
            if json.len() > MAX_GLOBAL_PREFERENCES_BYTES {
                return Err("global preferences exceed their storage bound".into());
            }
            transaction.execute(
                "INSERT INTO reader_preferences (singleton_key, preferences_json, preferences_revision) VALUES (1, ?1, ?2)
                 ON CONFLICT(singleton_key) DO UPDATE SET preferences_json = excluded.preferences_json,
                 preferences_revision = excluded.preferences_revision, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                params![json, next_revision],
            ).map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(true)
    }

    /// Returns the reading preferences to what the application ships with, keeping the device
    /// answer, which describes the hardware rather than a reading choice.
    pub(crate) fn reset_app_reading_preferences(&mut self, revision: u32) -> Result<bool, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let (current, current_revision) = load_global_preferences(&transaction)?;
        if revision != current_revision {
            return Ok(false);
        }
        let next = GlobalReaderPreferences {
            reading_defaults: Some(PublicationReaderPreferences::default()),
            side_buttons_turn_pages: current.side_buttons_turn_pages,
            ..GlobalReaderPreferences::default()
        };
        if current != next {
            let next_revision = current_revision
                .checked_add(1)
                .ok_or("preference revision exhausted")?;
            let json = serde_json::to_string(&next).map_err(|error| error.to_string())?;
            transaction.execute(
                "INSERT INTO reader_preferences (singleton_key, preferences_json, preferences_revision) VALUES (1, ?1, ?2)
                 ON CONFLICT(singleton_key) DO UPDATE SET preferences_json = excluded.preferences_json,
                 preferences_revision = excluded.preferences_revision, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                params![json, next_revision],
            ).map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(true)
    }

    /// Drops a publication's own layout, so it inherits the application defaults again. Without
    /// this a book that once differed from the defaults could never follow them a second time.
    pub(crate) fn clear_publication_preferences(
        &mut self,
        publication_id: &str,
    ) -> Result<(), String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let live = transaction
            .query_row(
                "SELECT 1 FROM publications
                 WHERE publication_id = ?1 AND deletion_state = 'live'",
                [publication_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("could not validate preference publication: {error}"))?
            .is_some();
        if !live {
            return Err("preference publication is unavailable".to_owned());
        }
        transaction
            .execute(
                "DELETE FROM publication_reader_preferences WHERE publication_id = ?1",
                [publication_id],
            )
            .map_err(|error| format!("could not clear publication preferences: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit publication preferences: {error}"))
    }

    pub fn load_publication_preferences(
        &self,
        publication_id: &str,
    ) -> Result<StoredPreferences, String> {
        let live = self
            .connection
            .query_row(
                "SELECT 1 FROM publications
                 WHERE publication_id = ?1 AND deletion_state = 'live'",
                [publication_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("could not validate preference publication: {error}"))?
            .is_some();
        if !live {
            return Err("preference publication is unavailable".to_owned());
        }
        let (global, global_revision) = load_global_preferences(&self.connection)?;
        let (publication, publication_revision) =
            load_publication_preferences(&self.connection, publication_id)?;
        let publication = if publication_revision == 0 {
            global.reading_defaults.clone().unwrap_or(publication)
        } else {
            publication
        };
        Ok(StoredPreferences {
            preferences: compose_preferences(global, publication)?,
            revisions: PreferenceRevisions {
                global: global_revision,
                publication: publication_revision,
            },
        })
    }

    pub(crate) fn compare_and_swap_preferences(
        &mut self,
        publication_id: &str,
        base_global_revision: u32,
        base_publication_revision: u32,
        preferences: &ReaderPreferences,
    ) -> Result<SavePreferencesOutcome, String> {
        let (mut next_global, next_publication) = preferences.split()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin preference transaction: {error}"))?;
        let live = transaction
            .query_row(
                "SELECT 1 FROM publications
                 WHERE publication_id = ?1 AND deletion_state = 'live'",
                [publication_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("could not validate preference session: {error}"))?
            .is_some();
        if !live {
            return Err("preference publication is unavailable".to_owned());
        }
        let (current_global, global_revision) = load_global_preferences(&transaction)?;
        next_global.reading_defaults = current_global.reading_defaults.clone();
        let (current_publication, publication_revision) =
            load_publication_preferences(&transaction, publication_id)?;
        let current_publication = if publication_revision == 0 {
            current_global
                .reading_defaults
                .clone()
                .unwrap_or(current_publication)
        } else {
            current_publication
        };
        let current = StoredPreferences {
            preferences: compose_preferences(current_global.clone(), current_publication.clone())?,
            revisions: PreferenceRevisions {
                global: global_revision,
                publication: publication_revision,
            },
        };
        if base_global_revision != global_revision
            || base_publication_revision != publication_revision
        {
            return Ok(SavePreferencesOutcome::RevisionMismatch(current));
        }
        let global_changed = next_global != current_global;
        let publication_changed = next_publication != current_publication;
        let next_global_revision = if global_changed {
            match global_revision.checked_add(1) {
                Some(revision) => revision,
                None => return Ok(SavePreferencesOutcome::RevisionExhausted(current)),
            }
        } else {
            global_revision
        };
        let next_publication_revision = if publication_changed {
            match publication_revision.checked_add(1) {
                Some(revision) => revision,
                None => return Ok(SavePreferencesOutcome::RevisionExhausted(current)),
            }
        } else {
            publication_revision
        };
        if global_changed {
            let json = serde_json::to_string(&next_global)
                .map_err(|error| format!("could not serialize global preferences: {error}"))?;
            if json.len() > MAX_GLOBAL_PREFERENCES_BYTES {
                return Err("global preferences exceed their storage bound".to_owned());
            }
            transaction
                .execute(
                    "INSERT INTO reader_preferences
                        (singleton_key, preferences_json, preferences_revision)
                     VALUES (1, ?1, ?2)
                     ON CONFLICT(singleton_key) DO UPDATE SET
                        preferences_json = excluded.preferences_json,
                        preferences_revision = excluded.preferences_revision,
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                    params![json, next_global_revision],
                )
                .map_err(|error| format!("could not save global preferences: {error}"))?;
        }
        if publication_changed {
            let json = serde_json::to_string(&next_publication)
                .map_err(|error| format!("could not serialize publication preferences: {error}"))?;
            if json.len() > MAX_PUBLICATION_PREFERENCES_BYTES {
                return Err("publication preferences exceed their storage bound".to_owned());
            }
            transaction
                .execute(
                    "INSERT INTO publication_reader_preferences
                        (publication_id, preferences_json, preferences_revision)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(publication_id) DO UPDATE SET
                        preferences_json = excluded.preferences_json,
                        preferences_revision = excluded.preferences_revision,
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                    params![publication_id, json, next_publication_revision],
                )
                .map_err(|error| format!("could not save publication preferences: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("could not commit reader preferences: {error}"))?;
        Ok(SavePreferencesOutcome::Saved(StoredPreferences {
            preferences: preferences.clone(),
            revisions: PreferenceRevisions {
                global: next_global_revision,
                publication: next_publication_revision,
            },
        }))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderPreferences {
    pub color_scheme: String,
    pub presentation_mode: String,
    pub pagination_columns: String,
    pub publication_theme: String,
    pub publication_typeface: String,
    pub font_scale: String,
    pub line_height: String,
    pub letter_spacing: String,
    pub word_spacing: String,
    pub paragraph_spacing: String,
    pub text_indent: String,
    pub measure: String,
    pub alignment: String,
    pub hyphenation: String,
    pub keyboard_mode: String,
    pub touch_zones: bool,
    #[serde(default)]
    pub click_zones: bool,
    pub narration_rate: String,
    pub narration_skipped_types: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferenceRevisions {
    pub global: u32,
    pub publication: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPreferences {
    pub preferences: ReaderPreferences,
    pub revisions: PreferenceRevisions,
}

pub(crate) enum SavePreferencesOutcome {
    Saved(StoredPreferences),
    RevisionMismatch(StoredPreferences),
    RevisionExhausted(StoredPreferences),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GlobalReaderPreferences {
    pub(crate) version: u8,
    #[serde(default)]
    pub(crate) reading_defaults: Option<PublicationReaderPreferences>,
    pub(crate) color_scheme: String,
    pub(crate) publication_theme: String,
    pub(crate) publication_typeface: String,
    pub(crate) font_scale: String,
    pub(crate) line_height: String,
    #[serde(default = "default_letter_spacing")]
    pub(crate) letter_spacing: String,
    #[serde(default = "default_word_spacing")]
    pub(crate) word_spacing: String,
    #[serde(default = "default_paragraph_spacing")]
    pub(crate) paragraph_spacing: String,
    #[serde(default = "default_text_indent")]
    pub(crate) text_indent: String,
    pub(crate) measure: String,
    pub(crate) alignment: String,
    pub(crate) hyphenation: String,
    pub(crate) keyboard_mode: String,
    pub(crate) touch_zones: bool,
    #[serde(default)]
    pub(crate) click_zones: bool,
    #[serde(default = "default_narration_rate")]
    pub(crate) narration_rate: String,
    #[serde(default)]
    pub(crate) narration_skipped_types: Vec<String>,
    /// Whether a device's own page buttons turn pages here. Absent until it is chosen, so the
    /// answer can follow the display a device has rather than a value nobody set.
    #[serde(default)]
    pub(crate) side_buttons_turn_pages: Option<bool>,
}

impl GlobalReaderPreferences {
    /// Rewrites values this version no longer offers. `original` text size meant "leave the
    /// publication's root size alone", which rendered as 100% for every publication that did
    /// not fix its own root; the rewrite is in memory, so the row settles on the next save.
    pub(crate) fn migrate(&mut self) {
        if self.font_scale == "original" {
            self.font_scale = "100".to_owned();
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppSettings {
    pub preferences: ReaderPreferences,
    pub revision: u32,
    /// What the application itself keeps, which the reader knows nothing about.
    pub device: DeviceSettings,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DeviceSettings {
    pub side_buttons_turn_pages: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublicationReaderPreferences {
    pub(crate) version: u8,
    pub(crate) presentation_mode: String,
    pub(crate) pagination_columns: String,
}

pub(crate) fn load_global_preferences(
    connection: &Connection,
) -> Result<(GlobalReaderPreferences, u32), String> {
    let stored = connection
        .query_row(
            "SELECT preferences_json, preferences_revision FROM reader_preferences
             WHERE singleton_key = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|error| format!("could not load global preferences: {error}"))?;
    let Some((json, revision)) = stored else {
        return Ok((GlobalReaderPreferences::default(), 0));
    };
    if json.len() > MAX_GLOBAL_PREFERENCES_BYTES {
        return Err("stored global preferences exceed their bound".to_owned());
    }
    let mut preferences: GlobalReaderPreferences = serde_json::from_str(&json)
        .map_err(|error| format!("stored global preferences are invalid: {error}"))?;
    preferences.migrate();
    let revision = u32::try_from(revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| "stored global preference revision is invalid".to_owned())?;
    compose_preferences(preferences.clone(), PublicationReaderPreferences::default())?;
    Ok((preferences, revision))
}

pub(super) const MAX_GLOBAL_PREFERENCES_BYTES: usize = 4 * 1024;

pub(super) const MAX_PUBLICATION_PREFERENCES_BYTES: usize = 1024;

pub(super) fn load_publication_preferences(
    connection: &Connection,
    publication_id: &str,
) -> Result<(PublicationReaderPreferences, u32), String> {
    let stored = connection
        .query_row(
            "SELECT preferences_json, preferences_revision
             FROM publication_reader_preferences WHERE publication_id = ?1",
            [publication_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|error| format!("could not load publication preferences: {error}"))?;
    let Some((json, revision)) = stored else {
        return Ok((PublicationReaderPreferences::default(), 0));
    };
    if json.len() > MAX_PUBLICATION_PREFERENCES_BYTES {
        return Err("stored publication preferences exceed their bound".to_owned());
    }
    let preferences: PublicationReaderPreferences = serde_json::from_str(&json)
        .map_err(|error| format!("stored publication preferences are invalid: {error}"))?;
    let revision = u32::try_from(revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| "stored publication preference revision is invalid".to_owned())?;
    compose_preferences(GlobalReaderPreferences::default(), preferences.clone())?;
    Ok((preferences, revision))
}

pub(super) const NARRATION_RATES: [&str; 9] =
    ["0.5", "0.75", "1", "1.25", "1.5", "1.75", "2", "2.5", "3"];

pub(super) const MAX_NARRATION_SKIPPED_TYPES: usize = 32;

pub(super) const MAX_NARRATION_SKIPPED_TYPE_BYTES: usize = 64;

pub(super) fn default_narration_rate() -> String {
    "1".to_owned()
}

/// Spacing a stored row predates: no reader-imposed spacing, authored paragraph shape kept.
pub(super) fn default_letter_spacing() -> String {
    "0".to_owned()
}

pub(super) fn default_word_spacing() -> String {
    "0".to_owned()
}

pub(super) fn default_paragraph_spacing() -> String {
    "original".to_owned()
}

pub(super) fn default_text_indent() -> String {
    "original".to_owned()
}
