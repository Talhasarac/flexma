//! Where a file's document is stored: as parts (see `flexma_doc::parts`),
//! which rooms save one by one, or as one JSON string in `files.document`.
//! The parts are the document while `files.parts_version` equals
//! `files.version`. Files only ever saved whole (new files, restored
//! versions, or saves by an older server) use the column.

use crate::error::{AppError, AppResult};
use rusqlite::{params, Connection, OptionalExtension};

pub struct Stored {
    pub json: String,
    pub version: i64,
    /// The stored parts, `(name, JSON)`, when the document came from them.
    pub parts: Vec<(String, String)>,
}

pub fn read(c: &Connection, file_id: &str) -> AppResult<Option<Stored>> {
    let row: Option<(String, i64, Option<i64>)> = c
        .query_row("SELECT document, version, parts_version FROM files WHERE id = ?1", params![file_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((whole, version, parts_version)) = row else { return Ok(None) };
    if parts_version == Some(version) {
        let mut stmt = c.prepare("SELECT part, document FROM file_parts WHERE file_id = ?1")?;
        let mut parts: Vec<(String, String)> = stmt.query_map(params![file_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        if !parts.is_empty() {
            // The shared part first: it holds the pages the others hang from.
            parts.sort_by_key(|(name, _)| name != flexma_doc::parts::SHARED);
            let json = flexma_doc::parts::assemble(parts.iter().map(|(_, j)| j.as_str()));
            return Ok(Some(Stored { json, version, parts }));
        }
    }
    Ok(Some(Stored { json: whole, version, parts: Vec::new() }))
}

/// The file's document as one JSON string.
pub fn read_json(c: &Connection, file_id: &str) -> AppResult<String> {
    Ok(read(c, file_id)?.ok_or_else(|| AppError::not_found("file"))?.json)
}

/// Save a room's document: write the parts in `write`, drop stored parts
/// that are no longer among `all`, and make them the document at `version`.
pub fn write_parts(c: &Connection, file_id: &str, version: i64, write: &[(&str, &str)], all: &[&str]) -> rusqlite::Result<()> {
    for (part, json) in write {
        c.execute(
            "INSERT INTO file_parts (file_id, part, document) VALUES (?1, ?2, ?3)
             ON CONFLICT(file_id, part) DO UPDATE SET document = excluded.document",
            params![file_id, part, json],
        )?;
    }
    let stored: Vec<String> = c.prepare("SELECT part FROM file_parts WHERE file_id = ?1")?.query_map(params![file_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    for part in stored.iter().filter(|p| !all.contains(&p.as_str())) {
        c.execute("DELETE FROM file_parts WHERE file_id = ?1 AND part = ?2", params![file_id, part])?;
    }
    c.execute(
        "UPDATE files SET version = ?2, parts_version = ?2, updated_at = ?3 WHERE id = ?1",
        params![file_id, version, crate::db::now()],
    )?;
    Ok(())
}

/// Also keep the whole document in `files.document`, so an older server can
/// still open the file. Rooms do it when they close; skipped when the file
/// moved on since (another version was restored).
pub fn write_whole(c: &Connection, file_id: &str, json: &str, version: i64) -> rusqlite::Result<()> {
    c.execute("UPDATE files SET document = ?1 WHERE id = ?2 AND version = ?3 AND parts_version = ?3", params![json, file_id, version])?;
    Ok(())
}
