//! API tokens for AI agents, agent setup snippets, and the skill files served to
//! Claude Code / Codex installs.

use crate::auth::{new_api_token, AuthUser};
use crate::db::{new_id, now, sha256_hex, ts};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

pub async fn list(State(st): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let rows = st
        .db
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, name, prefix, created_at, last_used_at FROM api_tokens WHERE user_id = ?1 ORDER BY created_at DESC",
            )?;
            let rows = stmt
                .query_map(params![user.id], |r| {
                    Ok(json!({
                        "id": r.get::<_, String>(0)?,
                        "name": r.get::<_, String>(1)?,
                        "prefix": r.get::<_, String>(2)?,
                        "createdAt": ts(r.get(3)?),
                        "lastUsedAt": r.get::<_, Option<i64>>(4)?.map(ts),
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
    name: Option<String>,
}

pub async fn create(State(st): State<AppState>, user: AuthUser, Json(req): Json<CreateReq>) -> AppResult<Json<Value>> {
    if user.via_token {
        return Err(AppError::forbidden("API tokens cannot create other tokens"));
    }
    let name = req.name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "AI agent".into());
    if name.len() > 80 {
        return Err(AppError::bad_request("token name is too long"));
    }
    let (token, prefix) = new_api_token();
    let hash = sha256_hex(&token);
    let id = new_id();
    let (id2, name2, prefix2) = (id.clone(), name.clone(), prefix.clone());
    st.db
        .call(move |c| {
            c.execute(
                "INSERT INTO api_tokens (id, user_id, name, prefix, token_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id2, user.id, name2, prefix2, hash, now()],
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "id": id, "name": name, "prefix": prefix, "token": token })))
}

pub async fn delete(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let n = st
        .db
        .call(move |c| Ok(c.execute("DELETE FROM api_tokens WHERE id = ?1 AND user_id = ?2", params![id, user.id])?))
        .await?;
    if n == 0 {
        return Err(AppError::not_found("token"));
    }
    Ok(Json(json!({})))
}

pub fn setup_snippets(base: &str) -> Value {
    let mcp = format!("{base}/mcp");
    json!({
        "mcpUrl": mcp,
        "skillInstallCommand": format!("curl -fsSL {base}/agents/install.sh | sh"),
        // OAuth (recommended): the client opens a browser to sign in, like hosted MCP servers.
        "claudeCommand": format!("claude mcp add --scope user --transport http flexma {mcp}"),
        "codexCommand": format!("codex mcp add flexma --url {mcp} && codex mcp login flexma"),
        "codexConfig": format!(
            "[mcp_servers.flexma]\nurl = \"{mcp}\"\n# Optional: let Codex use Flexma tools without asking each time (needed for `codex exec`)\n# default_tools_approval_mode = \"approve\""
        ),
        // API token alternative for headless / CI agents.
        "claudeTokenCommand": format!(
            "claude mcp add --scope user --transport http flexma {mcp} --header \"Authorization: Bearer $FLEXMA_TOKEN\""
        ),
        "codexTokenCommand": format!("codex mcp add flexma --url {mcp} --bearer-token-env-var FLEXMA_TOKEN"),
        "codexTokenConfig": format!(
            "[mcp_servers.flexma]\nurl = \"{mcp}\"\nbearer_token_env_var = \"FLEXMA_TOKEN\"\ndefault_tools_approval_mode = \"approve\""
        ),
    })
}

pub async fn agent_setup(State(st): State<AppState>, headers: HeaderMap, _user: AuthUser) -> Json<Value> {
    Json(setup_snippets(&st.base_url(&headers)))
}

// ---------- skill files ----------

/// The Flexma skill, embedded at build time from `skills/flexma`.
pub const SKILL_FILES: &[(&str, &str)] = &[
    ("SKILL.md", include_str!("../../../skills/flexma/SKILL.md")),
    ("references/node-spec.md", include_str!("../../../skills/flexma/references/node-spec.md")),
    ("references/recipes.md", include_str!("../../../skills/flexma/references/recipes.md")),
];

pub async fn skill_file(Path(path): Path<String>) -> Response {
    match SKILL_FILES.iter().find(|(p, _)| *p == path) {
        Some((_, body)) => ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], *body).into_response(),
        None => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

pub async fn install_script(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let base = st.base_url(&headers);
    let snippets = setup_snippets(&base);
    let mut files = String::new();
    for (p, _) in SKILL_FILES {
        files.push_str(&format!("  fetch \"{p}\"\n"));
    }
    let script = format!(
        r#"#!/bin/sh
# Flexma skill installer for Claude Code and Codex.
# Installs the "flexma" skill into ~/.claude/skills and ~/.codex/skills.
set -e
BASE="{base}"
install_to() {{
  DEST="$1/flexma"
  mkdir -p "$DEST/references"
  fetch() {{ curl -fsSL "$BASE/agents/skills/flexma/$1" -o "$DEST/$1"; }}
{files}  echo "  installed skill -> $DEST"
}}
echo "Installing the Flexma skill from $BASE"
install_to "$HOME/.claude/skills"
install_to "$HOME/.codex/skills"
echo ""
echo "Next, connect the Flexma MCP server (your agent will open a browser to sign in):"
echo ""
echo "  Claude Code:"
echo '    {claude}'
echo ""
echo "  Codex:"
echo '    {codex}'
echo ""
"#,
        claude = snippets["claudeCommand"].as_str().unwrap_or_default().replace('\'', "'\\''"),
        codex = snippets["codexCommand"].as_str().unwrap_or_default().replace('\'', "'\\''"),
    );
    ([(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")], script).into_response()
}
