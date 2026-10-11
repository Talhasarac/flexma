//! Workspace fonts: font files uploaded to a workspace and Google Fonts added
//! to it. Everyone in the workspace (and share-link guests) loads them in the
//! editor, and server-side renders (PNG exports, thumbnails, agent
//! screenshots) use them too, so a design looks the same everywhere.
//!
//! Google Fonts are downloaded once by this server and served from it, so
//! browsers never contact Google.

use crate::db::{new_id, now, ts};
use crate::error::{AppError, AppResult};
use crate::render::{base_fontdb, FontDb};
use crate::state::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// Always available: bundled with the web app and installed on the server.
pub const BUILTIN: &[&str] = &["Inter"];

/// Fonts most computers have. Fine to use, but they may differ per device and
/// aren't installed on the server, so exports fall back to Inter.
pub const WEB_SAFE: &[&str] = &[
    "system-ui", "Arial", "Helvetica", "Georgia", "Times New Roman", "Courier New", "Verdana", "Tahoma",
    "Trebuchet MS", "Impact", "Menlo", "Monaco",
];

const MAX_FAMILY_LEN: usize = 100;

pub fn fonts_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("fonts")
}

pub fn font_path(data_dir: &Path, id: &str, format: &str) -> PathBuf {
    fonts_dir(data_dir).join(format!("{id}.{format}"))
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// What a font file contains (one face).
#[derive(Debug, Clone)]
pub struct FaceInfo {
    pub family: String,
    pub weight: u16,
    /// Same as `weight` unless it's a variable font with a weight axis.
    pub weight_max: u16,
    pub italic: bool,
    pub format: &'static str,
}

/// Inspect an uploaded font file. Only single-face TrueType/OpenType files are
/// accepted, since both browsers and the server renderer can read them.
pub fn inspect(bytes: &[u8]) -> Result<FaceInfo, String> {
    if bytes.starts_with(b"wOF2") || bytes.starts_with(b"wOFF") {
        return Err("WOFF and WOFF2 files aren't supported; upload the .ttf or .otf version of the font".into());
    }
    if bytes.starts_with(b"ttcf") {
        return Err("font collections (.ttc) aren't supported; upload the individual .ttf or .otf files".into());
    }
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|_| "this isn't a valid .ttf or .otf font file".to_string())?;
    let name = |id: u16| -> Option<String> {
        let mut fallback = None;
        for n in face.names() {
            if n.name_id != id {
                continue;
            }
            if let Some(s) = n.to_string() {
                // Prefer the Windows US-English record.
                if n.platform_id == ttf_parser::PlatformId::Windows && n.language_id == 0x409 {
                    return Some(s);
                }
                fallback.get_or_insert(s);
            }
        }
        fallback
    };
    let family = name(ttf_parser::name_id::TYPOGRAPHIC_FAMILY)
        .or_else(|| name(ttf_parser::name_id::FAMILY))
        .map(|f| clean_family(&f))
        .filter(|f| !f.is_empty())
        .ok_or("the font has no family name")?;
    let weight = face.weight().to_number().clamp(1, 1000);
    let weight_max = face
        .variation_axes()
        .into_iter()
        .find(|a| a.tag == ttf_parser::Tag::from_bytes(b"wght"))
        .map(|a| (a.max_value.round() as u16).clamp(weight, 1000))
        .unwrap_or(weight);
    let weight = face
        .variation_axes()
        .into_iter()
        .find(|a| a.tag == ttf_parser::Tag::from_bytes(b"wght"))
        .map(|a| (a.min_value.round() as u16).clamp(1, weight))
        .unwrap_or(weight);
    let format = if bytes.starts_with(b"OTTO") { "otf" } else { "ttf" };
    Ok(FaceInfo { family, weight, weight_max, italic: face.is_italic() || face.is_oblique(), format })
}

/// Family names end up in CSS `font` strings; keep them plain.
pub fn clean_family(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | ';' | '\\' | '{' | '}'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_FAMILY_LEN)
        .collect()
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

pub fn list_json(c: &Connection, workspace_id: &str) -> AppResult<Vec<Value>> {
    let mut stmt = c.prepare(
        "SELECT f.id, f.family, f.weight, f.weight_max, f.italic, f.source, f.format, f.size, u.name, f.created_at
         FROM fonts f LEFT JOIN users u ON u.id = f.created_by
         WHERE f.workspace_id = ?1 ORDER BY f.family COLLATE NOCASE, f.italic, f.weight",
    )?;
    let rows = stmt
        .query_map(params![workspace_id], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "family": r.get::<_, String>(1)?,
                "weight": r.get::<_, i64>(2)?,
                "weightMax": r.get::<_, i64>(3)?,
                "italic": r.get::<_, i64>(4)? != 0,
                "source": r.get::<_, String>(5)?,
                "format": r.get::<_, String>(6)?,
                "size": r.get::<_, i64>(7)?,
                "createdBy": r.get::<_, Option<String>>(8)?,
                "createdAt": ts(r.get(9)?),
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Families available in a workspace.
pub fn families(c: &Connection, workspace_id: &str) -> AppResult<Vec<String>> {
    let mut stmt = c.prepare("SELECT DISTINCT family FROM fonts WHERE workspace_id = ?1 ORDER BY family COLLATE NOCASE")?;
    let rows = stmt.query_map(params![workspace_id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}

/// Insert a face, replacing an existing face with the same family, weight and
/// style. Returns (new id, replaced face's file path to delete).
pub fn insert_face(
    c: &Connection,
    data_dir: &Path,
    workspace_id: &str,
    user_id: &str,
    info: &FaceInfo,
    source: &str,
    size: usize,
) -> AppResult<(String, Option<PathBuf>)> {
    let old: Option<(String, String)> = c
        .query_row(
            "SELECT id, format FROM fonts WHERE workspace_id = ?1 AND family = ?2 AND weight = ?3 AND italic = ?4",
            params![workspace_id, info.family, info.weight, info.italic as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((old_id, _)) = &old {
        c.execute("DELETE FROM fonts WHERE id = ?1", params![old_id])?;
    }
    let id = new_id();
    c.execute(
        "INSERT INTO fonts (id, workspace_id, family, weight, weight_max, italic, source, format, size, created_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            workspace_id,
            info.family,
            info.weight,
            info.weight_max,
            info.italic as i64,
            source,
            info.format,
            size as i64,
            user_id,
            now()
        ],
    )?;
    Ok((id, old.map(|(id, fmt)| font_path(data_dir, &id, &fmt))))
}

/// Remove every face of a family. Returns the files to delete.
pub fn delete_family(c: &Connection, data_dir: &Path, workspace_id: &str, family: &str) -> AppResult<Vec<PathBuf>> {
    let mut stmt = c.prepare("SELECT id, format FROM fonts WHERE workspace_id = ?1 AND family = ?2")?;
    let files = stmt
        .query_map(params![workspace_id, family], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if files.is_empty() {
        return Err(AppError::not_found("font"));
    }
    c.execute("DELETE FROM fonts WHERE workspace_id = ?1 AND family = ?2", params![workspace_id, family])?;
    Ok(files.into_iter().map(|(id, fmt)| font_path(data_dir, &id, &fmt)).collect())
}

/// The workspace a file belongs to.
pub fn file_workspace(c: &Connection, file_id: &str) -> AppResult<String> {
    c.query_row(
        "SELECT p.workspace_id FROM files f JOIN projects p ON p.id = f.project_id WHERE f.id = ?1",
        params![file_id],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| AppError::not_found("file"))
}

// ---------------------------------------------------------------------------
// Server-side rendering
// ---------------------------------------------------------------------------

fn fontdb_cache() -> &'static Mutex<HashMap<String, Arc<FontDb>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<FontDb>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// System fonts plus the workspace's fonts, for resvg. Cached until the
/// workspace's fonts change. Font files are read on demand, not kept in memory.
pub async fn fontdb_for(st: &AppState, workspace_id: &str) -> Arc<FontDb> {
    if let Some(db) = fontdb_cache().lock().unwrap().get(workspace_id) {
        return db.clone();
    }
    let ws = workspace_id.to_string();
    let rows: Vec<(String, String)> = st
        .db
        .call(move |c| {
            let mut stmt = c.prepare("SELECT id, format FROM fonts WHERE workspace_id = ?1")?;
            let rows = stmt.query_map(params![ws], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap_or_default();
    let db = if rows.is_empty() {
        base_fontdb()
    } else {
        let data_dir = st.cfg.data_dir.clone();
        tokio::task::spawn_blocking(move || {
            let mut db = (*base_fontdb()).clone();
            for (id, format) in rows {
                if let Err(e) = db.load_font_file(font_path(&data_dir, &id, &format)) {
                    tracing::warn!("could not load font {id}: {e}");
                }
            }
            Arc::new(db)
        })
        .await
        .unwrap_or_else(|_| base_fontdb())
    };
    fontdb_cache().lock().unwrap().insert(workspace_id.to_string(), db.clone());
    db
}

/// After a workspace's fonts change: rebuild its render font database, redo
/// thumbnails, and tell open editors to reload fonts.
pub async fn fonts_changed(st: &AppState, workspace_id: &str) {
    fontdb_cache().lock().unwrap().remove(workspace_id);
    let ws = workspace_id.to_string();
    let files: Vec<String> = st
        .db
        .call(move |c| {
            c.execute(
                "UPDATE files SET thumbnail_version = NULL WHERE project_id IN (SELECT id FROM projects WHERE workspace_id = ?1)",
                params![ws],
            )?;
            let mut stmt = c.prepare("SELECT f.id FROM files f JOIN projects p ON p.id = f.project_id WHERE p.workspace_id = ?1")?;
            let ids = stmt.query_map(params![ws], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
            Ok(ids)
        })
        .await
        .unwrap_or_default();
    for f in files {
        st.rooms.notify_if_open(&f, json!({ "t": "fonts" })).await;
    }
}

// ---------------------------------------------------------------------------
// Google Fonts
// ---------------------------------------------------------------------------

const GOOGLE_METADATA_URL: &str = "https://fonts.google.com/metadata/fonts";
const GOOGLE_CSS_URL: &str = "https://fonts.googleapis.com/css2";
const GOOGLE_FILES_PREFIX: &str = "https://fonts.gstatic.com/";
const CATALOGUE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const MAX_FONT_BYTES: u64 = 10 * 1024 * 1024;

/// One family in the (slimmed-down) Google Fonts catalogue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleFamily {
    pub family: String,
    /// "Sans Serif", "Serif", "Display", "Handwriting" or "Monospace".
    pub category: String,
    /// Available styles: "400", "700", "400i", …
    pub styles: Vec<String>,
    /// Rank (1 = most popular).
    pub popularity: u32,
}

pub struct Catalogue {
    pub json: Arc<str>,
    pub families: Vec<GoogleFamily>,
}

impl Catalogue {
    pub fn find(&self, family: &str) -> Option<&GoogleFamily> {
        self.families.iter().find(|f| f.family.eq_ignore_ascii_case(family.trim()))
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

fn get_text(url: &str) -> Result<String, String> {
    agent()
        .get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

fn get_font(url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with(GOOGLE_FILES_PREFIX) {
        return Err(format!("unexpected font URL {url}"));
    }
    agent()
        .get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(MAX_FONT_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

fn slim_catalogue(raw: &str) -> Result<Vec<GoogleFamily>, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        family_metadata_list: Vec<RawFamily>,
    }
    #[derive(Deserialize)]
    struct RawFamily {
        family: String,
        category: Option<String>,
        fonts: HashMap<String, serde_json::Value>,
        popularity: Option<u32>,
    }
    let raw = raw.trim_start_matches(")]}'");
    let parsed: Raw = serde_json::from_str(raw).map_err(|e| format!("unexpected catalogue format: {e}"))?;
    let mut out: Vec<GoogleFamily> = parsed
        .family_metadata_list
        .into_iter()
        .map(|f| {
            let mut styles: Vec<String> = f.fonts.into_keys().collect();
            styles.sort_by_key(|s| (s.ends_with('i'), s.trim_end_matches('i').parse::<u16>().unwrap_or(400)));
            GoogleFamily {
                family: f.family,
                category: f.category.unwrap_or_else(|| "Sans Serif".into()),
                styles,
                popularity: f.popularity.unwrap_or(u32::MAX),
            }
        })
        .filter(|f| !f.styles.is_empty())
        .collect();
    out.sort_by_key(|f| f.popularity);
    Ok(out)
}

fn catalogue_cache() -> &'static Mutex<Option<(SystemTime, Arc<Catalogue>)>> {
    static C: OnceLock<Mutex<Option<(SystemTime, Arc<Catalogue>)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The Google Fonts catalogue: cached in memory and on disk for a week, so
/// the server only asks Google occasionally. A stale copy is used if Google
/// can't be reached.
pub async fn google_catalogue(st: &AppState) -> AppResult<Arc<Catalogue>> {
    if let Some((at, c)) = catalogue_cache().lock().unwrap().as_ref() {
        if at.elapsed().unwrap_or_default() < CATALOGUE_MAX_AGE {
            return Ok(c.clone());
        }
    }
    let path = st.cfg.data_dir.join("cache").join("google-fonts.json");
    let cat = tokio::task::spawn_blocking(move || -> Result<(SystemTime, Catalogue), String> {
        let from_disk = |p: &Path| -> Option<(SystemTime, Catalogue)> {
            let at = std::fs::metadata(p).ok()?.modified().ok()?;
            let text = std::fs::read_to_string(p).ok()?;
            let families: Vec<GoogleFamily> = serde_json::from_str(&text).ok()?;
            Some((at, Catalogue { json: Arc::from(text), families }))
        };
        if let Some((at, c)) = from_disk(&path) {
            if at.elapsed().unwrap_or_default() < CATALOGUE_MAX_AGE {
                return Ok((at, c));
            }
        }
        match get_text(GOOGLE_METADATA_URL).and_then(|raw| slim_catalogue(&raw)) {
            Ok(families) => {
                let text = serde_json::to_string(&families).map_err(|e| e.to_string())?;
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&path, &text);
                Ok((SystemTime::now(), Catalogue { json: Arc::from(text), families }))
            }
            Err(e) => {
                tracing::warn!("could not fetch the Google Fonts catalogue: {e}");
                from_disk(&path).ok_or(e)
            }
        }
    })
    .await
    .map_err(AppError::internal)?
    .map_err(|_| AppError::new(axum::http::StatusCode::SERVICE_UNAVAILABLE, "Google Fonts can't be reached from this server right now"))?;
    let (at, c) = cat;
    let c = Arc::new(c);
    *catalogue_cache().lock().unwrap() = Some((at, c.clone()));
    Ok(c)
}

fn encode_component(s: &str, space_as_plus: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' if space_as_plus => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The css2 URL asking for every style of a family (TrueType, since we send
/// no browser user agent).
fn css2_url(f: &GoogleFamily) -> String {
    let fam = encode_component(&f.family, true);
    let parse = |s: &str| (s.ends_with('i'), s.trim_end_matches('i').parse::<u16>().unwrap_or(400));
    let mut styles: Vec<(bool, u16)> = f.styles.iter().map(|s| parse(s)).collect();
    styles.sort();
    styles.dedup();
    if styles.iter().any(|(i, _)| *i) {
        let list: Vec<String> = styles.iter().map(|(i, w)| format!("{},{w}", *i as u8)).collect();
        format!("{GOOGLE_CSS_URL}?family={fam}:ital,wght@{}", list.join(";"))
    } else if styles == [(false, 400)] {
        format!("{GOOGLE_CSS_URL}?family={fam}")
    } else {
        let list: Vec<String> = styles.iter().map(|(_, w)| w.to_string()).collect();
        format!("{GOOGLE_CSS_URL}?family={fam}:wght@{}", list.join(";"))
    }
}

/// (italic, weight, url) for each `@font-face` in a Google Fonts stylesheet.
fn parse_css(css: &str) -> Vec<(bool, u16, String)> {
    let mut out = Vec::new();
    for block in css.split("@font-face").skip(1) {
        let block = block.split('}').next().unwrap_or("");
        let prop = |name: &str| -> Option<String> {
            let i = block.find(name)?;
            let rest = &block[i + name.len()..];
            let rest = rest.trim_start().trim_start_matches(':').trim_start();
            Some(rest.split(';').next()?.trim().to_string())
        };
        let italic = prop("font-style").map(|s| s == "italic").unwrap_or(false);
        let weight = prop("font-weight").and_then(|s| s.parse().ok()).unwrap_or(400);
        let url = block.find("url(").and_then(|i| {
            let rest = &block[i + 4..];
            Some(rest[..rest.find(')')?].trim_matches(|c| c == '\'' || c == '"').to_string())
        });
        if let Some(url) = url {
            out.push((italic, weight, url));
        }
    }
    out
}

/// Download every style of a Google Fonts family: (face info, bytes).
pub fn download_family(f: &GoogleFamily) -> Result<Vec<(FaceInfo, Vec<u8>)>, String> {
    let css = get_text(&css2_url(f))?;
    let faces = parse_css(&css);
    if faces.is_empty() {
        return Err(format!("Google Fonts returned no files for {}", f.family));
    }
    let mut out = Vec::new();
    for (italic, weight, url) in faces.into_iter().take(40) {
        let bytes = get_font(&url)?;
        let parsed = inspect(&bytes)?;
        out.push((
            FaceInfo { family: f.family.clone(), weight, weight_max: weight, italic, format: parsed.format },
            bytes,
        ));
    }
    Ok(out)
}

/// A tiny font containing just the glyphs of the family's name, for previews
/// in the font picker. Cached on disk.
pub async fn google_preview(st: &AppState, family: &str) -> AppResult<Vec<u8>> {
    let cat = google_catalogue(st).await?;
    let f = cat.find(family).ok_or_else(|| AppError::not_found("Google font"))?.clone();
    let dir = st.cfg.data_dir.join("cache").join("google-previews");
    let path = dir.join(format!("{}.ttf", hex::encode(sha2::Sha256::digest(f.family.as_bytes()))));
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return Ok(bytes);
    }
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let url = format!(
            "{GOOGLE_CSS_URL}?family={}&text={}",
            encode_component(&f.family, true),
            encode_component(&f.family, false)
        );
        let css = get_text(&url)?;
        let (_, _, font_url) = parse_css(&css).into_iter().next().ok_or("no preview font")?;
        get_font(&font_url)
    })
    .await
    .map_err(AppError::internal)?
    .map_err(|e| AppError::new(axum::http::StatusCode::BAD_GATEWAY, format!("couldn't load the preview from Google Fonts: {e}")))?;
    let _ = tokio::fs::create_dir_all(&dir).await;
    let _ = tokio::fs::write(&path, &bytes).await;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css2_urls() {
        let f = |styles: &[&str]| GoogleFamily {
            family: "Open Sans".into(),
            category: "Sans Serif".into(),
            styles: styles.iter().map(|s| s.to_string()).collect(),
            popularity: 1,
        };
        assert_eq!(css2_url(&f(&["400"])), "https://fonts.googleapis.com/css2?family=Open+Sans");
        assert_eq!(css2_url(&f(&["700", "400"])), "https://fonts.googleapis.com/css2?family=Open+Sans:wght@400;700");
        assert_eq!(
            css2_url(&f(&["400", "400i", "700"])),
            "https://fonts.googleapis.com/css2?family=Open+Sans:ital,wght@0,400;0,700;1,400"
        );
    }

    #[test]
    fn parses_google_css() {
        let css = "/* latin */\n@font-face {\n  font-family: 'Poppins';\n  font-style: italic;\n  font-weight: 700;\n  font-display: swap;\n  src: url(https://fonts.gstatic.com/s/poppins/v24/a.ttf) format('truetype');\n}\n@font-face {\n  font-family: 'Poppins';\n  font-style: normal;\n  font-weight: 400;\n  src: url(https://fonts.gstatic.com/s/poppins/v24/b.ttf) format('truetype');\n}\n";
        assert_eq!(
            parse_css(css),
            vec![
                (true, 700, "https://fonts.gstatic.com/s/poppins/v24/a.ttf".to_string()),
                (false, 400, "https://fonts.gstatic.com/s/poppins/v24/b.ttf".to_string()),
            ]
        );
    }

    #[test]
    fn cleans_family_names() {
        assert_eq!(clean_family("  My \"Font\";  Pro "), "My Font Pro");
    }
}
