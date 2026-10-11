//! Public view-only share links. One link per file; anyone with the link can
//! view the file live (as an anonymous "Guest" peer), inspect layers and export,
//! but not edit or see comments. Turning a link off or regenerating it kicks
//! everyone who joined through the old one.

use crate::api_files::{export_png_response, export_svg_response, serve_asset, ExportQuery};
use crate::auth::{file_access, AuthUser, Role};
use crate::db::{now, random_string, ts};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap};
use axum::response::Response;
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

const TOKEN_LEN: usize = 32;

/// (file id, workspace id) for a live share token.
pub fn resolve(c: &rusqlite::Connection, token: &str) -> AppResult<(String, String)> {
    if token.len() != TOKEN_LEN || !token.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Err(AppError::not_found("share link"));
    }
    c.query_row(
        "SELECT s.file_id, p.workspace_id FROM file_shares s
         JOIN files f ON f.id = s.file_id JOIN projects p ON p.id = f.project_id
         WHERE s.token = ?1 AND (s.expires_at IS NULL OR s.expires_at > ?2)",
        params![token, now()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?
    .ok_or_else(|| AppError::not_found("share link (it may have been turned off or expired)"))
}

fn share_json(base: &str, row: Option<(String, Option<String>, i64, Option<i64>)>) -> Value {
    match row {
        Some((token, by, created, expires)) => json!({
            "enabled": true,
            "token": token,
            "url": format!("{base}/s/{token}"),
            "createdBy": by,
            "createdAt": ts(created),
            "expiresAt": expires.map(ts),
        }),
        None => json!({ "enabled": false }),
    }
}

fn load_share(c: &rusqlite::Connection, file_id: &str) -> AppResult<Option<(String, Option<String>, i64, Option<i64>)>> {
    Ok(c.query_row(
        "SELECT s.token, u.name, s.created_at, s.expires_at FROM file_shares s
         LEFT JOIN users u ON u.id = s.created_by WHERE s.file_id = ?1 AND (s.expires_at IS NULL OR s.expires_at > ?2)",
        params![file_id, now()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()?)
}

pub async fn get(State(st): State<AppState>, headers: HeaderMap, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let base = st.base_url(&headers);
    let row = st
        .db
        .call(move |c| {
            file_access(c, &user.id, &id, Role::Viewer)?;
            load_share(c, &id)
        })
        .await?;
    Ok(Json(share_json(&base, row)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateShare {
    enabled: bool,
    /// Days until the link expires; `null`/absent keeps the current expiry,
    /// `0` means never.
    expires_in_days: Option<u32>,
    #[serde(default)]
    regenerate: bool,
}

/// Turn a file's public link on/off, change its expiry, or regenerate it.
pub async fn update(
    State(st): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateShare>,
) -> AppResult<Json<Value>> {
    let base = st.base_url(&headers);
    let fid = id.clone();
    let (old_token, row) = st
        .db
        .call(move |c| set_share(c, &user.id, &fid, req.enabled, req.expires_in_days, req.regenerate))
        .await?;
    if let Some(t) = old_token {
        st.rooms.kick_share_if_open(&id, &t).await;
    }
    Ok(Json(share_json(&base, row)))
}

/// Shared by the REST handler and the MCP tool. Returns (token that stopped
/// working, current link row).
#[allow(clippy::type_complexity)]
pub fn set_share(
    c: &rusqlite::Connection,
    user_id: &str,
    fid: &str,
    enabled: bool,
    expires_in_days: Option<u32>,
    regenerate: bool,
) -> AppResult<(Option<String>, Option<(String, Option<String>, i64, Option<i64>)>)> {
    file_access(c, user_id, fid, Role::Editor)?;
    let old: Option<String> = c.query_row("SELECT token FROM file_shares WHERE file_id = ?1", params![fid], |r| r.get(0)).optional()?;
    if !enabled {
        c.execute("DELETE FROM file_shares WHERE file_id = ?1", params![fid])?;
        return Ok((old, None));
    }
    let expires = expires_in_days.map(|d| if d == 0 { None } else { Some(now() + d as i64 * 86400) });
    let live = load_share(c, fid)?;
    match (&live, regenerate) {
        (Some(_), false) => {
            if let Some(e) = expires {
                c.execute("UPDATE file_shares SET expires_at = ?1 WHERE file_id = ?2", params![e, fid])?;
            }
        }
        _ => {
            // Regenerating keeps the current expiry unless a new one was given.
            let expires_at = match expires {
                Some(e) => e,
                None => live.as_ref().and_then(|l| l.3),
            };
            let token = random_string(TOKEN_LEN);
            c.execute(
                "INSERT INTO file_shares (file_id, token, created_by, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(file_id) DO UPDATE SET token = excluded.token, created_by = excluded.created_by,
                   created_at = excluded.created_at, expires_at = excluded.expires_at",
                params![fid, token, user_id, now(), expires_at],
            )?;
        }
    }
    let row = load_share(c, fid)?;
    let replaced = match (&old, &row) {
        (Some(o), Some(n)) if *o != n.0 => Some(o.clone()),
        _ => None,
    };
    Ok((replaced, row))
}

pub fn share_url(base: &str, token: &str) -> String {
    format!("{base}/s/{token}")
}

/// Public: what's behind a share link.
pub async fn info(State(st): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let v = st
        .db
        .call(move |c| {
            let (file_id, _) = resolve(c, &token)?;
            Ok(c.query_row(
                "SELECT f.id, f.name, w.name, u.name, f.updated_at, s.expires_at FROM files f
                 JOIN projects p ON p.id = f.project_id JOIN workspaces w ON w.id = p.workspace_id
                 JOIN file_shares s ON s.file_id = f.id LEFT JOIN users u ON u.id = s.created_by
                 WHERE f.id = ?1",
                params![file_id],
                |r| {
                    Ok(json!({
                        "fileId": r.get::<_, String>(0)?,
                        "name": r.get::<_, String>(1)?,
                        "workspaceName": r.get::<_, String>(2)?,
                        "sharedBy": r.get::<_, Option<String>>(3)?,
                        "updatedAt": ts(r.get(4)?),
                        "expiresAt": r.get::<_, Option<i64>>(5)?.map(ts),
                    }))
                },
            )?)
        })
        .await?;
    Ok(Json(v))
}

pub async fn asset(State(st): State<AppState>, Path((token, aid)): Path<(String, String)>) -> AppResult<Response> {
    let t = token.clone();
    let (_, ws) = st.db.call(move |c| resolve(c, &t)).await?;
    serve_asset(&st, &aid, move |_, asset_ws| if asset_ws == ws { Ok(()) } else { Err(AppError::not_found("asset")) }).await
}

pub async fn export_svg(State(st): State<AppState>, Path(token): Path<String>, Query(q): Query<ExportQuery>) -> AppResult<Response> {
    let t = token.clone();
    let (file_id, _) = st.db.call(move |c| resolve(c, &t)).await?;
    export_svg_response(&st, &file_id, q, Some(token)).await
}

pub async fn export_png(State(st): State<AppState>, Path(token): Path<String>, Query(q): Query<ExportQuery>) -> AppResult<Response> {
    let t = token.clone();
    let (file_id, _) = st.db.call(move |c| resolve(c, &t)).await?;
    // Public links get a smaller size cap than members.
    export_png_response(&st, &file_id, q, st.cfg.max_export_px.min(4096)).await
}

/// Public preview image (e.g. for link unfurls).
pub async fn thumbnail(State(st): State<AppState>, Path(token): Path<String>) -> AppResult<Response> {
    let t = token.clone();
    let (file_id, _) = st.db.call(move |c| resolve(c, &t)).await?;
    let mut res = crate::api_files::thumbnail_response(&st, &file_id).await?;
    res.headers_mut().insert(header::CACHE_CONTROL, "public, max-age=60".parse().unwrap());
    Ok(res)
}
