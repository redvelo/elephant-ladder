//! Where a reader was: the stored position, its revision compare-and-swap, and
//! the summary the library shows.

#[cfg(test)]
use super::StoredReadingPosition;
use super::{
    ReadingPosition, ReadingSummary, SaveReadingPositionOutcome, Storage, validate_reading_position,
};
use rusqlite::{OptionalExtension, params};

impl Storage {
    #[cfg(test)]
    pub(crate) fn load_reading_position(
        &self,
        publication_id: &str,
    ) -> Result<Option<StoredReadingPosition>, String> {
        self.connection
            .query_row(
                "SELECT position_json, position_revision
                 FROM reading_positions WHERE publication_id = ?1",
                [publication_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|error| format!("could not load reading position: {error}"))?
            .map(|(position_json, revision)| {
                Ok(StoredReadingPosition {
                    position_json,
                    revision: u32::try_from(revision)
                        .map_err(|_| "stored reading position revision is invalid".to_owned())?,
                })
            })
            .transpose()
    }

    #[cfg(test)]
    pub(crate) fn compare_and_swap_reading_position(
        &mut self,
        publication_id: &str,
        base_revision: u32,
        position_json: &str,
    ) -> Result<SaveReadingPositionOutcome, String> {
        self.save_reading_position_summary(publication_id, base_revision, position_json, None)
    }

    pub(crate) fn save_reading_position_summary(
        &mut self,
        publication_id: &str,
        base_revision: u32,
        position_json: &str,
        summary: Option<&ReadingSummary>,
    ) -> Result<SaveReadingPositionOutcome, String> {
        let summary_json = summary
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| error.to_string())?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| format!("could not begin reading position transaction: {error}"))?;
        let publication_exists = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM publications
                    WHERE publication_id = ?1 AND deletion_state = 'live'
                 )",
                [publication_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| format!("could not validate reading position publication: {error}"))?;
        if !publication_exists {
            return Err("reading position publication is unavailable".to_owned());
        }
        let current_revision = transaction
            .query_row(
                "SELECT position_revision FROM reading_positions WHERE publication_id = ?1",
                [publication_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| format!("could not read current position revision: {error}"))?
            .map(|revision| {
                u32::try_from(revision)
                    .map_err(|_| "stored reading position revision is invalid".to_owned())
            })
            .transpose()?
            .unwrap_or(0);
        if current_revision != base_revision {
            return Ok(SaveReadingPositionOutcome::RevisionMismatch { current_revision });
        }
        let Some(revision) = current_revision.checked_add(1) else {
            return Ok(SaveReadingPositionOutcome::RevisionExhausted { current_revision });
        };
        if current_revision == 0 {
            transaction
                .execute(
                    "INSERT INTO reading_positions (
                        publication_id, position_json, position_revision, summary_json
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![publication_id, position_json, revision, summary_json],
                )
                .map_err(|error| format!("could not create reading position: {error}"))?;
        } else {
            let changed = transaction
                .execute(
                    "UPDATE reading_positions
                     SET position_json = ?2, position_revision = ?3, summary_json = ?5,
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE publication_id = ?1 AND position_revision = ?4",
                    params![
                        publication_id,
                        position_json,
                        revision,
                        base_revision,
                        summary_json
                    ],
                )
                .map_err(|error| format!("could not update reading position: {error}"))?;
            if changed != 1 {
                return Err("reading position compare-and-swap was not atomic".to_owned());
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("could not commit reading position: {error}"))?;
        Ok(SaveReadingPositionOutcome::Saved { revision })
    }
}

pub(super) fn decode_reading_position(
    position_json: Option<String>,
    revision: Option<i64>,
) -> Result<(Option<ReadingPosition>, u32), String> {
    match (position_json, revision) {
        (None, None) => Ok((None, 0)),
        (Some(json), Some(revision)) => {
            let revision = u32::try_from(revision)
                .map_err(|_| "stored reading position revision is invalid".to_owned())?;
            let position: ReadingPosition = serde_json::from_str(&json)
                .map_err(|error| format!("stored reading position is invalid: {error}"))?;
            validate_reading_position(&position)
                .map_err(|error| format!("stored reading position is invalid: {error}"))?;
            Ok((Some(position), revision))
        }
        _ => Err("stored reading position descriptor is incomplete".to_owned()),
    }
}
