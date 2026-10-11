//! OAuth 2.1 authorization server for remote MCP clients, following the MCP
//! authorization spec: protected-resource metadata (RFC 9728), authorization
//! server metadata (RFC 8414), dynamic client registration (RFC 7591) and the
//! authorization-code flow with PKCE (S256).
//!
//! This makes Flexma's MCP work like hosted MCP servers (e.g. Figma's): run
//! `claude mcp add --transport http flexma https://your-server/mcp`, and the
//! client opens a browser to sign in and approve access. Access tokens are
//! ordinary API tokens, so they show up (and can be revoked) under AI Agents.

use crate::auth::{self, AuthUser};
use crate::db::{new_id, now, random_string, sha256_hex};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Form, Json};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

const CODE_TTL: i64 = 600;
const ACCESS_TTL: i64 = 30 * 86400;
const REFRESH_TTL: i64 = 365 * 86400;

pub fn resource_metadata_url(base: &str) -> String {
    format!("{base}/.well-known/oauth-protected-resource")
}

pub async fn protected_resource(State(st): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let base = st.base_url(&headers);
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "bearer_methods_supported": ["header"],
        "resource_name": st.cfg.instance_name,
    }))
}

pub async fn authorization_server(State(st): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let base = st.base_url(&headers);
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": ["mcp"],
    }))
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    (status, Json(json!({ "error": error, "error_description": description }))).into_response()
}

fn valid_redirect(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    if lower.contains('#') {
        return false;
    }
    lower.starts_with("https://")
        || lower.starts_with("http://localhost")
        || lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://[::1]")
        // Native app custom schemes (e.g. cursor://, vscode://)
        || (lower.contains("://") && !lower.starts_with("http://") && !lower.starts_with("javascript:") && !lower.starts_with("data:"))
}

#[derive(Deserialize)]
pub struct RegisterReq {
    #[serde(default)]
    redirect_uris: Vec<String>,
    client_name: Option<String>,
}

pub async fn register(State(st): State<AppState>, Json(req): Json<RegisterReq>) -> Response {
    if req.redirect_uris.is_empty() || req.redirect_uris.len() > 10 || !req.redirect_uris.iter().all(|u| valid_redirect(u)) {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_redirect_uri", "redirect_uris must be https or loopback URLs");
    }
    let name: String = req
        .client_name
        .unwrap_or_else(|| "MCP client".into())
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    let client_id = format!("fxc_{}", random_string(24));
    let uris = serde_json::to_string(&req.redirect_uris).unwrap_or_default();
    let (cid, n, u) = (client_id.clone(), name.clone(), uris);
    let res = st
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![cid, n, u, now()],
            )?;
            Ok(())
        })
        .await;
    if res.is_err() {
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not register client");
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": client_id,
            "client_id_issued_at": now(),
            "client_name": name,
            "redirect_uris": req.redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })),
    )
        .into_response()
}

#[derive(Deserialize, Clone)]
pub struct AuthorizeParams {
    response_type: Option<String>,
    client_id: String,
    redirect_uri: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    state: Option<String>,
    scope: Option<String>,
}

struct Client {
    name: String,
    redirect_uris: Vec<String>,
}

async fn load_client(st: &AppState, client_id: &str) -> Option<Client> {
    let cid = client_id.to_string();
    st.db
        .call(move |c| {
            Ok(c.query_row(
                "SELECT client_name, redirect_uris FROM oauth_clients WHERE client_id = ?1",
                params![cid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?)
        })
        .await
        .ok()
        .flatten()
        .map(|(name, uris)| Client { name, redirect_uris: serde_json::from_str(&uris).unwrap_or_default() })
}

fn esc(s: &str) -> String {
    flexma_doc::svg::escape(s)
}

/// The Silly Toast Studio mark (the studio behind Flexma).
pub const SILLY_TOAST_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" aria-hidden="true"><rect width="64" height="64" rx="16" fill="#F28C28"/><path d="M15 27C5 17 23 9 33 9s26 8 16 18l2 24H16z" fill="#FFF3D6" stroke="#26211D" stroke-width="4" stroke-linejoin="round"/><circle cx="25" cy="32" r="2.5" fill="#26211D"/><circle cx="39" cy="30" r="2.5" fill="#26211D"/><path d="m29 41 8-1" stroke="#26211D" stroke-width="3" stroke-linecap="round"/></svg>"##;

fn page(title: &str, body: &str) -> Html<String> {
    let toast = SILLY_TOAST_SVG;
    Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title><style>
:root{{--bg:#f5f5f7;--card:#fff;--text:#1d1d1f;--muted:#6e6e73;--border:#e2e2e6;--accent:#6d5dfc}}
@media (prefers-color-scheme:dark){{:root{{--bg:#111113;--card:#1c1c1f;--text:#f2f2f4;--muted:#9a9aa2;--border:#2c2c31}}}}
*{{box-sizing:border-box}}body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--text);font:14px/1.5 Inter,system-ui,-apple-system,Segoe UI,sans-serif;padding:16px}}
.card{{width:100%;max-width:400px;background:var(--card);border:1px solid var(--border);border-radius:14px;padding:28px;box-shadow:0 10px 30px #0000000f}}
h1{{font-size:18px;margin:0 0 6px}}p{{color:var(--muted);margin:0 0 18px}}label{{display:block;font-size:12px;color:var(--muted);margin:12px 0 4px}}
input{{width:100%;padding:10px 12px;border:1px solid var(--border);border-radius:8px;background:transparent;color:var(--text);font:inherit}}
.row{{display:flex;gap:8px;margin-top:20px}}button{{flex:1;padding:10px 14px;border-radius:8px;border:1px solid var(--border);background:transparent;color:var(--text);font:inherit;font-weight:600;cursor:pointer}}
button.primary{{background:var(--accent);border-color:var(--accent);color:#fff}}.logo{{width:36px;height:36px;border-radius:9px;background:var(--accent);display:grid;place-items:center;color:#fff;font-weight:800;margin-bottom:14px}}
ul{{color:var(--muted);padding-left:18px;margin:0 0 4px}}.err{{color:#e5484d;margin-top:10px}}
main{{width:100%;max-width:400px;display:flex;flex-direction:column;align-items:center;gap:18px}}
.madeby{{display:inline-flex;align-items:center;gap:6px;color:var(--muted);text-decoration:none;font-size:11px}}.madeby b{{color:var(--text);font-weight:800}}.madeby svg{{width:18px;height:18px;border-radius:5px}}</style></head>
<body><main><div class="card"><div class="logo">F</div>{body}</div>
<a class="madeby" href="https://sillytoast.studio" target="_blank" rel="noreferrer">{toast}<span>Made by <b>silly toast</b> studio</span></a></main></body></html>"#
    ))
}

fn hidden_fields(p: &AuthorizeParams) -> String {
    let mut s = String::new();
    let mut add = |k: &str, v: &Option<String>| {
        if let Some(v) = v {
            s.push_str(&format!(r#"<input type="hidden" name="{k}" value="{}">"#, esc(v)));
        }
    };
    add("client_id", &Some(p.client_id.clone()));
    add("redirect_uri", &p.redirect_uri);
    add("code_challenge", &p.code_challenge);
    add("code_challenge_method", &p.code_challenge_method);
    add("state", &p.state);
    add("scope", &p.scope);
    add("response_type", &p.response_type);
    s
}

fn consent_page(client: &Client, user: Option<&AuthUser>, p: &AuthorizeParams, error: Option<&str>) -> Html<String> {
    let who = match user {
        Some(u) => format!("<p>Signed in as <b>{}</b> ({})</p>", esc(&u.name), esc(&u.email)),
        None => r#"<label>Email</label><input name="email" type="email" autocomplete="username" required autofocus>
<label>Password</label><input name="password" type="password" autocomplete="current-password" required>"#
            .to_string(),
    };
    let err = error.map(|e| format!(r#"<div class="err">{}</div>"#, esc(e))).unwrap_or_default();
    page(
        "Authorize access — Flexma",
        &format!(
            r#"<h1>{name} wants to use Flexma</h1>
<p>It will be able to act as you through the Flexma MCP server:</p>
<ul><li>read your workspaces, projects and design files</li><li>create and edit designs (edits appear live to collaborators)</li></ul>
<form method="post" action="/oauth/authorize">{hidden}{who}{err}
<div class="row"><button name="action" value="deny">Deny</button><button class="primary" name="action" value="allow">Allow access</button></div></form>"#,
            name = esc(&client.name),
            hidden = hidden_fields(p),
        ),
    )
}

fn validate_params(p: &AuthorizeParams, client: &Client) -> Result<String, String> {
    if p.response_type.as_deref() != Some("code") {
        return Err("response_type must be 'code'".into());
    }
    let redirect = match &p.redirect_uri {
        Some(r) => r.clone(),
        None if client.redirect_uris.len() == 1 => client.redirect_uris[0].clone(),
        None => return Err("redirect_uri is required".into()),
    };
    if !client.redirect_uris.contains(&redirect) {
        return Err("redirect_uri is not registered for this client".into());
    }
    if p.code_challenge.as_deref().unwrap_or("").is_empty() || p.code_challenge_method.as_deref() != Some("S256") {
        return Err("PKCE with code_challenge_method=S256 is required".into());
    }
    Ok(redirect)
}

pub async fn authorize_get(State(st): State<AppState>, headers: HeaderMap, Query(p): Query<AuthorizeParams>) -> Response {
    let Some(client) = load_client(&st, &p.client_id).await else {
        return page("Unknown client", "<h1>Unknown application</h1><p>This client is not registered with this Flexma server. Try reconnecting from your MCP client.</p>").into_response();
    };
    if let Err(e) = validate_params(&p, &client) {
        return page("Invalid request", &format!("<h1>Invalid authorization request</h1><p>{}</p>", esc(&e))).into_response();
    }
    let user = auth::authenticate(&st.db, &headers).await.ok().flatten().filter(|u| !u.via_token);
    consent_page(&client, user.as_ref(), &p, None).into_response()
}

#[derive(Deserialize)]
pub struct AuthorizeForm {
    #[serde(flatten)]
    params: AuthorizeParams,
    action: Option<String>,
    email: Option<String>,
    password: Option<String>,
}

fn redirect_with(uri: &str, pairs: &[(&str, &str)]) -> Response {
    let mut url = uri.to_string();
    let sep = if url.contains('?') { '&' } else { '?' };
    url.push(sep);
    let q: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect();
    url.push_str(&q.join("&"));
    Redirect::to(&url).into_response()
}

fn urlencode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

pub async fn authorize_post(State(st): State<AppState>, headers: HeaderMap, Form(f): Form<AuthorizeForm>) -> Response {
    // Same-origin form posts only.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let host = headers
            .get("x-forwarded-host")
            .or_else(|| headers.get(header::HOST))
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if origin != "null" && origin.split("://").nth(1).unwrap_or(origin) != host {
            return oauth_error(StatusCode::FORBIDDEN, "invalid_request", "cross-origin request rejected");
        }
    }
    let p = f.params.clone();
    let Some(client) = load_client(&st, &p.client_id).await else {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_client", "unknown client");
    };
    let redirect = match validate_params(&p, &client) {
        Ok(r) => r,
        Err(e) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", &e),
    };
    let state = p.state.clone().unwrap_or_default();
    let base = st.base_url(&headers);
    if f.action.as_deref() != Some("allow") {
        return redirect_with(&redirect, &[("error", "access_denied"), ("state", &state), ("iss", &base)]);
    }
    let mut user = auth::authenticate(&st.db, &headers).await.ok().flatten().filter(|u| !u.via_token);
    if user.is_none() {
        let email = f.email.unwrap_or_default().trim().to_lowercase();
        let pw = f.password.unwrap_or_default();
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
            .await
            .ok()
            .flatten();
        user = match row {
            Some((u, hash)) => {
                let ok = tokio::task::spawn_blocking(move || auth::verify_password(&pw, &hash)).await.unwrap_or(false);
                ok.then_some(u)
            }
            None => None,
        };
        if user.is_none() {
            return consent_page(&client, None, &p, Some("Wrong email or password.")).into_response();
        }
    }
    let user = user.unwrap();
    let code = random_string(40);
    let hash = sha256_hex(&code);
    let challenge = p.code_challenge.clone().unwrap_or_default();
    let (cid, uid, ruri, scope) = (p.client_id.clone(), user.id.clone(), redirect.clone(), p.scope.clone());
    let res = st
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO oauth_codes (code_hash, client_id, user_id, redirect_uri, code_challenge, scope, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![hash, cid, uid, ruri, challenge, scope, now() + CODE_TTL],
            )?;
            c.execute("DELETE FROM oauth_codes WHERE expires_at < ?1", params![now()])?;
            Ok(())
        })
        .await;
    if res.is_err() {
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not issue code");
    }
    redirect_with(&redirect, &[("code", &code), ("state", &state), ("iss", &base)])
}

#[derive(Deserialize)]
pub struct TokenForm {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
}

fn pkce_s256(verifier: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Issue an access token (an API token row) plus a refresh token.
async fn issue(st: &AppState, user_id: String, client_id: String, replace_token: Option<String>) -> Response {
    let client = load_client(st, &client_id).await;
    let label = format!("{} (OAuth)", client.map(|c| c.name).unwrap_or_else(|| "MCP client".into()));
    let (access, prefix) = auth::new_api_token();
    let refresh = format!("fxr_{}", random_string(40));
    let (ah, rh) = (sha256_hex(&access), sha256_hex(&refresh));
    let res = st
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            if let Some(old) = replace_token {
                tx.execute("DELETE FROM api_tokens WHERE id = ?1", params![old])?;
            }
            let tid = new_id();
            tx.execute(
                "INSERT INTO api_tokens (id, user_id, name, prefix, token_hash, created_at, expires_at, oauth_client_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![tid, user_id, label, prefix, ah, now(), now() + ACCESS_TTL, client_id],
            )?;
            tx.execute(
                "INSERT INTO oauth_refresh_tokens (token_hash, client_id, user_id, api_token_id, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![rh, client_id, user_id, tid, now() + REFRESH_TTL],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await;
    if res.is_err() {
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not issue token");
    }
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": ACCESS_TTL,
            "refresh_token": refresh,
            "scope": "mcp",
        })),
    )
        .into_response()
}

pub async fn token(State(st): State<AppState>, Form(f): Form<TokenForm>) -> Response {
    match f.grant_type.as_str() {
        "authorization_code" => {
            let (Some(code), Some(verifier)) = (f.code, f.code_verifier) else {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "code and code_verifier are required");
            };
            let hash = sha256_hex(&code);
            let row = st
                .db
                .call(move |c| {
                    let row: Option<(String, String, String, String, i64)> = c
                        .query_row(
                            "SELECT client_id, user_id, redirect_uri, code_challenge, expires_at FROM oauth_codes WHERE code_hash = ?1",
                            params![hash],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                        )
                        .optional()?;
                    // Codes are single-use.
                    c.execute("DELETE FROM oauth_codes WHERE code_hash = ?1", params![hash])?;
                    Ok(row)
                })
                .await
                .ok()
                .flatten();
            let Some((client_id, user_id, redirect_uri, challenge, expires)) = row else {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "authorization code is invalid or used");
            };
            if expires < now() {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "authorization code expired");
            }
            if f.client_id.as_deref().is_some_and(|c| c != client_id) {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "client mismatch");
            }
            if f.redirect_uri.as_deref().is_some_and(|r| r != redirect_uri) {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "redirect_uri mismatch");
            }
            if pkce_s256(&verifier) != challenge {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "PKCE verification failed");
            }
            issue(&st, user_id, client_id, None).await
        }
        "refresh_token" => {
            let Some(rt) = f.refresh_token else {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "refresh_token is required");
            };
            let hash = sha256_hex(&rt);
            let row = st
                .db
                .call(move |c| {
                    let row: Option<(String, String, Option<String>, i64)> = c
                        .query_row(
                            "SELECT client_id, user_id, api_token_id, expires_at FROM oauth_refresh_tokens WHERE token_hash = ?1",
                            params![hash],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                        )
                        .optional()?;
                    // Rotate: refresh tokens are single-use.
                    c.execute("DELETE FROM oauth_refresh_tokens WHERE token_hash = ?1", params![hash])?;
                    Ok(row)
                })
                .await
                .ok()
                .flatten();
            let Some((client_id, user_id, old_token, expires)) = row else {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "refresh token is invalid");
            };
            if expires < now() || f.client_id.as_deref().is_some_and(|c| c != client_id) {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "refresh token is invalid");
            }
            issue(&st, user_id, client_id, old_token).await
        }
        _ => oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type", "use authorization_code or refresh_token"),
    }
}

#[derive(Deserialize)]
pub struct RevokeForm {
    token: String,
}

pub async fn revoke(State(st): State<AppState>, Form(f): Form<RevokeForm>) -> StatusCode {
    let hash = sha256_hex(&f.token);
    let _ = st
        .db
        .call(move |c| {
            c.execute("DELETE FROM api_tokens WHERE token_hash = ?1 AND oauth_client_id IS NOT NULL", params![hash])?;
            c.execute("DELETE FROM oauth_refresh_tokens WHERE token_hash = ?1", params![hash])?;
            Ok(())
        })
        .await;
    StatusCode::OK
}
