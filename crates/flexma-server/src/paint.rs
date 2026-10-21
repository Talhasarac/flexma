//! Paint layers on the server (see `flexma_doc::raster`).
//!
//! The room orders brush strokes (`seq`) and relays them live; this module
//! stores them in order, keeps the tile snapshots browsers upload, and builds
//! a file's paint state for clients that open it: each tile's latest snapshot
//! plus the strokes painted on top of it since.
//!
//! Tiles have two snapshots: `base`, which strokes replay onto (it lags the
//! newest strokes so they can still be undone), and `view`, the latest look,
//! used by exports, thumbnails and agent screenshots.

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use flexma_doc::raster::{tiles_in, Stroke};
use flexma_doc::{Document, NodeType};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Base,
    View,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "base" => Some(Kind::Base),
            "view" => Some(Kind::View),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Kind::Base => "base",
            Kind::View => "view",
        }
    }
}

/// Node ids contain ':'; keep file names plain.
fn safe(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

pub fn layer_dir(data_dir: &Path, file_id: &str, layer: &str) -> PathBuf {
    data_dir.join("paint").join(safe(file_id)).join(safe(layer))
}

pub fn tile_path(data_dir: &Path, file_id: &str, layer: &str, tx: i32, ty: i32, kind: Kind) -> PathBuf {
    layer_dir(data_dir, file_id, layer).join(format!("{tx}_{ty}.{}.png", kind.as_str()))
}

/// Remove a deleted file's tiles from disk.
pub fn delete_file_tiles(data_dir: &Path, file_id: &str) {
    let _ = std::fs::remove_dir_all(data_dir.join("paint").join(safe(file_id)));
}

/// A duplicated file keeps its paint: copy the stroke log and tile rows
/// (node ids are the same in the copy).
pub fn copy_file_rows(c: &Connection, from: &str, to: &str) -> AppResult<()> {
    c.execute(
        "INSERT INTO paint_strokes (file_id, seq, id, layer_id, user_id, data, x0, y0, x1, y1, undone, created_at)
         SELECT ?2, seq, id, layer_id, user_id, data, x0, y0, x1, y1, undone, created_at FROM paint_strokes WHERE file_id = ?1",
        params![from, to],
    )?;
    c.execute(
        "INSERT INTO paint_tiles (file_id, layer_id, tx, ty, base_seq, view_seq, updated_at)
         SELECT ?2, layer_id, tx, ty, base_seq, view_seq, updated_at FROM paint_tiles WHERE file_id = ?1",
        params![from, to],
    )?;
    Ok(())
}

/// Copy a file's tile images to a duplicate.
pub fn copy_file_tiles(data_dir: &Path, from: &str, to: &str) {
    let (src, dst) = (data_dir.join("paint").join(safe(from)), data_dir.join("paint").join(safe(to)));
    let Ok(layers) = std::fs::read_dir(&src) else { return };
    for l in layers.flatten() {
        let Ok(tiles) = std::fs::read_dir(l.path()) else { continue };
        let dir = dst.join(l.file_name());
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        for t in tiles.flatten() {
            if t.path().extension().is_some_and(|e| e == "png") {
                let _ = std::fs::copy(t.path(), dir.join(t.file_name()));
            }
        }
    }
}

/// A layer's tiles for server-side renders: (column, row, path), preferring
/// the latest-look snapshot.
pub fn tile_files(data_dir: &Path, file_id: &str, layer: &str) -> Vec<(i32, i32, PathBuf)> {
    let dir = layer_dir(data_dir, file_id, layer);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: HashMap<(i32, i32), (bool, PathBuf)> = HashMap::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".png") else { continue };
        let Some((pos, kind)) = stem.rsplit_once('.') else { continue };
        let Some((x, y)) = pos.split_once('_') else { continue };
        let (Ok(x), Ok(y)) = (x.parse::<i32>(), y.parse::<i32>()) else { continue };
        let view = kind == "view";
        if !view && kind != "base" {
            continue;
        }
        match out.get(&(x, y)) {
            Some((true, _)) => {}
            _ => {
                out.insert((x, y), (view, e.path()));
            }
        }
    }
    let mut v: Vec<(i32, i32, PathBuf)> = out.into_iter().map(|((x, y), (_, p))| (x, y, p)).collect();
    v.sort_by_key(|(x, y, _)| (*y, *x));
    v
}

// ---------------------------------------------------------------------------
// Writing strokes in order
// ---------------------------------------------------------------------------

pub enum Write {
    Insert { file_id: String, seq: u64, user_id: String, stroke: Box<Stroke> },
    Undo { file_id: String, id: String, undone: bool },
}

/// One task per room applies stroke writes strictly in order, so an undo can
/// never land before the stroke it undoes.
pub fn spawn_writer(db: Db) -> mpsc::UnboundedSender<Write> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Write>();
    tokio::spawn(async move {
        while let Some(w) = rx.recv().await {
            let res = db
                .call(move |c| {
                    match w {
                        Write::Insert { file_id, seq, user_id, stroke } => {
                            let b = stroke.bounds();
                            let data = serde_json::to_string(&*stroke).unwrap_or_default();
                            c.execute(
                                "INSERT OR IGNORE INTO paint_strokes (file_id, seq, id, layer_id, user_id, data, x0, y0, x1, y1, created_at)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                                params![file_id, seq as i64, stroke.id, stroke.layer, user_id, data, b[0], b[1], b[2], b[3], now()],
                            )?;
                        }
                        Write::Undo { file_id, id, undone } => {
                            c.execute(
                                "UPDATE paint_strokes SET undone = ?1 WHERE file_id = ?2 AND id = ?3",
                                params![undone as i64, file_id, id],
                            )?;
                        }
                    }
                    Ok(())
                })
                .await;
            if let Err(e) = res {
                tracing::warn!("could not store a paint stroke: {}", e.message);
            }
        }
    });
    tx
}

/// The highest stroke number used in a file.
pub fn last_seq(c: &Connection, file_id: &str) -> AppResult<u64> {
    let v: Option<i64> = c.query_row("SELECT MAX(seq) FROM paint_strokes WHERE file_id = ?1", params![file_id], |r| r.get(0))?;
    Ok(v.unwrap_or(0).max(0) as u64)
}

// ---------------------------------------------------------------------------
// Paint state for clients
// ---------------------------------------------------------------------------

/// The document's paint layers and their sizes.
pub fn layers_of(doc: &Document) -> HashMap<String, (f64, f64)> {
    doc.nodes()
        .filter(|n| n.kind == NodeType::Paint && !flexma_doc::node::is_derived_id(&n.id))
        .map(|n| (n.id.clone(), (n.width, n.height)))
        .collect()
}

/// Everything a client needs to show a file's paint layers: per layer, its
/// tiles' snapshot numbers and the strokes some tile still has to replay.
pub fn state(c: &Connection, file_id: &str, layers: &HashMap<String, (f64, f64)>) -> AppResult<Value> {
    let mut tiles: HashMap<String, HashMap<(i32, i32), (Option<i64>, Option<i64>)>> = HashMap::new();
    {
        let mut stmt = c.prepare("SELECT layer_id, tx, ty, base_seq, view_seq FROM paint_tiles WHERE file_id = ?1")?;
        let rows = stmt.query_map(params![file_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i32>(1)?, r.get::<_, i32>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, Option<i64>>(4)?))
        })?;
        for row in rows {
            let (layer, x, y, base, view) = row?;
            if layers.contains_key(&layer) {
                tiles.entry(layer).or_default().insert((x, y), (base, view));
            }
        }
    }
    let mut strokes: HashMap<String, Vec<Value>> = HashMap::new();
    let mut seq = 0i64;
    {
        let mut stmt = c.prepare(
            "SELECT seq, layer_id, data, undone, x0, y0, x1, y1, user_id FROM paint_strokes WHERE file_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![file_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                [r.get::<_, f64>(4)? as f32, r.get::<_, f64>(5)? as f32, r.get::<_, f64>(6)? as f32, r.get::<_, f64>(7)? as f32],
                r.get::<_, Option<String>>(8)?,
            ))
        })?;
        for row in rows {
            let (s, layer, data, undone, b, user) = row?;
            seq = seq.max(s);
            let Some(&(w, h)) = layers.get(&layer) else { continue };
            let lt = tiles.get(&layer);
            // Needed if some tile it touches has no snapshot that already includes it.
            let needed = tiles_in(b, w, h).into_iter().any(|t| lt.and_then(|m| m.get(&t)).and_then(|(base, _)| *base).is_none_or(|base| base < s));
            if !needed {
                continue;
            }
            let Ok(stroke) = serde_json::from_str::<Value>(&data) else { continue };
            strokes.entry(layer).or_default().push(json!({ "seq": s, "undone": undone != 0, "user": user, "stroke": stroke }));
        }
    }
    let mut out = serde_json::Map::new();
    for id in layers.keys() {
        let t: Vec<Value> = tiles
            .get(id)
            .map(|m| m.iter().map(|((x, y), (base, view))| json!({ "x": x, "y": y, "base": base, "view": view })).collect())
            .unwrap_or_default();
        out.insert(id.clone(), json!({ "tiles": t, "strokes": strokes.remove(id).unwrap_or_default() }));
    }
    Ok(json!({ "seq": seq, "layers": out }))
}

/// Store an uploaded tile snapshot if it's newer than what's there. Returns
/// whether it was kept. `latest` is the room's last stroke number: a snapshot
/// claiming strokes past it would outrank every honest upload for good and
/// hide the strokes painted later, so it isn't kept.
pub fn save_tile(c: &Connection, data_dir: &Path, file_id: &str, layer: &str, tx: i32, ty: i32, kind: Kind, seq: i64, latest: i64, png: &[u8]) -> AppResult<bool> {
    if seq > latest {
        return Ok(false);
    }
    let current: Option<(Option<i64>, Option<i64>)> = rusqlite::OptionalExtension::optional(c.query_row(
        "SELECT base_seq, view_seq FROM paint_tiles WHERE file_id = ?1 AND layer_id = ?2 AND tx = ?3 AND ty = ?4",
        params![file_id, layer, tx, ty],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ))?;
    let have = current.and_then(|(b, v)| if kind == Kind::Base { b } else { v });
    let newer = match kind {
        Kind::Base => have.is_none_or(|h| seq > h),
        Kind::View => have.is_none_or(|h| seq >= h),
    };
    if !newer {
        return Ok(false);
    }
    let path = tile_path(data_dir, file_id, layer, tx, ty, kind);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(AppError::internal)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, png).map_err(AppError::internal)?;
    std::fs::rename(&tmp, &path).map_err(AppError::internal)?;
    let col = if kind == Kind::Base { "base_seq" } else { "view_seq" };
    c.execute(
        &format!(
            "INSERT INTO paint_tiles (file_id, layer_id, tx, ty, {col}, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(file_id, layer_id, tx, ty) DO UPDATE SET {col} = excluded.{col}, updated_at = excluded.updated_at"
        ),
        params![file_id, layer, tx, ty, seq, now()],
    )?;
    // A base snapshot is also the latest look until a newer view arrives.
    if kind == Kind::Base && current.and_then(|(_, v)| v).is_none_or(|v| v < seq) {
        let view = tile_path(data_dir, file_id, layer, tx, ty, Kind::View);
        let _ = std::fs::remove_file(&view);
        c.execute(
            "UPDATE paint_tiles SET view_seq = NULL WHERE file_id = ?1 AND layer_id = ?2 AND tx = ?3 AND ty = ?4",
            params![file_id, layer, tx, ty],
        )?;
    }
    c.execute("UPDATE files SET thumbnail_version = NULL WHERE id = ?1", params![file_id])?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_files_prefer_the_latest_look() {
        let dir = std::env::temp_dir().join(format!("flexma-paint-test-{}", std::process::id()));
        let l = layer_dir(&dir, "f1", "a:1");
        std::fs::create_dir_all(&l).unwrap();
        for name in ["0_0.base.png", "0_0.view.png", "1_0.base.png", "junk.txt", "2_x.view.png"] {
            std::fs::write(l.join(name), b"x").unwrap();
        }
        let t = tile_files(&dir, "f1", "a:1");
        assert_eq!(t.len(), 2);
        assert!(t[0].2.ends_with("0_0.view.png"));
        assert!(t[1].2.ends_with("1_0.base.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An upload claiming strokes the room hasn't numbered yet (say `i64::MAX`)
    /// would win against every later snapshot, so it isn't kept.
    #[tokio::test]
    async fn snapshots_cant_run_ahead_of_the_room() {
        let dir = std::env::temp_dir().join(format!("flexma-paint-seq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Db::open(&dir.join("t.db")).unwrap();
        let d = dir.clone();
        let kept = db
            .call(move |c| {
                c.execute_batch(
                    "INSERT INTO workspaces (id, name, created_at) VALUES ('w', 'W', 0);
                     INSERT INTO projects (id, workspace_id, name, created_at) VALUES ('p', 'w', 'P', 0);
                     INSERT INTO files (id, project_id, name, document, created_at, updated_at) VALUES ('f', 'p', 'F', '{}', 0, 0);",
                )?;
                let save = |seq| save_tile(c, &d, "f", "a:1", 0, 0, Kind::Base, seq, 5, b"png");
                Ok([save(i64::MAX)?, save(6)?, save(5)?, save(4)?])
            })
            .await
            .unwrap();
        assert_eq!(kept, [false, false, true, false]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
