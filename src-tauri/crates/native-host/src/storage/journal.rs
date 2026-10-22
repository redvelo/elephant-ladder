use super::Storage;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReadingSummary {
    pub chapter: Option<String>,
    pub pages: Option<ReadingPages>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadingPages {
    current: u32,
    total: u32,
    accuracy: PageAccuracy,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum PageAccuracy {
    Exact,
    Calibrated,
}

impl ReadingSummary {
    pub fn valid(&self) -> bool {
        self.chapter
            .as_ref()
            .is_none_or(|value| value.len() <= 4096)
            && self
                .pages
                .as_ref()
                .is_none_or(|pages| pages.current > 0 && pages.current <= pages.total)
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Reflection {
    pub id: String,
    pub text: String,
    pub prompt: u32,
    pub context: Option<ReadingSummary>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Debug)]
pub(crate) struct Journal {
    pub summary: Option<ReadingSummary>,
    pub draft: Option<Reflection>,
    pub entries: Vec<Reflection>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum JournalMutation {
    Begin,
    Update { id: String, text: String },
    Publish { id: String },
    Delete { id: String },
}

#[derive(Debug)]
pub(crate) enum JournalError {
    Missing,
    Invalid,
    Storage,
}

impl From<rusqlite::Error> for JournalError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}
impl From<serde_json::Error> for JournalError {
    fn from(_: serde_json::Error) -> Self {
        Self::Storage
    }
}

impl Storage {
    pub(crate) fn load_journal(&self, publication_id: &str) -> Result<Journal, JournalError> {
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM publications WHERE publication_id = ?1 AND deletion_state = 'live')",
            [publication_id], |row| row.get(0),
        )?;
        if !exists {
            return Err(JournalError::Missing);
        }
        let summary_json: Option<String> = self
            .connection
            .query_row(
                "SELECT summary_json FROM reading_positions WHERE publication_id = ?1",
                [publication_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let mut journal = Journal {
            summary: summary_json
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
            draft: None,
            entries: Vec::new(),
        };
        let mut statement = self.connection.prepare(
            "SELECT id, text, prompt, draft, context_json, created_at, updated_at FROM reflections WHERE publication_id = ?1 ORDER BY created_at DESC, rowid DESC"
        )?;
        let rows = statement.query_map([publication_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        for row in rows {
            let (id, text, prompt, draft, context, created_at, updated_at) = row?;
            let reflection = Reflection {
                id,
                text,
                prompt,
                context: context
                    .map(|json| serde_json::from_str(&json))
                    .transpose()?,
                created_at,
                updated_at,
            };
            if draft {
                journal.draft = Some(reflection);
            } else {
                journal.entries.push(reflection);
            }
        }
        Ok(journal)
    }

    pub(crate) fn mutate_journal(
        &mut self,
        publication_id: &str,
        mutation: JournalMutation,
    ) -> Result<Journal, JournalError> {
        self.load_journal(publication_id)?;
        let transaction = self.connection.transaction()?;
        match mutation {
            JournalMutation::Begin => {
                transaction.execute(
                    "INSERT INTO reflections (id, publication_id, prompt, draft, context_json, position_json)
                     SELECT lower(hex(randomblob(16))), ?1,
                        COALESCE((SELECT (prompt + 1) % 5 FROM reflections WHERE publication_id = ?1 ORDER BY rowid DESC LIMIT 1), 0),
                        1, (SELECT summary_json FROM reading_positions WHERE publication_id = ?1),
                        (SELECT position_json FROM reading_positions WHERE publication_id = ?1)
                     WHERE NOT EXISTS(SELECT 1 FROM reflections WHERE publication_id = ?1 AND draft = 1)", [publication_id],
                )?;
            }
            JournalMutation::Update { id, text } => {
                if text.len() > 20_000 {
                    return Err(JournalError::Invalid);
                }
                let changed = transaction.execute(
                    "UPDATE reflections SET text = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE publication_id = ?1 AND id = ?2 AND (draft = 1 OR ?4)",
                    params![publication_id, id, text, !text.trim().is_empty()],
                )?;
                if changed != 1 {
                    return Err(JournalError::Invalid);
                }
            }
            JournalMutation::Publish { id } => {
                let text: Option<String> = transaction.query_row(
                    "SELECT text FROM reflections WHERE publication_id = ?1 AND id = ?2 AND draft = 1", params![publication_id, id], |row| row.get(0),
                ).optional()?;
                if text.is_none_or(|text| text.trim().is_empty()) {
                    return Err(JournalError::Invalid);
                }
                transaction.execute("UPDATE reflections SET draft = 0, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE publication_id = ?1 AND id = ?2", params![publication_id, id])?;
            }
            JournalMutation::Delete { id } => {
                if transaction.execute(
                    "DELETE FROM reflections WHERE publication_id = ?1 AND id = ?2",
                    params![publication_id, id],
                )? != 1
                {
                    return Err(JournalError::Missing);
                }
            }
        }
        transaction.commit()?;
        self.load_journal(publication_id)
    }
}
