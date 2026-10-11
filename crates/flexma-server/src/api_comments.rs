//! Comments: threads pinned to a point on a page (optionally attached to a
//! node), with replies and resolve. Shared by the REST API and MCP tools.
//! Changes are pushed to open editors as `{t:"comments"}` so they refetch.

use crate::auth::{file_access, AuthUser, Role};
use crate::db::{new_id, now, ts};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

/// Who is writing a comment (a person, or an agent acting for one).
pub struct Author {
    pub user_id: String,
    pub name: String,
    pub color: String,
    pub is_agent: bool,
}

impl Author {
    pub fn person(u: &AuthUser) -> Author {
        Author { user_id: u.id.clone(), name: u.name.clone(), color: u.color.clone(), is_agent: false }
    }
}

const COMMENT_COLS: &str =
    "id, parent_id, page_id, node_id, x, y, author_id, author_name, author_color, is_agent, body, resolved, created_at, updated_at";

fn row_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, String>(0)?,
        "parentId": r.get::<_, Option<String>>(1)?,
        "pageId": r.get::<_, Option<String>>(2)?,
        "nodeId": r.get::<_, Option<String>>(3)?,
        "x": r.get::<_, f64>(4)?,
        "y": r.get::<_, f64>(5)?,
        "author": {
            "id": r.get::<_, Option<String>>(6)?,
            "name": r.get::<_, String>(7)?,
            "color": r.get::<_, String>(8)?,
            "isAgent": r.get::<_, i64>(9)? != 0,
        },
        "body": r.get::<_, String>(10)?,
        "resolved": r.get::<_, i64>(11)? != 0,
        "createdAt": ts(r.get(12)?),
        "updatedAt": ts(r.get(13)?),
    }))
}

/// All threads of a file (newest last), each with its replies.
pub fn load_threads(c: &rusqlite::Connection, file_id: &str, include_resolved: bool) -> AppResult<Vec<Value>> {
    let mut stmt = c.prepare(&format!("SELECT {COMMENT_COLS} FROM comments WHERE file_id = ?1 ORDER BY created_at, id"))?;
    let all = stmt.query_map(params![file_id], row_json)?.collect::<Result<Vec<_>, _>>()?;
    let mut threads: Vec<Value> = Vec::new();
    for c in &all {
        if c["parentId"].is_null() && (include_resolved || !c["resolved"].as_bool().unwrap_or(false)) {
            let mut t = c.clone();
            let replies: Vec<Value> = all.iter().filter(|r| r["parentId"] == c["id"]).cloned().collect();
            t["replies"] = Value::Array(replies);
            threads.push(t);
        }
    }
    Ok(threads)
}

pub struct NewComment {
    pub body: String,
    pub parent_id: Option<String>,
    pub page_id: Option<String>,
    pub node_id: Option<String>,
    pub x: f64,
    pub y: f64,
}

/// Insert a comment or reply; returns its id.
pub fn insert_comment(c: &rusqlite::Connection, file_id: &str, author: &Author, nc: NewComment) -> AppResult<String> {
    let body = nc.body.trim().to_string();
    if body.is_empty() || body.len() > 10_000 {
        return Err(AppError::bad_request("comment text must be 1–10000 characters"));
    }
    let (page, node, x, y) = match &nc.parent_id {
        Some(p) => {
            // Replies inherit the thread's position.
            let row: Option<(Option<String>, Option<String>, f64, f64)> = c
                .query_row(
                    "SELECT page_id, node_id, x, y FROM comments WHERE id = ?1 AND file_id = ?2 AND parent_id IS NULL",
                    params![p, file_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            row.ok_or_else(|| AppError::not_found("comment thread"))?
        }
        None => (nc.page_id.clone(), nc.node_id.clone(), nc.x, nc.y),
    };
    let id = new_id();
    let t = now();
    c.execute(
        &format!("INSERT INTO comments (file_id, {COMMENT_COLS}) VALUES (?13, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12, ?12)"),
        params![id, nc.parent_id, page, node, x, y, author.user_id, author.name, author.color, author.is_agent as i64, body, t, file_id],
    )?;
    if let Some(p) = &nc.parent_id {
        // Replying reopens a resolved thread.
        c.execute("UPDATE comments SET resolved = 0, updated_at = ?1 WHERE id = ?2", params![t, p])?;
    }
    Ok(id)
}

#[derive(Deserialize)]
pub struct ListQuery {
    resolved: Option<bool>,
}

pub async fn list(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    let threads = st
        .db
        .call(move |c| {
            file_access(c, &user.id, &id, Role::Viewer)?;
            load_threads(c, &id, q.resolved.unwrap_or(true))
        })
        .await?;
    Ok(Json(Value::Array(threads)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReq {
    body: String,
    parent_id: Option<String>,
    page_id: Option<String>,
    node_id: Option<String>,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
}

pub async fn create(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<CreateReq>,
) -> AppResult<Json<Value>> {
    let fid = id.clone();
    // Viewers may comment (like Figma), editors and owners too.
    let cid = st
        .db
        .call(move |c| {
            file_access(c, &user.id, &fid, Role::Viewer)?;
            let author = Author::person(&user);
            insert_comment(
                c,
                &fid,
                &author,
                NewComment { body: req.body, parent_id: req.parent_id, page_id: req.page_id, node_id: req.node_id, x: req.x, y: req.y },
            )
        })
        .await?;
    st.rooms.notify_if_open(&id, json!({ "t": "comments" })).await;
    Ok(Json(json!({ "id": cid })))
}

#[derive(Deserialize)]
pub struct UpdateReq {
    body: Option<String>,
    resolved: Option<bool>,
}

pub async fn update(State(st): State<AppState>, user: AuthUser, Path(cid): Path<String>, Json(req): Json<UpdateReq>) -> AppResult<Json<Value>> {
    let file_id = st
        .db
        .call(move |c| {
            let row: Option<(String, Option<String>)> = c
                .query_row("SELECT file_id, author_id FROM comments WHERE id = ?1", params![cid], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let (fid, author) = row.ok_or_else(|| AppError::not_found("comment"))?;
            let role = file_access(c, &user.id, &fid, Role::Viewer)?.2;
            if let Some(b) = req.body {
                if author.as_deref() != Some(user.id.as_str()) {
                    return Err(AppError::forbidden("you can only edit your own comments"));
                }
                let b = b.trim().to_string();
                if b.is_empty() {
                    return Err(AppError::bad_request("comment text must not be empty"));
                }
                c.execute("UPDATE comments SET body = ?1, updated_at = ?2 WHERE id = ?3", params![b, now(), cid])?;
            }
            if let Some(r) = req.resolved {
                if !role.can_edit() && author.as_deref() != Some(user.id.as_str()) {
                    return Err(AppError::forbidden("only editors can resolve other people's comments"));
                }
                c.execute("UPDATE comments SET resolved = ?1, updated_at = ?2 WHERE id = ?3", params![r as i64, now(), cid])?;
            }
            Ok(fid)
        })
        .await?;
    st.rooms.notify_if_open(&file_id, json!({ "t": "comments" })).await;
    Ok(Json(json!({})))
}

pub async fn delete(State(st): State<AppState>, user: AuthUser, Path(cid): Path<String>) -> AppResult<Json<Value>> {
    let file_id = st
        .db
        .call(move |c| {
            let row: Option<(String, Option<String>)> = c
                .query_row("SELECT file_id, author_id FROM comments WHERE id = ?1", params![cid], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let (fid, author) = row.ok_or_else(|| AppError::not_found("comment"))?;
            let role = file_access(c, &user.id, &fid, Role::Viewer)?.2;
            if author.as_deref() != Some(user.id.as_str()) && role != Role::Owner {
                return Err(AppError::forbidden("you can only delete your own comments"));
            }
            c.execute("DELETE FROM comments WHERE id = ?1", params![cid])?;
            Ok(fid)
        })
        .await?;
    st.rooms.notify_if_open(&file_id, json!({ "t": "comments" })).await;
    Ok(Json(json!({})))
}
