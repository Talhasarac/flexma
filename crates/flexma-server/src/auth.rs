//! Authentication: password hashing, browser sessions (cookie) and API tokens
//! (bearer), plus the `AuthUser` extractor and workspace/file access checks.

use crate::db::{now, random_string, sha256_hex, Db};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, Method};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

pub const SESSION_COOKIE: &str = "flexma_session";
pub const SESSION_DAYS: i64 = 30;
pub const TOKEN_PREFIX: &str = "fxm_";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
    pub color: String,
    /// Authenticated via API token (agents) rather than a browser session.
    #[serde(skip)]
    pub via_token: bool,
    #[serde(skip)]
    pub token_name: Option<String>,
}

pub fn hash_password(pw: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(AppError::internal)
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

/// Create a session row; returns the raw token for the cookie.
pub async fn create_session(db: &Db, user_id: &str) -> AppResult<String> {
    let token = random_string(40);
    let hash = sha256_hex(&token);
    let uid = user_id.to_string();
    db.call(move |c| {
        let t = now();
        c.execute(
            "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![hash, uid, t, t + SESSION_DAYS * 86400],
        )?;
        // Opportunistic cleanup.
        c.execute("DELETE FROM sessions WHERE expires_at < ?1", params![t])?;
        Ok(())
    })
    .await?;
    Ok(token)
}

pub fn session_cookie(token: &str, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        SESSION_DAYS * 86400,
        if secure { "; Secure" } else { "" }
    )
}

pub fn clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    for h in headers.get_all(header::COOKIE) {
        let Ok(s) = h.to_str() else { continue };
        for part in s.split(';') {
            let part = part.trim();
            if let Some(v) = part.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let v = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer "))?;
    Some(v.trim().to_string())
}

fn user_from_row(r: &rusqlite::Row) -> rusqlite::Result<AuthUser> {
    Ok(AuthUser {
        id: r.get(0)?,
        email: r.get(1)?,
        name: r.get(2)?,
        is_admin: r.get::<_, i64>(3)? != 0,
        color: r.get(4)?,
        via_token: false,
        token_name: None,
    })
}

/// Resolve the user for a request from either a bearer API token or the session cookie.
pub async fn authenticate(db: &Db, headers: &HeaderMap) -> AppResult<Option<AuthUser>> {
    if let Some(tok) = bearer_token(headers) {
        let hash = sha256_hex(&tok);
        return db
            .call(move |c| {
                let row = c
                    .query_row(
                        "SELECT u.id, u.email, u.name, u.is_admin, u.color, t.id, t.name FROM api_tokens t
                         JOIN users u ON u.id = t.user_id
                         WHERE t.token_hash = ?1 AND (t.expires_at IS NULL OR t.expires_at > ?2)",
                        params![hash, now()],
                        |r| Ok((user_from_row(r)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?)),
                    )
                    .optional()?;
                Ok(match row {
                    Some((mut u, tid, tname)) => {
                        c.execute("UPDATE api_tokens SET last_used_at = ?1 WHERE id = ?2", params![now(), tid])?;
                        u.via_token = true;
                        u.token_name = Some(tname);
                        Some(u)
                    }
                    None => None,
                })
            })
            .await;
    }
    if let Some(tok) = cookie_value(headers, SESSION_COOKIE) {
        let hash = sha256_hex(&tok);
        return db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT u.id, u.email, u.name, u.is_admin, u.color FROM sessions s
                     JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?1 AND s.expires_at > ?2",
                    params![hash, now()],
                    user_from_row,
                )
                .optional()?)
            })
            .await;
    }
    Ok(None)
}

/// Reject cross-site state-changing requests made with cookie auth.
fn check_origin(parts: &Parts) -> AppResult<()> {
    if matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return Ok(());
    }
    let Some(origin) = parts.headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return Ok(());
    };
    let host = parts
        .headers
        .get("x-forwarded-host")
        .or_else(|| parts.headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let origin_host = origin.split("://").nth(1).unwrap_or(origin);
    if origin_host == host {
        Ok(())
    } else {
        Err(AppError::forbidden("cross-origin request rejected"))
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let user = authenticate(&state.db, &parts.headers).await?.ok_or_else(AppError::unauthorized)?;
        if !user.via_token {
            check_origin(parts)?;
        }
        Ok(user)
    }
}

// ---------- roles & access ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Viewer = 0,
    Editor = 1,
    Owner = 2,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "viewer" => Some(Role::Viewer),
            "editor" => Some(Role::Editor),
            "owner" => Some(Role::Owner),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Owner => "owner",
        }
    }
    pub fn can_edit(self) -> bool {
        self >= Role::Editor
    }
}

pub fn workspace_role(c: &rusqlite::Connection, user_id: &str, workspace_id: &str) -> AppResult<Option<Role>> {
    let r: Option<String> = c
        .query_row(
            "SELECT role FROM workspace_members WHERE workspace_id = ?1 AND user_id = ?2",
            params![workspace_id, user_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(r.and_then(|s| Role::parse(&s)))
}

pub fn require_workspace(c: &rusqlite::Connection, user_id: &str, workspace_id: &str, min: Role) -> AppResult<Role> {
    match workspace_role(c, user_id, workspace_id)? {
        Some(r) if r >= min => Ok(r),
        Some(_) => Err(AppError::forbidden(format!("requires {} access to this workspace", min.as_str()))),
        None => Err(AppError::not_found("workspace")),
    }
}

/// Workspace id + caller's role for a project.
pub fn project_access(c: &rusqlite::Connection, user_id: &str, project_id: &str, min: Role) -> AppResult<(String, Role)> {
    let ws: Option<String> = c
        .query_row("SELECT workspace_id FROM projects WHERE id = ?1", params![project_id], |r| r.get(0))
        .optional()?;
    let ws = ws.ok_or_else(|| AppError::not_found("project"))?;
    let role = require_workspace(c, user_id, &ws, min).map_err(|e| {
        if e.status == axum::http::StatusCode::NOT_FOUND {
            AppError::not_found("project")
        } else {
            e
        }
    })?;
    Ok((ws, role))
}

/// (workspace id, project id, role) for a file.
pub fn file_access(c: &rusqlite::Connection, user_id: &str, file_id: &str, min: Role) -> AppResult<(String, String, Role)> {
    let row: Option<(String, String)> = c
        .query_row(
            "SELECT p.workspace_id, f.project_id FROM files f JOIN projects p ON p.id = f.project_id WHERE f.id = ?1",
            params![file_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (ws, proj) = row.ok_or_else(|| AppError::not_found("file"))?;
    let role = require_workspace(c, user_id, &ws, min).map_err(|e| {
        if e.status == axum::http::StatusCode::NOT_FOUND {
            AppError::not_found("file")
        } else {
            e
        }
    })?;
    Ok((ws, proj, role))
}

pub fn new_api_token() -> (String, String) {
    let token = format!("{TOKEN_PREFIX}{}", random_string(40));
    let prefix = token[..TOKEN_PREFIX.len() + 6].to_string();
    (token, prefix)
}
