//! Instance administration (admins only): users, instance settings and backups.

use crate::auth::{hash_password, AuthUser};
use crate::db::{now, random_string, ts};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::Response;
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

fn require_admin(u: &AuthUser) -> AppResult<()> {
    if u.is_admin && !u.via_token {
        Ok(())
    } else {
        Err(AppError::forbidden("administrators only"))
    }
}

/// Instance setting stored in the database (overrides the env default).
pub fn setting(c: &rusqlite::Connection, key: &str) -> AppResult<Option<String>> {
    Ok(c.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0)).optional()?)
}

pub async fn allow_signup(st: &AppState) -> bool {
    let env_default = st.cfg.allow_signup;
    st.db
        .call(move |c| Ok(setting(c, "allow_signup")?.map(|v| v == "true").unwrap_or(env_default)))
        .await
        .unwrap_or(env_default)
}

pub async fn list_users(State(st): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    require_admin(&user)?;
    let rows = st
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT u.id, u.email, u.name, u.color, u.is_admin, u.created_at,
                        (SELECT COUNT(*) FROM workspace_members m WHERE m.user_id = u.id),
                        (SELECT MAX(created_at) FROM sessions s WHERE s.user_id = u.id)
                 FROM users u ORDER BY u.created_at",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "id": r.get::<_, String>(0)?,
                        "email": r.get::<_, String>(1)?,
                        "name": r.get::<_, String>(2)?,
                        "color": r.get::<_, String>(3)?,
                        "isAdmin": r.get::<_, i64>(4)? != 0,
                        "createdAt": ts(r.get(5)?),
                        "workspaceCount": r.get::<_, i64>(6)?,
                        "lastSignIn": r.get::<_, Option<i64>>(7)?.map(ts),
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUser {
    is_admin: Option<bool>,
}

pub async fn update_user(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateUser>,
) -> AppResult<Json<Value>> {
    require_admin(&user)?;
    if let Some(admin) = req.is_admin {
        st.db
            .call(move |c| {
                if !admin {
                    let admins: i64 = c.query_row("SELECT COUNT(*) FROM users WHERE is_admin = 1 AND id != ?1", params![id], |r| r.get(0))?;
                    if admins == 0 {
                        return Err(AppError::bad_request("there must be at least one administrator"));
                    }
                }
                let n = c.execute("UPDATE users SET is_admin = ?1 WHERE id = ?2", params![admin as i64, id])?;
                if n == 0 {
                    return Err(AppError::not_found("user"));
                }
                Ok(())
            })
            .await?;
    }
    Ok(Json(json!({})))
}

/// Set a temporary password (there is no email on a self-hosted box) and sign
/// the user out everywhere. Returns the password to hand over.
pub async fn reset_password(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    require_admin(&user)?;
    let temp = random_string(14);
    let hash = hash_password(&temp)?;
    st.db
        .call(move |c| {
            let n = c.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![hash, id])?;
            if n == 0 {
                return Err(AppError::not_found("user"));
            }
            c.execute("DELETE FROM sessions WHERE user_id = ?1", params![id])?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "password": temp })))
}

pub async fn delete_user(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    require_admin(&user)?;
    if id == user.id {
        return Err(AppError::bad_request("you can't delete your own account here"));
    }
    st.db
        .call(move |c| {
            let tx = c.transaction()?;
            // Workspaces they share where they're the only owner would become ownerless.
            let blocked: Option<String> = tx
                .query_row(
                    "SELECT w.name FROM workspaces w
                     JOIN workspace_members m ON m.workspace_id = w.id AND m.user_id = ?1 AND m.role = 'owner'
                     WHERE (SELECT COUNT(*) FROM workspace_members o WHERE o.workspace_id = w.id AND o.role = 'owner') = 1
                       AND (SELECT COUNT(*) FROM workspace_members o WHERE o.workspace_id = w.id) > 1
                     LIMIT 1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(name) = blocked {
                return Err(AppError::bad_request(format!(
                    "they are the only owner of the shared workspace \"{name}\"; make someone else an owner first"
                )));
            }
            // Personal workspaces (only member) go with them.
            tx.execute(
                "DELETE FROM workspaces WHERE id IN (
                   SELECT m.workspace_id FROM workspace_members m WHERE m.user_id = ?1
                     AND (SELECT COUNT(*) FROM workspace_members o WHERE o.workspace_id = m.workspace_id) = 1)",
                params![id],
            )?;
            let n = tx.execute("DELETE FROM users WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(AppError::not_found("user"));
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({})))
}

pub async fn get_settings(State(st): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    require_admin(&user)?;
    let allow = allow_signup(&st).await;
    Ok(Json(json!({
        "allowSignup": allow,
        "instanceName": st.cfg.instance_name,
        "publicUrl": st.cfg.public_url,
        "version": env!("CARGO_PKG_VERSION"),
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsReq {
    allow_signup: Option<bool>,
}

pub async fn update_settings(State(st): State<AppState>, user: AuthUser, Json(req): Json<SettingsReq>) -> AppResult<Json<Value>> {
    require_admin(&user)?;
    if let Some(v) = req.allow_signup {
        st.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES ('allow_signup', ?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    params![if v { "true" } else { "false" }, now()],
                )?;
                Ok(())
            })
            .await?;
    }
    get_settings(State(st), user).await
}

/// What a full backup contains (sizes), for the Admin page.
pub async fn backup_info(State(st): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    require_admin(&user)?;
    Ok(Json(crate::backup::info(&st).await?))
}

/// Download a full-server backup (.zip). Browser sessions only: API and agent
/// tokens can't fetch it, since it holds everyone's data.
pub async fn backup(State(st): State<AppState>, user: AuthUser) -> AppResult<Response> {
    require_admin(&user)?;
    let (file, size, filename) = crate::backup::create(&st).await?;
    tracing::info!("{} downloaded a full backup ({size} bytes)", user.email);
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(tokio::fs::File::from_std(file)));
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_LENGTH, size)
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\""))
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .map_err(AppError::internal)
}
