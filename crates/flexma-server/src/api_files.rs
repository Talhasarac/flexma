//! Files, exports, thumbnails and image assets.

use crate::auth::{file_access, project_access, require_workspace, AuthUser, Role};
use crate::db::{new_id, now, ts};
use crate::error::{AppError, AppResult};
use crate::render;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use flexma_doc::{Document, NodeId, NodeType};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

/// SVGs we serve contain user text; forbid scripts and remote loads outright.
const SVG_CSP: &str = "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; font-src 'self' data:";

const FILE_SELECT: &str = "SELECT f.id, f.name, f.project_id, p.name, p.workspace_id, m.role, f.created_at, f.updated_at, u.name, f.kind
    FROM files f JOIN projects p ON p.id = f.project_id
    JOIN workspace_members m ON m.workspace_id = p.workspace_id AND m.user_id = ?1
    LEFT JOIN users u ON u.id = f.created_by";

fn file_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, String>(0)?,
        "name": r.get::<_, String>(1)?,
        "projectId": r.get::<_, String>(2)?,
        "projectName": r.get::<_, String>(3)?,
        "workspaceId": r.get::<_, String>(4)?,
        "role": r.get::<_, String>(5)?,
        "createdAt": ts(r.get(6)?),
        "updatedAt": ts(r.get(7)?),
        "createdBy": r.get::<_, Option<String>>(8)?.unwrap_or_default(),
        "kind": r.get::<_, String>(9)?,
    }))
}

pub fn get_file_json(c: &rusqlite::Connection, user_id: &str, file_id: &str) -> AppResult<Value> {
    c.query_row(&format!("{FILE_SELECT} WHERE f.id = ?2"), params![user_id, file_id], file_json)
        .optional()?
        .ok_or_else(|| AppError::not_found("file"))
}

/// Create a file row with an empty document (or the given one). Returns the id.
/// What a file is for: `design` (screens and UI) or `painting`, which shows
/// the brush and eraser and starts with a canvas to paint on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FileKind {
    #[default]
    Design,
    Painting,
}

impl FileKind {
    pub fn parse(s: &str) -> AppResult<FileKind> {
        match s {
            "design" => Ok(FileKind::Design),
            "painting" => Ok(FileKind::Painting),
            _ => Err(AppError::bad_request("kind must be \"design\" or \"painting\"")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Design => "design",
            FileKind::Painting => "painting",
        }
    }
}

/// A new painting file: a white 1920×1080 canvas frame with a paint layer.
fn painting_document() -> Document {
    use flexma_doc::{Color, Node, NodeType, Op, Paint};
    let mut doc = Document::new_empty();
    let mut canvas = Node::new("1:1", NodeType::Frame);
    canvas.name = "Canvas".into();
    canvas.parent = Some("0:1".into());
    (canvas.width, canvas.height) = (1920.0, 1080.0);
    canvas.fills = vec![Paint::solid(Color::WHITE)];
    canvas.clips_content = true;
    canvas.index = flexma_doc::fractional::key_between(None, None);
    let mut layer = Node::new("1:2", NodeType::Paint);
    layer.name = "Paint layer 1".into();
    layer.parent = Some("1:1".into());
    (layer.width, layer.height) = (1920.0, 1080.0);
    layer.fills = vec![];
    layer.index = flexma_doc::fractional::key_between(None, None);
    for node in [canvas, layer] {
        let _ = doc.apply(&Op::Create { node });
    }
    doc
}

pub fn insert_file(c: &rusqlite::Connection, project_id: &str, name: &str, user_id: &str, doc_json: Option<String>, kind: FileKind) -> AppResult<String> {
    let id = new_id();
    let doc = doc_json.unwrap_or_else(|| match kind {
        FileKind::Design => Document::new_empty().to_json(),
        FileKind::Painting => painting_document().to_json(),
    });
    let t = now();
    c.execute(
        "INSERT INTO files (id, project_id, name, document, version, created_by, created_at, updated_at, kind)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?6, ?7)",
        params![id, project_id, name, doc, user_id, t, kind.as_str()],
    )?;
    Ok(id)
}

pub async fn list_project_files(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let rows = st
        .db
        .call(move |c| {
            project_access(c, &user.id, &id, Role::Viewer)?;
            let mut stmt = c.prepare(&format!("{FILE_SELECT} WHERE f.project_id = ?2 ORDER BY f.updated_at DESC"))?;
            let rows = stmt.query_map(params![user.id, id], file_json)?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(Value::Array(rows)))
}

pub async fn list_workspace_files(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let rows = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &id, Role::Viewer)?;
            let mut stmt =
                c.prepare(&format!("{FILE_SELECT} WHERE p.workspace_id = ?2 ORDER BY f.updated_at DESC LIMIT 50"))?;
            let rows = stmt.query_map(params![user.id, id], file_json)?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize)]
pub struct CreateFile {
    name: Option<String>,
    /// "design" (default) or "painting".
    kind: Option<String>,
}

fn clean_file_name(n: Option<String>) -> AppResult<String> {
    let n = n.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Untitled".into());
    if n.len() > 200 {
        return Err(AppError::bad_request("file name is too long"));
    }
    Ok(n)
}

pub async fn create_file(
    State(st): State<AppState>,
    user: AuthUser,
    Path(project): Path<String>,
    Json(req): Json<CreateFile>,
) -> AppResult<Json<Value>> {
    let name = clean_file_name(req.name)?;
    let kind = req.kind.as_deref().map(FileKind::parse).transpose()?.unwrap_or_default();
    let v = st
        .db
        .call(move |c| {
            project_access(c, &user.id, &project, Role::Editor)?;
            let id = insert_file(c, &project, &name, &user.id, None, kind)?;
            get_file_json(c, &user.id, &id)
        })
        .await?;
    Ok(Json(v))
}

pub async fn get_file(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let v = st.db.call(move |c| get_file_json(c, &user.id, &id)).await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFile {
    name: Option<String>,
    project_id: Option<String>,
    /// Switch between a design and a painting file.
    kind: Option<String>,
}

pub async fn update_file(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateFile>,
) -> AppResult<Json<Value>> {
    let kind = req.kind.as_deref().map(FileKind::parse).transpose()?;
    let v = st
        .db
        .call(move |c| {
            let (ws, _, _) = file_access(c, &user.id, &id, Role::Editor)?;
            if let Some(n) = req.name {
                let n = clean_file_name(Some(n))?;
                c.execute("UPDATE files SET name = ?1, updated_at = ?2 WHERE id = ?3", params![n, now(), id])?;
            }
            if let Some(p) = req.project_id {
                let (pws, _) = project_access(c, &user.id, &p, Role::Editor)?;
                if pws != ws {
                    return Err(AppError::bad_request("files can only be moved between projects of the same workspace"));
                }
                c.execute("UPDATE files SET project_id = ?1 WHERE id = ?2", params![p, id])?;
            }
            if let Some(k) = kind {
                c.execute("UPDATE files SET kind = ?1 WHERE id = ?2", params![k.as_str(), id])?;
            }
            get_file_json(c, &user.id, &id)
        })
        .await?;
    Ok(Json(v))
}

pub async fn delete_file(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let fid = id.clone();
    st.db
        .call(move |c| {
            file_access(c, &user.id, &fid, Role::Editor)?;
            c.execute("DELETE FROM files WHERE id = ?1", params![fid])?;
            Ok(())
        })
        .await?;
    st.rooms.close(&id, Some("This file was deleted.".into()));
    crate::paint::delete_file_tiles(&st.cfg.data_dir, &id);
    Ok(Json(json!({})))
}

/// Latest document JSON: from the live room when open, else from the database.
pub async fn current_doc_json(st: &AppState, file_id: &str) -> AppResult<String> {
    let room = st.rooms.get(file_id).await?;
    room.read(|d, _| d.to_json()).await
}

pub async fn duplicate_file(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let uid = user.id.clone();
    let fid = id.clone();
    let (project, name, kind) = st
        .db
        .call(move |c| {
            let (_, project, _) = file_access(c, &uid, &fid, Role::Editor)?;
            let (name, kind): (String, String) = c.query_row("SELECT name, kind FROM files WHERE id = ?1", params![fid], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok((project, name, FileKind::parse(&kind).unwrap_or_default()))
        })
        .await?;
    let doc = current_doc_json(&st, &id).await?;
    let src = id.clone();
    let (v, new_id) = st
        .db
        .call(move |c| {
            let new_id = insert_file(c, &project, &format!("{name} (copy)"), &user.id, Some(doc), kind)?;
            crate::paint::copy_file_rows(c, &src, &new_id)?;
            Ok((get_file_json(c, &user.id, &new_id)?, new_id))
        })
        .await?;
    let data_dir = st.cfg.data_dir.clone();
    let _ = tokio::task::spawn_blocking(move || crate::paint::copy_file_tiles(&data_dir, &id, &new_id)).await;
    Ok(Json(v))
}

pub async fn get_document(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Response> {
    let fid = id.clone();
    st.db.call(move |c| file_access(c, &user.id, &fid, Role::Viewer)).await?;
    let json = current_doc_json(&st, &id).await?;
    Ok(([(header::CONTENT_TYPE, "application/json")], json).into_response())
}

const EMPTY_THUMB: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="200" viewBox="0 0 320 200"><rect width="320" height="200" fill="#f5f5f5"/></svg>"##;

pub async fn thumbnail(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Response> {
    let fid = id.clone();
    st.db.call(move |c| file_access(c, &user.id, &fid, Role::Viewer).map(|_| ())).await?;
    thumbnail_response(&st, &id).await
}

/// PNG thumbnail of a file's first page, rendered with its workspace's fonts
/// and cached until the file or the workspace's fonts change.
pub async fn thumbnail_response(st: &AppState, file_id: &str) -> AppResult<Response> {
    let fid = file_id.to_string();
    let (cached, cached_version, version, ws): (Option<Vec<u8>>, Option<i64>, i64, String) = st
        .db
        .call(move |c| {
            Ok(c.query_row(
                "SELECT f.thumbnail_png, f.thumbnail_version, f.version, p.workspace_id
                 FROM files f JOIN projects p ON p.id = f.project_id WHERE f.id = ?1",
                params![fid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?)
        })
        .await?;
    if cached_version == Some(version) {
        return Ok(thumbnail_image(cached));
    }
    let fid = file_id.to_string();
    let doc: String = st.db.call(move |c| Ok(c.query_row("SELECT document FROM files WHERE id = ?1", params![fid], |r| r.get(0))?)).await?;
    let png = render_thumbnail(st, file_id, &ws, doc).await?;
    let (fid, stored) = (file_id.to_string(), png.clone());
    st.db
        .call(move |c| {
            Ok(c.execute(
                "UPDATE files SET thumbnail_png = ?1, thumbnail_version = ?2 WHERE id = ?3 AND version = ?2",
                params![stored, version, fid],
            )?)
        })
        .await?;
    Ok(thumbnail_image(png))
}

/// The ids of a workspace's uploaded images: the only ones its files' server
/// renders may inline.
pub fn workspace_asset_ids(c: &rusqlite::Connection, workspace_id: &str) -> AppResult<std::collections::HashSet<String>> {
    let mut stmt = c.prepare("SELECT id FROM assets WHERE workspace_id = ?1")?;
    let ids = stmt.query_map(params![workspace_id], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?;
    Ok(ids)
}

/// Rasterize a stored document's first page (None when it's empty).
pub async fn render_thumbnail(st: &AppState, file_id: &str, workspace_id: &str, doc_json: String) -> AppResult<Option<Vec<u8>>> {
    let fonts = crate::fonts::fontdb_for(st, workspace_id).await;
    let ws = workspace_id.to_string();
    let allowed = st.db.call(move |c| workspace_asset_ids(c, &ws)).await?;
    let data_dir = st.cfg.data_dir.clone();
    let fid = file_id.to_string();
    render::run_render(move || {
        let paint = render::PaintSource { data_dir: &data_dir, file_id: &fid, share_token: None };
        render::load_document(&doc_json).and_then(|d| render::thumbnail_png(&d, &data_dir, &allowed, fonts, &paint))
    })
    .await
    .map_err(AppError::internal)
}

pub fn thumbnail_image(png: Option<Vec<u8>>) -> Response {
    match png {
        Some(png) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=5")], png).into_response(),
        None => (
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "private, max-age=5"),
                (header::CONTENT_SECURITY_POLICY, SVG_CSP),
            ],
            EMPTY_THUMB,
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
pub struct ExportQuery {
    node: Option<String>,
    page: Option<String>,
    scale: Option<f32>,
    background: Option<bool>,
}

/// Resolve export targets: a node, a page, or the first page.
pub fn export_targets(doc: &Document, node: Option<&str>, page: Option<&str>) -> Result<(Vec<NodeId>, Option<flexma_doc::Color>), String> {
    if let Some(n) = node {
        let nn = doc.get(n).ok_or_else(|| format!("node '{n}' not found"))?;
        if nn.kind == NodeType::Page {
            return Ok((vec![n.to_string()], Some(render::page_background(doc, n))));
        }
        return Ok((vec![n.to_string()], None));
    }
    let page = match page {
        Some(p) => {
            if doc.get(p).map(|n| n.kind) != Some(NodeType::Page) {
                return Err(format!("page '{p}' not found"));
            }
            p.to_string()
        }
        None => doc.pages().first().cloned().ok_or("document has no pages")?,
    };
    let bg = render::page_background(doc, &page);
    Ok((vec![page], Some(bg)))
}

pub async fn export_svg(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<ExportQuery>,
) -> AppResult<Response> {
    let fid = id.clone();
    st.db.call(move |c| file_access(c, &user.id, &fid, Role::Viewer)).await?;
    export_svg_response(&st, &id, q, None).await
}

/// SVG export of a file (access already checked). `share_token` makes image
/// references resolvable for anonymous viewers.
pub async fn export_svg_response(st: &AppState, id: &str, q: ExportQuery, share_token: Option<String>) -> AppResult<Response> {
    let room = st.rooms.get(id).await?;
    let with_bg = q.background.unwrap_or(true);
    let fid = id.to_string();
    let ws = st.db.call(move |c| crate::fonts::file_workspace(c, &fid)).await?;
    let fonts = crate::fonts::fontdb_for(st, &ws).await;
    let (data_dir, pfid) = (st.cfg.data_dir.clone(), id.to_string());
    let svg = room
        .read(move |doc, _| {
            let (ids, bg) = export_targets(doc, q.node.as_deref(), q.page.as_deref())?;
            let mode = match &share_token {
                Some(t) => render::AssetMode::Shared(t),
                None => render::AssetMode::Url,
            };
            let paint = render::PaintSource { data_dir: &data_dir, file_id: &pfid, share_token: share_token.as_deref() };
            render::export_svg(doc, &ids, if with_bg { bg } else { None }, mode, Some(&fonts), &paint)
                .map(|(s, _)| s)
                .ok_or_else(|| "nothing to export".to_string())
        })
        .await?
        .map_err(AppError::bad_request)?;
    Ok(([(header::CONTENT_TYPE, "image/svg+xml"), (header::CONTENT_SECURITY_POLICY, SVG_CSP)], svg).into_response())
}

pub async fn export_png(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<ExportQuery>,
) -> AppResult<Response> {
    let fid = id.clone();
    st.db.call(move |c| file_access(c, &user.id, &fid, Role::Viewer)).await?;
    export_png_response(&st, &id, q, st.cfg.max_export_px).await
}

/// PNG export of a file (access already checked), longest side ≤ `max_dim`.
pub async fn export_png_response(st: &AppState, id: &str, q: ExportQuery, max_dim: u32) -> AppResult<Response> {
    let room = st.rooms.get(id).await?;
    let data_dir = st.cfg.data_dir.clone();
    let fid = id.to_string();
    let (ws, allowed) = st
        .db
        .call(move |c| {
            let ws = crate::fonts::file_workspace(c, &fid)?;
            let allowed = workspace_asset_ids(c, &ws)?;
            Ok((ws, allowed))
        })
        .await?;
    let fonts = crate::fonts::fontdb_for(st, &ws).await;
    let scale = q.scale.unwrap_or(2.0);
    let with_bg = q.background.unwrap_or(true);
    let measure_fonts = fonts.clone();
    let pfid = id.to_string();
    let images = (data_dir.clone(), allowed.clone());
    let (svg, linear) = room
        .read(move |doc, _| {
            let (ids, bg) = export_targets(doc, q.node.as_deref(), q.page.as_deref())?;
            let linear = render::linear_for(doc, &ids);
            let paint = render::PaintSource { data_dir: &data_dir, file_id: &pfid, share_token: None };
            render::export_svg(doc, &ids, if with_bg { bg } else { None }, render::AssetMode::Inline(&data_dir, &allowed), Some(&measure_fonts), &paint)
                .map(|(s, _)| (s, linear))
                .ok_or_else(|| "nothing to export".to_string())
        })
        .await?
        .map_err(AppError::bad_request)?;
    let (png, _, _) = render::run_render(move || render::svg_to_png(&svg, scale, max_dim, fonts, linear, Some((&images.0, &images.1))))
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::bad_request)?;
    Ok(([(header::CONTENT_TYPE, "image/png")], png).into_response())
}

pub async fn upload_asset(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> AppResult<Json<Value>> {
    let mime = check_image(&st, &body)?;
    let (fid, uid) = (id.clone(), user.id.clone());
    let ws = st.db.call(move |c| Ok(file_access(c, &uid, &fid, Role::Editor)?.0)).await?;
    Ok(Json(store_upload(&st, &ws, &id, &user.id, mime, &body).await?))
}

/// The image's type, or why it can't be used.
fn check_image(st: &AppState, body: &[u8]) -> AppResult<&'static str> {
    if body.is_empty() {
        return Err(AppError::bad_request("empty upload"));
    }
    if body.len() > st.cfg.max_upload_bytes {
        return Err(AppError::new(StatusCode::PAYLOAD_TOO_LARGE, "image is too large"));
    }
    if is_svg(body) {
        return Ok("image/svg+xml");
    }
    render::sniff_mime(body).ok_or_else(|| AppError::bad_request("unsupported image type (use PNG, JPEG, GIF, WebP or SVG)"))
}

/// Whether an upload is an SVG document.
fn is_svg(body: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&body[..body.len().min(4096)]);
    let t = head.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with('<')) && head.contains("<svg")
}

/// Save an uploaded image; an SVG is drawn to a PNG first, since image fills
/// are bitmaps everywhere (editor, exports, Unity). It reports the SVG's own
/// size, so a node made from it gets the drawing's proportions and size.
async fn store_upload(st: &AppState, ws: &str, file_id: &str, user_id: &str, mime: &str, body: &[u8]) -> AppResult<Value> {
    if mime != "image/svg+xml" {
        return store_image(st, ws, file_id, user_id, mime, body).await;
    }
    let svg = std::str::from_utf8(body).map_err(|_| AppError::bad_request("the SVG isn't UTF-8 text"))?.to_string();
    if crate::svg_import::declares_entities(&svg) {
        return Err(AppError::bad_request("SVGs that declare XML entities (<!ENTITY>) aren't supported; export the SVG again without them"));
    }
    let fonts = crate::fonts::fontdb_for(st, ws).await;
    let (png, w, h) = render::run_render(move || render::rasterize_svg(&svg, fonts))
        .await
        .map_err(AppError::internal)?
        .map_err(|e| AppError::bad_request(format!("couldn't draw the SVG: {e}")))?;
    let mut asset = store_image(st, ws, file_id, user_id, "image/png", &png).await?;
    asset["pixelWidth"] = asset["width"].clone();
    asset["pixelHeight"] = asset["height"].clone();
    asset["width"] = json!(w);
    asset["height"] = json!(h);
    asset["from"] = json!("svg");
    Ok(asset)
}

/// Save an image to a workspace's assets (access already checked).
async fn store_image(st: &AppState, ws: &str, file_id: &str, user_id: &str, mime: &str, body: &[u8]) -> AppResult<Value> {
    let (w, h) = imagesize::blob_size(body).map(|s| (s.width as i64, s.height as i64)).unwrap_or((0, 0));
    // The same picture again (a re-export, a second upload) reuses its asset.
    let hash = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(body))
    };
    let (ws2, hash2) = (ws.to_string(), hash.clone());
    let existing: Option<(String, String)> = st
        .db
        .call(move |c| {
            Ok(c.query_row("SELECT id, mime FROM assets WHERE workspace_id = ?1 AND sha256 = ?2 ORDER BY created_at LIMIT 1", params![ws2, hash2], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?)
        })
        .await?;
    if let Some((aid, mime)) = existing {
        if render::asset_path(&st.cfg.data_dir, &aid).exists() {
            return Ok(json!({ "id": aid, "url": format!("/api/assets/{aid}"), "width": w, "height": h, "mime": mime, "reused": true }));
        }
    }
    let aid = new_id();
    let dir = st.cfg.data_dir.join("assets");
    tokio::fs::create_dir_all(&dir).await.map_err(AppError::internal)?;
    tokio::fs::write(render::asset_path(&st.cfg.data_dir, &aid), body).await.map_err(AppError::internal)?;
    let size = body.len() as i64;
    let (aid2, ws, fid, uid, mime2) = (aid.clone(), ws.to_string(), file_id.to_string(), user_id.to_string(), mime.to_string());
    st.db
        .call(move |c| {
            c.execute(
                "INSERT INTO assets (id, workspace_id, file_id, mime, width, height, size, created_by, created_at, sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![aid2, ws, fid, mime2, w, h, size, uid, now(), hash],
            )?;
            Ok(())
        })
        .await?;
    Ok(json!({ "id": aid, "url": format!("/api/assets/{aid}"), "width": w, "height": h, "mime": mime }))
}

/// Hash images uploaded before content hashes existed, a few at a time, so
/// they can be reused too.
pub async fn backfill_image_hashes(st: AppState) {
    loop {
        let batch: Vec<String> = match st
            .db
            .call(|c| {
                let mut q = c.prepare("SELECT id FROM assets WHERE sha256 IS NULL LIMIT 50")?;
                let ids = q.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
                Ok(ids)
            })
            .await
        {
            Ok(b) if !b.is_empty() => b,
            _ => return,
        };
        for aid in batch {
            let hash = match tokio::fs::read(render::asset_path(&st.cfg.data_dir, &aid)).await {
                Ok(bytes) => {
                    use sha2::{Digest, Sha256};
                    hex::encode(Sha256::digest(&bytes))
                }
                // Missing file: mark it so it isn't retried (and never matches).
                Err(_) => "missing".to_string(),
            };
            let _ = st.db.call(move |c| Ok(c.execute("UPDATE assets SET sha256 = ?1 WHERE id = ?2", params![hash, aid])?)).await;
        }
    }
}

/// `POST /api/image-uploads/{token}`: one image per request, no sign-in (the
/// link from MCP `upload_image` is the credential).
pub async fn upload_image_with_link(State(st): State<AppState>, Path(token): Path<String>, body: Bytes) -> AppResult<Json<Value>> {
    // A broken file doesn't use up one of the link's uploads.
    let mime = check_image(&st, &body)?;
    let grant = crate::uploads::take(&token, |k| matches!(k, crate::uploads::Kind::Image { .. }), "upload_image")?;
    let crate::uploads::Kind::Image { file_id } = &grant.kind else {
        return Err(AppError::internal("wrong upload link kind"));
    };
    // The person may have lost access since the link was made.
    let (uid, fid) = (grant.user_id.clone(), file_id.clone());
    st.db.call(move |c| file_access(c, &uid, &fid, Role::Editor).map(|_| ())).await?;
    let mut asset = store_upload(&st, &grant.workspace_id, file_id, &grant.user_id, mime, &body).await?;
    asset["remainingUploads"] = json!(grant.remaining);
    asset["node"] = json!({ "type": "image", "asset": asset["id"], "width": asset["width"], "height": asset["height"] });
    Ok(Json(asset))
}

/// `GET /api/png-exports/{token}`: a PNG export, no sign-in (the link from
/// MCP `export_png` is the credential).
pub async fn export_png_with_link(State(st): State<AppState>, Path(token): Path<String>) -> AppResult<Response> {
    let grant = crate::uploads::take(&token, |k| matches!(k, crate::uploads::Kind::Png { .. }), "export_png")?;
    let crate::uploads::Kind::Png { file_id, node, page, scale, background } = grant.kind else {
        return Err(AppError::internal("wrong link kind"));
    };
    // The person may have lost access since the link was made.
    let (uid, fid) = (grant.user_id.clone(), file_id.clone());
    st.db.call(move |c| file_access(c, &uid, &fid, Role::Viewer).map(|_| ())).await?;
    let q = ExportQuery { node, page, scale: Some(scale), background: Some(background) };
    let mut res = export_png_response(&st, &file_id, q, st.cfg.max_export_px).await?;
    res.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    Ok(res)
}

pub async fn get_asset(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Response> {
    let uid = user.id.clone();
    serve_asset(&st, &id, move |c, ws| require_workspace(c, &uid, ws, Role::Viewer).map(|_| ())).await
}

/// Serve an uploaded image after `allow(conn, workspace_id)` approves access.
pub async fn serve_asset<F>(st: &AppState, id: &str, allow: F) -> AppResult<Response>
where
    F: FnOnce(&rusqlite::Connection, &str) -> AppResult<()> + Send + 'static,
{
    if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(AppError::not_found("asset"));
    }
    let aid = id.to_string();
    let mime: String = st
        .db
        .call(move |c| {
            let row: Option<(String, String)> = c
                .query_row("SELECT workspace_id, mime FROM assets WHERE id = ?1", params![aid], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let (ws, mime) = row.ok_or_else(|| AppError::not_found("asset"))?;
            allow(c, &ws).map_err(|_| AppError::not_found("asset"))?;
            Ok(mime)
        })
        .await?;
    let bytes = tokio::fs::read(render::asset_path(&st.cfg.data_dir, id))
        .await
        .map_err(|_| AppError::not_found("asset"))?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "private, max-age=31536000, immutable".to_string()),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flexma_doc::NodeType;

    #[test]
    fn painting_files_start_with_a_canvas_and_a_layer() {
        let doc = painting_document();
        let canvas = doc.get("1:1").expect("canvas frame");
        assert_eq!((canvas.kind, canvas.parent.as_deref()), (NodeType::Frame, Some("0:1")));
        let layer = doc.get("1:2").expect("paint layer");
        assert_eq!((layer.kind, layer.parent.as_deref()), (NodeType::Paint, Some("1:1")));
        assert_eq!((layer.width, layer.height), (canvas.width, canvas.height));
        assert_eq!(FileKind::parse("painting").unwrap(), FileKind::Painting);
        assert!(FileKind::parse("sculpture").is_err());
    }
}
