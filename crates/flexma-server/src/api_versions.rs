//! Version history: automatic snapshots (taken while saving, at most every 15
//! minutes of editing), named versions, previews and restore.

use crate::auth::{file_access, AuthUser, Role};
use crate::db::{new_id, now, ts};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::Response;
use axum::Json;
use flexma_doc::Document;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

pub async fn list(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let rows = st
        .db
        .call(move |c| {
            file_access(c, &user.id, &id, Role::Viewer)?;
            let mut stmt = c.prepare(
                "SELECT v.id, v.label, v.version, v.created_at, u.name FROM file_versions v
                 LEFT JOIN users u ON u.id = v.created_by
                 WHERE v.file_id = ?1 ORDER BY v.created_at DESC LIMIT 300",
            )?;
            let rows = stmt
                .query_map(params![id], |r| {
                    Ok(json!({
                        "id": r.get::<_, String>(0)?,
                        "label": r.get::<_, Option<String>>(1)?,
                        "version": r.get::<_, i64>(2)?,
                        "createdAt": ts(r.get(3)?),
                        "createdBy": r.get::<_, Option<String>>(4)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize)]
pub struct CreateReq {
    label: Option<String>,
}

/// Save a named version of the current state.
pub async fn create(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<CreateReq>,
) -> AppResult<Json<Value>> {
    let label = req.label.map(|s| s.trim().chars().take(120).collect::<String>()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Saved version".into());
    let uid = user.id.clone();
    let fid = id.clone();
    st.db.call(move |c| file_access(c, &uid, &fid, Role::Editor)).await?;
    let room = st.rooms.get(&id).await?;
    room.flush().await?;
    let vid = new_id();
    let vid2 = vid.clone();
    st.db
        .call(move |c| {
            c.execute(
                "INSERT INTO file_versions (id, file_id, label, document, version, created_by, created_at)
                 SELECT ?1, id, ?2, document, version, ?3, ?4 FROM files WHERE id = ?5",
                params![vid2, label, user.id, now(), id],
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "id": vid })))
}

pub async fn preview(State(st): State<AppState>, user: AuthUser, Path((id, vid)): Path<(String, String)>) -> AppResult<Response> {
    let file_id = id.clone();
    let (doc, ws): (String, String) = st
        .db
        .call(move |c| {
            let (ws, _, _) = file_access(c, &user.id, &id, Role::Viewer)?;
            let doc = c
                .query_row("SELECT document FROM file_versions WHERE id = ?1 AND file_id = ?2", params![vid, id], |r| r.get(0))
                .optional()?
                .ok_or_else(|| AppError::not_found("version"))?;
            Ok((doc, ws))
        })
        .await?;
    let png = crate::api_files::render_thumbnail(&st, &file_id, &ws, doc).await?;
    let mut res = crate::api_files::thumbnail_image(png);
    res.headers_mut().insert(header::CACHE_CONTROL, "private, max-age=3600".parse().unwrap());
    Ok(res)
}

/// Restore a version. The current state is saved as a version first, so a
/// restore can itself be undone from the history.
pub async fn restore(State(st): State<AppState>, user: AuthUser, Path((id, vid)): Path<(String, String)>) -> AppResult<Json<Value>> {
    let uid = user.id.clone();
    let fid = id.clone();
    st.db.call(move |c| file_access(c, &uid, &fid, Role::Editor)).await?;
    // 1. Persist whatever is live right now, then take the file offline so
    //    nobody can reopen (and re-save) the old state mid-restore.
    if let Ok(room) = st.rooms.get(&id).await {
        let _ = room.flush().await;
    }
    st.rooms.block(&id);
    st.rooms.discard_and_wait(&id, Some("reload".into())).await;
    // 2. Snapshot current state, then swap in the chosen version.
    let (fid, uid) = (id.clone(), user.id.clone());
    let result = st
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            let (doc, label, created): (String, Option<String>, i64) = tx
                .query_row(
                    "SELECT document, label, created_at FROM file_versions WHERE id = ?1 AND file_id = ?2",
                    params![vid, fid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or_else(|| AppError::not_found("version"))?;
            Document::from_json(&doc).map_err(|e| AppError::internal(format!("corrupt version: {e}")))?;
            tx.execute(
                "INSERT INTO file_versions (id, file_id, label, document, version, created_by, created_at)
                 SELECT ?1, id, 'Before restore', document, version, ?2, ?3 FROM files WHERE id = ?4",
                params![new_id(), uid, now(), fid],
            )?;
            tx.execute(
                "UPDATE files SET document = ?1, version = version + 1, thumbnail = NULL, updated_at = ?2 WHERE id = ?3",
                params![doc, now(), fid],
            )?;
            tx.commit()?;
            Ok(label.unwrap_or_else(|| ts(created)))
        })
        .await;
    // 3. Back online: clients reconnect and load the restored document.
    st.rooms.unblock(&id);
    let restored_label = result?;
    Ok(Json(json!({ "restored": restored_label })))
}
