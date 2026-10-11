//! Workspaces (shared team spaces), members, invite links and projects.

use crate::auth::{project_access, require_workspace, AuthUser, Role};
use crate::db::{new_id, now, random_string, ts};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

const INVITE_DAYS: i64 = 14;

fn clean_name(s: &str, what: &str) -> AppResult<String> {
    let s = s.trim();
    if s.is_empty() || s.len() > 120 {
        return Err(AppError::bad_request(format!("{what} name must be 1–120 characters")));
    }
    Ok(s.to_string())
}

fn workspace_json(c: &rusqlite::Connection, user_id: &str, ws: &str) -> AppResult<Value> {
    let row = c.query_row(
        "SELECT w.id, w.name, w.created_at, m.role, (SELECT COUNT(*) FROM workspace_members WHERE workspace_id = w.id)
         FROM workspaces w JOIN workspace_members m ON m.workspace_id = w.id AND m.user_id = ?1 WHERE w.id = ?2",
        params![user_id, ws],
        |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "createdAt": ts(r.get(2)?),
                "role": r.get::<_, String>(3)?,
                "memberCount": r.get::<_, i64>(4)?,
            }))
        },
    )
    .optional()?;
    row.ok_or_else(|| AppError::not_found("workspace"))
}

pub async fn list(State(st): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let uid = user.id.clone();
    let rows = st
        .db
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT w.id, w.name, w.created_at, m.role, (SELECT COUNT(*) FROM workspace_members WHERE workspace_id = w.id)
                 FROM workspaces w JOIN workspace_members m ON m.workspace_id = w.id
                 WHERE m.user_id = ?1 ORDER BY w.created_at",
            )?;
            let rows = stmt
                .query_map(params![uid], |r| {
                    Ok(json!({
                        "id": r.get::<_, String>(0)?,
                        "name": r.get::<_, String>(1)?,
                        "createdAt": ts(r.get(2)?),
                        "role": r.get::<_, String>(3)?,
                        "memberCount": r.get::<_, i64>(4)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize)]
pub struct NameReq {
    name: String,
}

pub async fn create(State(st): State<AppState>, user: AuthUser, Json(req): Json<NameReq>) -> AppResult<Json<Value>> {
    let name = clean_name(&req.name, "workspace")?;
    let uid = user.id.clone();
    let v = st
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            let id = new_id();
            tx.execute(
                "INSERT INTO workspaces (id, name, created_by, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, name, uid, now()],
            )?;
            tx.execute(
                "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) VALUES (?1, ?2, 'owner', ?3)",
                params![id, uid, now()],
            )?;
            tx.execute(
                "INSERT INTO projects (id, workspace_id, name, created_at) VALUES (?1, ?2, 'Drafts', ?3)",
                params![new_id(), id, now()],
            )?;
            tx.commit()?;
            workspace_json(c, &uid, &id)
        })
        .await?;
    Ok(Json(v))
}

pub async fn get(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let v = st.db.call(move |c| workspace_json(c, &user.id, &id)).await?;
    Ok(Json(v))
}

pub async fn update(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<NameReq>,
) -> AppResult<Json<Value>> {
    let name = clean_name(&req.name, "workspace")?;
    let v = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &id, Role::Owner)?;
            c.execute("UPDATE workspaces SET name = ?1 WHERE id = ?2", params![name, id])?;
            workspace_json(c, &user.id, &id)
        })
        .await?;
    Ok(Json(v))
}

pub async fn delete(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let rooms = st.rooms.clone();
    let files: Vec<String> = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &id, Role::Owner)?;
            let mut stmt = c.prepare(
                "SELECT f.id FROM files f JOIN projects p ON p.id = f.project_id WHERE p.workspace_id = ?1",
            )?;
            let files = stmt.query_map(params![id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
            c.execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
            Ok(files)
        })
        .await?;
    for f in files {
        rooms.close(&f, Some("This file was deleted.".into()));
    }
    Ok(Json(json!({})))
}

// ---------- members ----------

pub async fn members(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let rows = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &id, Role::Viewer)?;
            let mut stmt = c.prepare(
                "SELECT u.id, u.email, u.name, u.color, m.role FROM workspace_members m JOIN users u ON u.id = m.user_id
                 WHERE m.workspace_id = ?1 ORDER BY m.created_at",
            )?;
            let rows = stmt
                .query_map(params![id], |r| {
                    Ok(json!({
                        "userId": r.get::<_, String>(0)?,
                        "email": r.get::<_, String>(1)?,
                        "name": r.get::<_, String>(2)?,
                        "color": r.get::<_, String>(3)?,
                        "role": r.get::<_, String>(4)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize)]
pub struct RoleReq {
    role: String,
}

fn owner_count(c: &rusqlite::Connection, ws: &str) -> AppResult<i64> {
    Ok(c.query_row(
        "SELECT COUNT(*) FROM workspace_members WHERE workspace_id = ?1 AND role = 'owner'",
        params![ws],
        |r| r.get(0),
    )?)
}

pub async fn update_member(
    State(st): State<AppState>,
    user: AuthUser,
    Path((id, member)): Path<(String, String)>,
    Json(req): Json<RoleReq>,
) -> AppResult<Json<Value>> {
    let role = Role::parse(&req.role).ok_or_else(|| AppError::bad_request("role must be owner, editor or viewer"))?;
    st.db
        .call(move |c| {
            require_workspace(c, &user.id, &id, Role::Owner)?;
            let current = crate::auth::workspace_role(c, &member, &id)?.ok_or_else(|| AppError::not_found("member"))?;
            if current == Role::Owner && role != Role::Owner && owner_count(c, &id)? <= 1 {
                return Err(AppError::bad_request("a workspace needs at least one owner"));
            }
            c.execute(
                "UPDATE workspace_members SET role = ?1 WHERE workspace_id = ?2 AND user_id = ?3",
                params![role.as_str(), id, member],
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({})))
}

pub async fn remove_member(
    State(st): State<AppState>,
    user: AuthUser,
    Path((id, member)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    st.db
        .call(move |c| {
            if member != user.id {
                require_workspace(c, &user.id, &id, Role::Owner)?;
            }
            let current = crate::auth::workspace_role(c, &member, &id)?.ok_or_else(|| AppError::not_found("member"))?;
            if current == Role::Owner && owner_count(c, &id)? <= 1 {
                return Err(AppError::bad_request("the last owner cannot leave; transfer ownership or delete the workspace"));
            }
            c.execute(
                "DELETE FROM workspace_members WHERE workspace_id = ?1 AND user_id = ?2",
                params![id, member],
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({})))
}

// ---------- invites ----------

fn invite_json(base: &str, r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let token: String = r.get(3)?;
    Ok(json!({
        "id": r.get::<_, String>(0)?,
        "email": r.get::<_, Option<String>>(1)?,
        "role": r.get::<_, String>(2)?,
        "token": token,
        "url": format!("{base}/invite/{token}"),
        "createdAt": ts(r.get(4)?),
        "expiresAt": ts(r.get(5)?),
    }))
}

pub async fn list_invites(
    State(st): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let base = st.base_url(&headers);
    let rows = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &id, Role::Owner)?;
            let mut stmt = c.prepare(
                "SELECT id, email, role, token, created_at, expires_at FROM invites
                 WHERE workspace_id = ?1 AND expires_at > ?2 ORDER BY created_at DESC",
            )?;
            let rows = stmt
                .query_map(params![id, now()], |r| invite_json(&base, r))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize)]
pub struct InviteReq {
    email: Option<String>,
    role: String,
}

pub async fn create_invite(
    State(st): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<InviteReq>,
) -> AppResult<Json<Value>> {
    let role = Role::parse(&req.role).ok_or_else(|| AppError::bad_request("role must be owner, editor or viewer"))?;
    let base = st.base_url(&headers);
    let email = req.email.map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty());
    let v = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &id, Role::Owner)?;
            let iid = new_id();
            let token = random_string(24);
            let t = now();
            c.execute(
                "INSERT INTO invites (id, workspace_id, email, role, token, created_by, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![iid, id, email, role.as_str(), token, user.id, t, t + INVITE_DAYS * 86400],
            )?;
            Ok(c.query_row(
                "SELECT id, email, role, token, created_at, expires_at FROM invites WHERE id = ?1",
                params![iid],
                |r| invite_json(&base, r),
            )?)
        })
        .await?;
    Ok(Json(v))
}

pub async fn delete_invite(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    st.db
        .call(move |c| {
            let ws: Option<String> = c
                .query_row("SELECT workspace_id FROM invites WHERE id = ?1", params![id], |r| r.get(0))
                .optional()?;
            let ws = ws.ok_or_else(|| AppError::not_found("invite"))?;
            require_workspace(c, &user.id, &ws, Role::Owner)?;
            c.execute("DELETE FROM invites WHERE id = ?1", params![id])?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({})))
}

pub async fn invite_info(State(st): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let v = st
        .db
        .call(move |c| {
            let row = c
                .query_row(
                    "SELECT w.name, i.role, u.name FROM invites i JOIN workspaces w ON w.id = i.workspace_id
                     LEFT JOIN users u ON u.id = i.created_by WHERE i.token = ?1 AND i.expires_at > ?2",
                    params![token, now()],
                    |r| {
                        Ok(json!({
                            "workspaceName": r.get::<_, String>(0)?,
                            "role": r.get::<_, String>(1)?,
                            "invitedBy": r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        }))
                    },
                )
                .optional()?;
            row.ok_or_else(|| AppError::not_found("invite (it may have expired)"))
        })
        .await?;
    Ok(Json(v))
}

pub async fn accept_invite(State(st): State<AppState>, user: AuthUser, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let v = st
        .db
        .call(move |c| {
            let row: Option<(String, String)> = c
                .query_row(
                    "SELECT workspace_id, role FROM invites WHERE token = ?1 AND expires_at > ?2",
                    params![token, now()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (ws, role) = row.ok_or_else(|| AppError::not_found("invite (it may have expired)"))?;
            // Never downgrade an existing member.
            let existing = crate::auth::workspace_role(c, &user.id, &ws)?;
            let new_role = Role::parse(&role).unwrap_or(Role::Viewer);
            match existing {
                Some(r) if r >= new_role => {}
                Some(_) => {
                    c.execute(
                        "UPDATE workspace_members SET role = ?1 WHERE workspace_id = ?2 AND user_id = ?3",
                        params![role, ws, user.id],
                    )?;
                }
                None => {
                    c.execute(
                        "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) VALUES (?1, ?2, ?3, ?4)",
                        params![ws, user.id, role, now()],
                    )?;
                }
            }
            Ok(json!({ "workspaceId": ws }))
        })
        .await?;
    Ok(Json(v))
}

// ---------- projects ----------

fn project_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, String>(0)?,
        "workspaceId": r.get::<_, String>(1)?,
        "name": r.get::<_, String>(2)?,
        "createdAt": ts(r.get(3)?),
        "fileCount": r.get::<_, i64>(4)?,
    }))
}

const PROJECT_SELECT: &str = "SELECT p.id, p.workspace_id, p.name, p.created_at,
    (SELECT COUNT(*) FROM files f WHERE f.project_id = p.id) FROM projects p";

pub async fn list_projects(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let rows = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &id, Role::Viewer)?;
            let mut stmt = c.prepare(&format!("{PROJECT_SELECT} WHERE p.workspace_id = ?1 ORDER BY p.created_at"))?;
            let rows = stmt.query_map(params![id], project_json)?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(Value::Array(rows)))
}

pub async fn create_project(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<NameReq>,
) -> AppResult<Json<Value>> {
    let name = clean_name(&req.name, "project")?;
    let v = st
        .db
        .call(move |c| {
            require_workspace(c, &user.id, &id, Role::Editor)?;
            let pid = new_id();
            c.execute(
                "INSERT INTO projects (id, workspace_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![pid, id, name, now()],
            )?;
            Ok(c.query_row(&format!("{PROJECT_SELECT} WHERE p.id = ?1"), params![pid], project_json)?)
        })
        .await?;
    Ok(Json(v))
}

pub async fn update_project(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<NameReq>,
) -> AppResult<Json<Value>> {
    let name = clean_name(&req.name, "project")?;
    let v = st
        .db
        .call(move |c| {
            project_access(c, &user.id, &id, Role::Editor)?;
            c.execute("UPDATE projects SET name = ?1 WHERE id = ?2", params![name, id])?;
            Ok(c.query_row(&format!("{PROJECT_SELECT} WHERE p.id = ?1"), params![id], project_json)?)
        })
        .await?;
    Ok(Json(v))
}

pub async fn delete_project(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let rooms = st.rooms.clone();
    let files = st
        .db
        .call(move |c| {
            project_access(c, &user.id, &id, Role::Editor)?;
            let mut stmt = c.prepare("SELECT id FROM files WHERE project_id = ?1")?;
            let files = stmt.query_map(params![id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
            c.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
            Ok(files)
        })
        .await?;
    for f in files {
        rooms.close(&f, Some("This file was deleted.".into()));
    }
    Ok(Json(json!({})))
}
