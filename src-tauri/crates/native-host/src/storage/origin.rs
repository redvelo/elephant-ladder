//! Durable browser-origin identity for imported publications.
//!
//! Each library item owns one high-entropy `.localhost` hostname that survives
//! duplicate import and compatible snapshot replacement. Deletion retires an
//! allocation before app-owned state is detached, and retired identifiers are
//! retained rather than reused.

use super::random_hex;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub(crate) fn allocate_browser_origin(
    transaction: &Transaction<'_>,
    publication_id: &str,
) -> Result<String, String> {
    if let Some(origin_id) = transaction
        .query_row(
            "SELECT origin_id FROM browser_origin_allocations
             WHERE publication_id = ?1 AND lifecycle_state = 'active'",
            [publication_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("could not inspect publication browser origin: {error}"))?
    {
        if !is_origin_id(&origin_id) {
            return Err("publication browser origin is invalid".to_owned());
        }
        return Ok(origin_id);
    }
    for _ in 0..8 {
        let origin_id = random_hex(16)?;
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO browser_origin_allocations
                    (origin_id, publication_id, lifecycle_state)
                 VALUES (?1, ?2, 'active')",
                params![origin_id, publication_id],
            )
            .map_err(|error| format!("could not allocate publication browser origin: {error}"))?;
        if inserted == 1 {
            return Ok(origin_id);
        }
        if active_browser_origin_id(transaction, publication_id).is_ok() {
            return Err("publication already has an active browser origin".to_owned());
        }
    }
    Err("could not allocate unique publication browser origin".to_owned())
}

pub(crate) fn active_browser_origin_id(
    transaction: &Transaction<'_>,
    publication_id: &str,
) -> Result<String, String> {
    let origin_id = transaction
        .query_row(
            "SELECT origin_id FROM browser_origin_allocations
             WHERE publication_id = ?1 AND lifecycle_state = 'active'",
            [publication_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("could not load publication browser origin: {error}"))?
        .ok_or_else(|| "publication has no active browser origin".to_owned())?;
    if !is_origin_id(&origin_id) {
        return Err("publication browser origin is invalid".to_owned());
    }
    Ok(origin_id)
}

pub(crate) fn validate_browser_origin_state(connection: &Connection) -> Result<(), String> {
    let configurations = connection
        .query_row(
            "SELECT COUNT(*) FROM installation_origin_config",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| {
            format!("could not validate installation origin configuration: {error}")
        })?;
    if configurations > 1 {
        return Err("installation origin configuration is invalid".to_owned());
    }
    if configurations == 1 {
        let (scheme_version, foreground_port, reader_port) = connection
            .query_row(
                "SELECT scheme_version, foreground_port, reader_port
                 FROM installation_origin_config",
                [],
                |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, Option<u32>>(2)?,
                    ))
                },
            )
            .map_err(|error| {
                format!("could not read installation origin configuration: {error}")
            })?;
        if scheme_version != 1
            || !(1..=u16::MAX as u32).contains(&foreground_port)
            || reader_port.is_some_and(|port| {
                !(1..=u16::MAX as u32).contains(&port) || port == foreground_port
            })
        {
            return Err("installation origin configuration is invalid".to_owned());
        }
    }

    let invalid_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM browser_origin_allocations o
             LEFT JOIN publications p ON p.publication_id = o.publication_id
             WHERE length(o.origin_id) != 32
                OR o.origin_id GLOB '*[^0-9a-f]*'
                OR (o.lifecycle_state = 'active' AND
                    (o.publication_id IS NULL OR p.deletion_state IS NULL OR p.deletion_state != 'live'))
                OR (o.lifecycle_state = 'active') != (o.retired_at IS NULL)",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| format!("could not validate browser origin allocations: {error}"))?;
    if invalid_count != 0 {
        return Err("browser origin allocation state is invalid".to_owned());
    }
    let missing_count = connection
        .query_row(
            "SELECT COUNT(*) FROM publications p
             WHERE p.deletion_state = 'live'
               AND NOT EXISTS (
                    SELECT 1 FROM browser_origin_allocations o
                    WHERE o.publication_id = p.publication_id AND o.lifecycle_state = 'active'
               )",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| format!("could not validate live publication browser origins: {error}"))?;
    if missing_count != 0 {
        return Err("live publication has no active browser origin".to_owned());
    }
    Ok(())
}

pub(crate) fn is_origin_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
