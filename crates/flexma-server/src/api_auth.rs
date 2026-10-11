//! Setup, signup, login, logout and profile.

use crate::auth::{self, AuthUser};
use crate::db::{new_id, now, random_color};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::response::{AppendHeaders, IntoResponse};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

pub async fn setup(State(st): State<AppState>) -> AppResult<Json<Value>> {
    let users: i64 = st.db.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?)).await?;
    let allow = crate::api_admin::allow_signup(&st).await;
    Ok(Json(json!({
        "needsSetup": users == 0,
        "allowSignup": users == 0 || allow,
        "version": env!("CARGO_PKG_VERSION"),
        "instanceName": st.cfg.instance_name,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignupReq {
    email: String,
    name: String,
    password: String,
    invite_token: Option<String>,
    workspace_name: Option<String>,
}

fn valid_email(e: &str) -> bool {
    let e = e.trim();
    e.len() <= 254 && e.contains('@') && !e.starts_with('@') && !e.ends_with('@') && !e.contains(' ')
}

fn user_json(u: &AuthUser) -> Value {
    json!({ "id": u.id, "email": u.email, "name": u.name, "isAdmin": u.is_admin, "color": u.color })
}

pub async fn signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> AppResult<impl IntoResponse> {
    let email = req.email.trim().to_lowercase();
    let name = req.name.trim().to_string();
    if !valid_email(&email) {
        return Err(AppError::bad_request("enter a valid email address"));
    }
    if name.is_empty() || name.len() > 80 {
        return Err(AppError::bad_request("enter your name"));
    }
    if req.password.len() < 8 {
        return Err(AppError::bad_request("password must be at least 8 characters"));
    }
    let hash = auth::hash_password(&req.password)?;
    let allow = crate::api_admin::allow_signup(&st).await;
    let invite = req.invite_token.clone();
    let ws_name = req.workspace_name.clone();
    let user = st
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            let users: i64 = tx.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
            let first = users == 0;
            let invite_row: Option<(String, String)> = match &invite {
                Some(t) => tx
                    .query_row(
                        "SELECT workspace_id, role FROM invites WHERE token = ?1 AND expires_at > ?2",
                        params![t, now()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?,
                None => None,
            };
            if !first && !allow && invite_row.is_none() {
                return Err(AppError::forbidden("sign-up is invite-only on this server; ask an admin for an invite link"));
            }
            let exists: Option<String> =
                tx.query_row("SELECT id FROM users WHERE email = ?1", params![email], |r| r.get(0)).optional()?;
            if exists.is_some() {
                return Err(AppError::conflict("an account with this email already exists"));
            }
            let id = new_id();
            let color = random_color();
            tx.execute(
                "INSERT INTO users (id, email, name, password_hash, is_admin, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, email, name, hash, first as i64, color, now()],
            )?;
            if let Some((ws, role)) = invite_row {
                tx.execute(
                    "INSERT OR IGNORE INTO workspace_members (workspace_id, user_id, role, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![ws, id, role, now()],
                )?;
            }
            if let Some(wn) = ws_name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
                let ws = new_id();
                tx.execute(
                    "INSERT INTO workspaces (id, name, created_by, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![ws, wn, id, now()],
                )?;
                tx.execute(
                    "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) VALUES (?1, ?2, 'owner', ?3)",
                    params![ws, id, now()],
                )?;
                tx.execute(
                    "INSERT INTO projects (id, workspace_id, name, created_at) VALUES (?1, ?2, 'Drafts', ?3)",
                    params![new_id(), ws, now()],
                )?;
            }
            tx.commit()?;
            Ok(AuthUser { id, email, name, is_admin: first, color, via_token: false, token_name: None })
        })
        .await?;
    let token = auth::create_session(&st.db, &user.id).await?;
    Ok((
        AppendHeaders([(header::SET_COOKIE, auth::session_cookie(&token, st.cfg.secure_cookies))]),
        Json(json!({ "user": user_json(&user) })),
    ))
}

#[derive(Deserialize)]
pub struct LoginReq {
    email: String,
    password: String,
}

pub async fn login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> AppResult<impl IntoResponse> {
    let email = req.email.trim().to_lowercase();
    let row: Option<(AuthUser, String)> = st
        .db
        .call(move |c| {
            Ok(c.query_row(
                "SELECT id, email, name, is_admin, color, password_hash FROM users WHERE email = ?1",
                params![email],
                |r| {
                    Ok((
                        AuthUser {
                            id: r.get(0)?,
                            email: r.get(1)?,
                            name: r.get(2)?,
                            is_admin: r.get::<_, i64>(3)? != 0,
                            color: r.get(4)?,
                            via_token: false,
                            token_name: None,
                        },
                        r.get(5)?,
                    ))
                },
            )
            .optional()?)
        })
        .await?;
    let pw = req.password.clone();
    let ok_user = match row {
        Some((u, hash)) => {
            let ok = tokio::task::spawn_blocking(move || auth::verify_password(&pw, &hash)).await.unwrap_or(false);
            ok.then_some(u)
        }
        None => {
            // Spend similar time to avoid user enumeration via timing.
            let _ = tokio::task::spawn_blocking(move || auth::hash_password(&pw)).await;
            None
        }
    };
    let user = ok_user.ok_or_else(|| AppError::new(axum::http::StatusCode::UNAUTHORIZED, "wrong email or password"))?;
    let token = auth::create_session(&st.db, &user.id).await?;
    Ok((
        AppendHeaders([(header::SET_COOKIE, auth::session_cookie(&token, st.cfg.secure_cookies))]),
        Json(json!({ "user": user_json(&user) })),
    ))
}

pub async fn logout(State(st): State<AppState>, headers: HeaderMap) -> AppResult<impl IntoResponse> {
    if let Some(tok) = auth::cookie_value(&headers, auth::SESSION_COOKIE) {
        let hash = crate::db::sha256_hex(&tok);
        st.db
            .call(move |c| {
                c.execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash])?;
                Ok(())
            })
            .await?;
    }
    Ok((AppendHeaders([(header::SET_COOKIE, auth::clear_cookie())]), Json(json!({}))))
}

pub async fn me(user: AuthUser) -> Json<Value> {
    Json(json!({ "user": user_json(&user) }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMe {
    name: Option<String>,
    password: Option<String>,
    current_password: Option<String>,
    color: Option<String>,
}

pub async fn update_me(State(st): State<AppState>, user: AuthUser, Json(req): Json<UpdateMe>) -> AppResult<Json<Value>> {
    let mut u = user.clone();
    if let Some(n) = req.name.map(|s| s.trim().to_string()) {
        if n.is_empty() || n.len() > 80 {
            return Err(AppError::bad_request("invalid name"));
        }
        u.name = n;
    }
    if let Some(c) = req.color {
        if flexma_doc::Color::parse(&c).is_none() {
            return Err(AppError::bad_request("invalid color"));
        }
        u.color = c;
    }
    let mut new_hash = None;
    if let Some(pw) = req.password {
        if pw.len() < 8 {
            return Err(AppError::bad_request("password must be at least 8 characters"));
        }
        let uid = user.id.clone();
        let current: String = st
            .db
            .call(move |c| Ok(c.query_row("SELECT password_hash FROM users WHERE id = ?1", params![uid], |r| r.get(0))?))
            .await?;
        let cur_pw = req.current_password.unwrap_or_default();
        if !auth::verify_password(&cur_pw, &current) {
            return Err(AppError::forbidden("current password is incorrect"));
        }
        new_hash = Some(auth::hash_password(&pw)?);
    }
    let u2 = u.clone();
    st.db
        .call(move |c| {
            c.execute("UPDATE users SET name = ?1, color = ?2 WHERE id = ?3", params![u2.name, u2.color, u2.id])?;
            if let Some(h) = new_hash {
                c.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![h, u2.id])?;
            }
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "user": user_json(&u) })))
}
