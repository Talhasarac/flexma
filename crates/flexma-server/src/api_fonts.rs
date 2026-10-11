//! Workspace fonts over REST (see `fonts.rs`), plus the Google Fonts catalogue
//! and picker previews. Editors manage fonts; every member and share-link
//! guest can load them.

use crate::api_share::resolve;
use crate::auth::{require_workspace, AuthUser, Role};
use crate::error::{AppError, AppResult};
use crate::fonts::{self, font_path};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

pub async fn list(State(st): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    let rows = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &ws, Role::Viewer)?;
            fonts::list_json(c, &ws)
        })
        .await?;
    Ok(Json(json!(rows)))
}

/// Upload one .ttf/.otf file (raw body). A face with the same family, weight
/// and style replaces the old one.
pub async fn upload(State(st): State<AppState>, user: AuthUser, Path(ws): Path<String>, body: Bytes) -> AppResult<Json<Value>> {
    let (uid, w) = (user.id.clone(), ws.clone());
    st.db.call(move |c| require_workspace(c, &uid, &w, Role::Editor).map(|_| ())).await?;
    let info = fonts::inspect(&body).map_err(AppError::bad_request)?;
    let face = store_face(&st, &ws, &user.id, info, "upload", body.to_vec()).await?;
    fonts::fonts_changed(&st, &ws).await;
    Ok(Json(face))
}

async fn store_face(st: &AppState, ws: &str, user_id: &str, info: fonts::FaceInfo, source: &'static str, bytes: Vec<u8>) -> AppResult<Value> {
    tokio::fs::create_dir_all(fonts::fonts_dir(&st.cfg.data_dir)).await.map_err(AppError::internal)?;
    let data_dir = st.cfg.data_dir.clone();
    let (w, uid, i2, size) = (ws.to_string(), user_id.to_string(), info.clone(), bytes.len());
    let (id, replaced) = st
        .db
        .call(move |c| fonts::insert_face(c, &data_dir, &w, &uid, &i2, source, size))
        .await?;
    let path = font_path(&st.cfg.data_dir, &id, info.format);
    if let Err(e) = tokio::fs::write(&path, &bytes).await {
        let gone = id.clone();
        let _ = st.db.call(move |c| Ok(c.execute("DELETE FROM fonts WHERE id = ?1", params![gone])?)).await;
        return Err(AppError::internal(e));
    }
    if let Some(old) = replaced {
        let _ = tokio::fs::remove_file(old).await;
    }
    Ok(json!({
        "id": id,
        "family": info.family,
        "weight": info.weight,
        "weightMax": info.weight_max,
        "italic": info.italic,
        "source": source,
    }))
}

#[derive(Deserialize)]
pub struct GoogleReq {
    family: String,
}

/// Add every style of a Google Fonts family to the workspace. The server
/// downloads the files once and serves them itself.
pub async fn add_google(
    State(st): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(req): Json<GoogleReq>,
) -> AppResult<Json<Value>> {
    let (uid, w) = (user.id.clone(), ws.clone());
    st.db.call(move |c| require_workspace(c, &uid, &w, Role::Editor).map(|_| ())).await?;
    let family = add_google_family(&st, &ws, &user.id, &req.family).await?;
    Ok(Json(family))
}

/// Shared with the MCP `add_font` tool.
pub async fn add_google_family(st: &AppState, ws: &str, user_id: &str, family: &str) -> AppResult<Value> {
    let cat = fonts::google_catalogue(st).await?;
    let f = cat
        .find(family)
        .ok_or_else(|| AppError::not_found(&format!("Google font \"{}\"", fonts::clean_family(family))))?
        .clone();
    let name = f.family.clone();
    let faces = tokio::task::spawn_blocking(move || fonts::download_family(&f))
        .await
        .map_err(AppError::internal)?
        .map_err(|e| AppError::new(axum::http::StatusCode::BAD_GATEWAY, format!("couldn't download {name} from Google Fonts: {e}")))?;
    let mut styles = Vec::new();
    let family = faces.first().map(|(i, _)| i.family.clone()).unwrap_or_default();
    for (info, bytes) in faces {
        styles.push(format!("{}{}", info.weight, if info.italic { "i" } else { "" }));
        store_face(st, ws, user_id, info, "google", bytes).await?;
    }
    fonts::fonts_changed(st, ws).await;
    Ok(json!({ "family": family, "styles": styles }))
}

#[derive(Deserialize)]
pub struct FamilyQuery {
    family: String,
}

/// Remove a family (all of its styles) from the workspace.
pub async fn delete_family(
    State(st): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<FamilyQuery>,
) -> AppResult<Json<Value>> {
    let data_dir = st.cfg.data_dir.clone();
    let w = ws.clone();
    let files = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &w, Role::Editor)?;
            fonts::delete_family(c, &data_dir, &w, &q.family)
        })
        .await?;
    for f in files {
        let _ = tokio::fs::remove_file(f).await;
    }
    fonts::fonts_changed(&st, &ws).await;
    Ok(Json(json!({})))
}

/// A font file, for members of its workspace.
pub async fn file(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Response> {
    serve(&st, id, move |c, ws| require_workspace(c, &user.id, ws, Role::Viewer).map(|_| ())).await
}

/// The fonts of a shared file's workspace, for share-link guests.
pub async fn share_list(State(st): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let rows = st
        .db
        .call(move |c| {
            let (_, ws) = resolve(c, &token)?;
            fonts::list_json(c, &ws)
        })
        .await?;
    Ok(Json(json!(rows)))
}

pub async fn share_file(State(st): State<AppState>, Path((token, id)): Path<(String, String)>) -> AppResult<Response> {
    serve(&st, id, move |c, ws| {
        let (_, share_ws) = resolve(c, &token)?;
        if share_ws == ws {
            Ok(())
        } else {
            Err(AppError::not_found("font"))
        }
    })
    .await
}

async fn serve<F>(st: &AppState, id: String, allow: F) -> AppResult<Response>
where
    F: FnOnce(&rusqlite::Connection, &str) -> AppResult<()> + Send + 'static,
{
    if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(AppError::not_found("font"));
    }
    let fid = id.clone();
    let format: String = st
        .db
        .call(move |c| {
            let (ws, format): (String, String) = c
                .query_row("SELECT workspace_id, format FROM fonts WHERE id = ?1", params![fid], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?
                .ok_or_else(|| AppError::not_found("font"))?;
            allow(c, &ws).map_err(|_| AppError::not_found("font"))?;
            Ok(format)
        })
        .await?;
    let bytes = tokio::fs::read(font_path(&st.cfg.data_dir, &id, &format))
        .await
        .map_err(|_| AppError::not_found("font"))?;
    let mime = if format == "otf" { "font/otf" } else { "font/ttf" };
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            // A font id never changes content (replacing a face makes a new id).
            (header::CACHE_CONTROL, "private, max-age=31536000, immutable"),
        ],
        bytes,
    )
        .into_response())
}

/// The Google Fonts catalogue: `[{ family, category, styles, popularity }]`,
/// most popular first.
pub async fn google_catalogue(State(st): State<AppState>, _user: AuthUser) -> AppResult<Response> {
    let cat = fonts::google_catalogue(&st).await?;
    Ok((
        [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "private, max-age=3600")],
        cat.json.to_string(),
    )
        .into_response())
}

/// A tiny font with just the family name's glyphs, for picker previews.
pub async fn google_preview(State(st): State<AppState>, _user: AuthUser, Query(q): Query<FamilyQuery>) -> AppResult<Response> {
    let bytes = fonts::google_preview(&st, &q.family).await?;
    Ok((
        [(header::CONTENT_TYPE, "font/ttf"), (header::CACHE_CONTROL, "private, max-age=604800")],
        bytes,
    )
        .into_response())
}
