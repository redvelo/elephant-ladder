use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::Storage;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ActivityRecord {
    pub recording_id: String,
    pub day: String,
    pub milliseconds: u32,
    pub pages: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActivityDay {
    day: String,
    milliseconds: u64,
    pages: u64,
}

impl ActivityRecord {
    pub fn valid(&self) -> bool {
        let bytes = self.day.as_bytes();
        let valid_date = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
            && self.day[5..7]
                .parse::<u32>()
                .is_ok_and(|n| (1..=12).contains(&n))
            && self.day[8..10]
                .parse::<u32>()
                .is_ok_and(|n| (1..=31).contains(&n));
        valid_date
            && self.recording_id.len() == 36
            && self
                .recording_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'-')
            && self.milliseconds <= 86_400_000
            && self.pages <= 100_000
    }
}

impl Storage {
    pub(crate) fn save_reading_activity(&self, record: &ActivityRecord) -> Result<(), String> {
        self.connection.execute(
            "INSERT INTO reading_activity(recording_id, day, milliseconds, pages) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(recording_id, day) DO UPDATE SET
             milliseconds = MAX(milliseconds, excluded.milliseconds), pages = MAX(pages, excluded.pages)",
            params![record.recording_id, record.day, record.milliseconds, record.pages],
        ).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub(crate) fn load_reading_activity(&self) -> Result<Vec<ActivityDay>, String> {
        let mut statement = self.connection.prepare(
            "SELECT day, SUM(milliseconds), SUM(pages) FROM reading_activity GROUP BY day ORDER BY day DESC"
        ).map_err(|error| error.to_string())?;
        statement
            .query_map([], |row| {
                Ok(ActivityDay {
                    day: row.get(0)?,
                    milliseconds: row.get(1)?,
                    pages: row.get(2)?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_activity_survives_reopen_and_out_of_order_retries() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Storage::open(directory.path()).unwrap();
        let mut record = ActivityRecord {
            recording_id: "01234567-0123-4123-8123-0123456789ab".to_owned(),
            day: "2026-09-23".to_owned(),
            milliseconds: 60_000,
            pages: 3,
        };
        assert!(record.valid());
        storage.save_reading_activity(&record).unwrap();
        storage.save_reading_activity(&record).unwrap();
        record.milliseconds = 10_000;
        record.pages = 1;
        storage.save_reading_activity(&record).unwrap();
        record.recording_id = "01234567-0123-4123-8123-0123456789ac".to_owned();
        storage.save_reading_activity(&record).unwrap();
        drop(storage);
        let storage = Storage::open(directory.path()).unwrap();
        let days = storage.load_reading_activity().unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].milliseconds, 70_000);
        assert_eq!(days[0].pages, 4);
    }
}
