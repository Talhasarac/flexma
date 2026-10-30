//! API tokens for AI agents, agent setup snippets, and the skill files served to
//! Claude Code / Codex / OpenCode installs.

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
    let n = st.db.call(move |c| delete_token(c, &id, &user.id)).await?;
    if n == 0 {
        return Err(AppError::not_found("token"));
    }
    st.rooms.recheck_all();
    Ok(Json(json!({})))
}

/// Delete one of a user's tokens. Returns the rows deleted (0: not theirs).
pub fn delete_token(c: &mut rusqlite::Connection, id: &str, user_id: &str) -> AppResult<usize> {
    let tx = c.transaction()?;
    // An OAuth client's refresh token would mint a new access token, so it goes too.
    tx.execute("DELETE FROM oauth_refresh_tokens WHERE api_token_id = ?1 AND user_id = ?2", params![id, user_id])?;
    let n = tx.execute("DELETE FROM api_tokens WHERE id = ?1 AND user_id = ?2", params![id, user_id])?;
    tx.commit()?;
    Ok(n)
}

pub fn setup_snippets(base: &str) -> Value {
    let mcp = format!("{base}/mcp");
    json!({
        "mcpUrl": mcp,
        "skillInstallCommand": format!("curl -fsSL {base}/agents/install.sh | sh"),
        // Optional: Unity projects (installs the flexma skill too).
        "unitySkillInstallCommand": format!("curl -fsSL {base}/agents/install/unity-ui-to-flexma.sh | sh"),
        // OAuth (recommended): the client opens a browser to sign in, like hosted MCP servers.
        "claudeCommand": format!("claude mcp add --scope user --transport http flexma {mcp}"),
        "codexCommand": format!("codex mcp add flexma --url {mcp} && codex mcp login flexma"),
        "codexConfig": format!(
            "[mcp_servers.flexma]\nurl = \"{mcp}\"\n# Optional: let Codex use Flexma tools without asking each time (needed for `codex exec`)\n# default_tools_approval_mode = \"approve\""
        ),
        "opencodeCommand": format!("opencode mcp add flexma --url {mcp} && opencode mcp auth flexma"),
        "opencodeConfig": opencode_config(&mcp, None),
        // API token alternative for headless / CI agents.
        "claudeTokenCommand": format!(
            "claude mcp add --scope user --transport http flexma {mcp} --header \"Authorization: Bearer $FLEXMA_TOKEN\""
        ),
        "codexTokenCommand": format!("codex mcp add flexma --url {mcp} --bearer-token-env-var FLEXMA_TOKEN"),
        "codexTokenConfig": format!(
            "[mcp_servers.flexma]\nurl = \"{mcp}\"\nbearer_token_env_var = \"FLEXMA_TOKEN\"\ndefault_tools_approval_mode = \"approve\""
        ),
        // OpenCode reads the token from FLEXMA_TOKEN when it connects ({env:…}).
        "opencodeTokenCommand": format!("opencode mcp add flexma --url {mcp} --header 'Authorization=Bearer {{env:FLEXMA_TOKEN}}'"),
        "opencodeTokenConfig": opencode_config(&mcp, Some("Bearer {env:FLEXMA_TOKEN}")),
        // Windows PowerShell variants (5.1 has no `&&`, and no `sh` to pipe into).
        "windows": {
            "skillInstallCommand": format!("irm {base}/agents/install.ps1 | iex"),
            "unitySkillInstallCommand": format!("irm {base}/agents/install/unity-ui-to-flexma.ps1 | iex"),
            "claudeCommand": format!("claude mcp add --scope user --transport http flexma {mcp}"),
            "codexCommand": format!("codex mcp add flexma --url {mcp}; codex mcp login flexma"),
            "claudeTokenCommand": format!(
                "claude mcp add --scope user --transport http flexma {mcp} --header \"Authorization: Bearer $env:FLEXMA_TOKEN\""
            ),
            "codexTokenCommand": format!("codex mcp add flexma --url {mcp} --bearer-token-env-var FLEXMA_TOKEN"),
            "opencodeCommand": format!("opencode mcp add flexma --url {mcp}; opencode mcp auth flexma"),
            "opencodeTokenCommand": format!("opencode mcp add flexma --url {mcp} --header 'Authorization=Bearer {{env:FLEXMA_TOKEN}}'"),
        },
    })
}

/// `opencode.json` with the Flexma server: signed in through the browser
/// (OAuth), or with an `Authorization` header (OAuth then stays off).
fn opencode_config(mcp: &str, authorization: Option<&str>) -> String {
    // Written by hand so the keys read in the usual order (type and url first).
    let auth = authorization
        .map(|a| format!(",\n      \"headers\": {{ \"Authorization\": {} }},\n      \"oauth\": false", json!(a)))
        .unwrap_or_default();
    format!(
        "{{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"mcp\": {{\n    \"flexma\": {{\n      \"type\": \"remote\",\n      \"url\": {}{auth}\n    }}\n  }}\n}}",
        json!(mcp)
    )
}

pub async fn agent_setup(State(st): State<AppState>, headers: HeaderMap, _user: AuthUser) -> Json<Value> {
    Json(setup_snippets(&st.base_url(&headers)))
}

// ---------- skill files ----------

/// An agent skill, embedded at build time from `skills/<name>`.
pub struct Skill {
    pub name: &'static str,
    /// Path inside the skill folder, and the file's text.
    pub files: &'static [(&'static str, &'static str)],
}

/// Everyone installs this one.
pub const FLEXMA_SKILL: Skill = Skill {
    name: "flexma",
    files: &[
        ("SKILL.md", include_str!("../../../skills/flexma/SKILL.md")),
        ("references/node-spec.md", include_str!("../../../skills/flexma/references/node-spec.md")),
        ("references/recipes.md", include_str!("../../../skills/flexma/references/recipes.md")),
    ],
};

/// Optional: exports a Unity project's uGUI and UI Toolkit screens into
/// Flexma. Ships the editor scripts the agent copies into the project.
pub const UNITY_SKILL: Skill = Skill {
    name: "unity-ui-to-flexma",
    files: &[
        ("SKILL.md", include_str!("../../../skills/unity-ui-to-flexma/SKILL.md")),
        ("scripts/FlexmaUIExporter.cs", include_str!("../../../skills/unity-ui-to-flexma/scripts/FlexmaUIExporter.cs")),
        ("scripts/FlexmaUIToolkitExport.cs", include_str!("../../../skills/unity-ui-to-flexma/scripts/FlexmaUIToolkitExport.cs")),
        ("scripts/FlexmaUploader.cs", include_str!("../../../skills/unity-ui-to-flexma/scripts/FlexmaUploader.cs")),
        ("scripts/FlexmaClient.cs", include_str!("../../../skills/unity-ui-to-flexma/scripts/FlexmaClient.cs")),
        ("scripts/FlexmaUIExporterWindow.cs", include_str!("../../../skills/unity-ui-to-flexma/scripts/FlexmaUIExporterWindow.cs")),
    ],
};

/// Optional: builds Flexma frames into Unity uGUI prefabs that look the same
/// as the design. Ships the editor and runtime scripts.
pub const FLEXMA_TO_UNITY_SKILL: Skill = Skill {
    name: "flexma-to-unity",
    files: &[
        ("SKILL.md", include_str!("../../../skills/flexma-to-unity/SKILL.md")),
        ("scripts/Runtime/FlexmaNode.cs", include_str!("../../../skills/flexma-to-unity/scripts/Runtime/FlexmaNode.cs")),
        ("scripts/Runtime/FlexmaLayout.cs", include_str!("../../../skills/flexma-to-unity/scripts/Runtime/FlexmaLayout.cs")),
        ("scripts/Runtime/FlexmaScreens.cs", include_str!("../../../skills/flexma-to-unity/scripts/Runtime/FlexmaScreens.cs")),
        ("scripts/Runtime/FlexmaScrollHug.cs", include_str!("../../../skills/flexma-to-unity/scripts/Runtime/FlexmaScrollHug.cs")),
        ("scripts/Editor/FlexmaImporter.cs", include_str!("../../../skills/flexma-to-unity/scripts/Editor/FlexmaImporter.cs")),
        ("scripts/Editor/FlexmaImportCheck.cs", include_str!("../../../skills/flexma-to-unity/scripts/Editor/FlexmaImportCheck.cs")),
        ("scripts/Editor/FlexmaImportWindow.cs", include_str!("../../../skills/flexma-to-unity/scripts/Editor/FlexmaImportWindow.cs")),
    ],
};

pub const SKILLS: &[&Skill] = &[&FLEXMA_SKILL, &UNITY_SKILL, &FLEXMA_TO_UNITY_SKILL];

/// Optional installers, `/agents/install/<name>.sh|.ps1`: each also installs
/// (or updates) the `flexma` skill it builds on.
const ADD_ONS: &[&Skill] = &[&UNITY_SKILL, &FLEXMA_TO_UNITY_SKILL];

pub async fn skill_file(Path((skill, path)): Path<(String, String)>) -> Response {
    let file = SKILLS.iter().find(|s| s.name == skill).and_then(|s| s.files.iter().find(|(p, _)| *p == path));
    match file {
        Some((p, body)) => {
            let kind = if p.ends_with(".md") { "text/markdown; charset=utf-8" } else { "text/plain; charset=utf-8" };
            ([(header::CONTENT_TYPE, kind)], *body).into_response()
        }
        None => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

pub async fn install_script(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let base = st.base_url(&headers);
    ([(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")], install_sh(&base, &[&FLEXMA_SKILL])).into_response()
}

/// The same installer for Windows: `irm <base>/agents/install.ps1 | iex`.
pub async fn install_script_ps1(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let base = st.base_url(&headers);
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], install_ps1(&base, &[&FLEXMA_SKILL])).into_response()
}

/// An optional skill's installer: `/agents/install/unity-ui-to-flexma.sh` (or `.ps1`).
pub async fn add_on_installer(State(st): State<AppState>, headers: HeaderMap, Path(file): Path<String>) -> Response {
    let base = st.base_url(&headers);
    let find = |name: &str| ADD_ONS.iter().copied().find(|s| s.name == name);
    if let Some(skill) = file.strip_suffix(".sh").and_then(find) {
        return ([(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")], install_sh(&base, &[&FLEXMA_SKILL, skill])).into_response();
    }
    if let Some(skill) = file.strip_suffix(".ps1").and_then(find) {
        return ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], install_ps1(&base, &[&FLEXMA_SKILL, skill])).into_response();
    }
    (axum::http::StatusCode::NOT_FOUND, "not found").into_response()
}

fn names(skills: &[&Skill]) -> String {
    let n: Vec<String> = skills.iter().map(|s| format!("\"{}\"", s.name)).collect();
    match n.len() {
        1 => format!("the {} skill", n[0]),
        _ => format!("the {} skills", n.join(" and ")),
    }
}

fn install_sh(base: &str, skills: &[&Skill]) -> String {
    let snippets = setup_snippets(base);
    let mut fetches = String::new();
    for s in skills {
        for (p, _) in s.files {
            fetches.push_str(&format!("  fetch {} \"{p}\"\n", s.name));
        }
    }
    let what = names(skills);
    format!(
        r#"#!/bin/sh
# Flexma skill installer for Claude Code, Codex and OpenCode.
# Installs {what} into ~/.claude/skills, ~/.codex/skills and ~/.config/opencode/skills.
set -e
BASE="{base}"
install_to() {{
  ROOT="$1"
  fetch() {{
    OUT="$ROOT/$1/$2"
    mkdir -p "$(dirname "$OUT")"
    if command -v curl >/dev/null 2>&1; then curl -fsSL "$BASE/agents/skills/$1/$2" -o "$OUT"
    else wget -q "$BASE/agents/skills/$1/$2" -O "$OUT"; fi
  }}
{fetches}  echo "  installed -> $ROOT"
}}
echo "Installing {what} from $BASE"
install_to "$HOME/.claude/skills"
install_to "$HOME/.codex/skills"
install_to "${{XDG_CONFIG_HOME:-$HOME/.config}}/opencode/skills"
echo ""
echo "Next, connect the Flexma MCP server (your agent will open a browser to sign in):"
echo ""
echo "  Claude Code:"
echo '    {claude}'
echo ""
echo "  Codex:"
echo '    {codex}'
echo ""
echo "  OpenCode:"
echo '    {opencode}'
echo ""
"#,
        claude = snippets["claudeCommand"].as_str().unwrap_or_default().replace('\'', "'\\''"),
        codex = snippets["codexCommand"].as_str().unwrap_or_default().replace('\'', "'\\''"),
        opencode = snippets["opencodeCommand"].as_str().unwrap_or_default().replace('\'', "'\\''"),
    )
}

fn install_ps1(base: &str, skills: &[&Skill]) -> String {
    let snippets = setup_snippets(base);
    let ps = |s: &str| s.replace('\'', "''");
    let files: Vec<String> = skills.iter().flat_map(|s| s.files.iter().map(move |(p, _)| format!("'{}/{p}'", s.name))).collect();
    let what = names(skills);
    format!(
        r#"# Flexma skill installer for Claude Code, Codex and OpenCode (Windows PowerShell).
# Installs {what} into ~\.claude\skills, ~\.codex\skills and ~\.config\opencode\skills.
# Runs in its own scope so nothing leaks into your session.
& {{
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$FlexmaBase = '{base}'
$FlexmaFiles = @({files})
function Install-FlexmaSkills([string]$Root) {{
  foreach ($File in $FlexmaFiles) {{
    $Out = Join-Path $Root ($File -replace '/', '\')
    New-Item -ItemType Directory -Force -Path (Split-Path $Out) | Out-Null
    Invoke-WebRequest -UseBasicParsing -Uri "$FlexmaBase/agents/skills/$File" -OutFile $Out
  }}
  Write-Host "  installed -> $Root"
}}
Write-Host "Installing {what} from $FlexmaBase"
Install-FlexmaSkills (Join-Path $HOME '.claude\skills')
Install-FlexmaSkills (Join-Path $HOME '.codex\skills')
Install-FlexmaSkills (Join-Path $HOME '.config\opencode\skills')
Write-Host ''
Write-Host 'Next, connect the Flexma MCP server (your agent will open a browser to sign in):'
Write-Host ''
Write-Host '  Claude Code:'
Write-Host '    {claude}'
Write-Host ''
Write-Host '  Codex:'
Write-Host '    {codex}'
Write-Host ''
Write-Host '  OpenCode:'
Write-Host '    {opencode}'
Write-Host ''
}}
"#,
        what = ps(&what),
        base = ps(base),
        files = files.join(", "),
        claude = ps(snippets["windows"]["claudeCommand"].as_str().unwrap_or_default()),
        codex = ps(snippets["windows"]["codexCommand"].as_str().unwrap_or_default()),
        opencode = ps(snippets["windows"]["opencodeCommand"].as_str().unwrap_or_default()),
    )
}

#[cfg(test)]
mod skill_tests {
    use super::*;

    #[test]
    fn installers_fetch_every_file() {
        let sh = install_sh("https://f.example", &[&FLEXMA_SKILL, &UNITY_SKILL]);
        for s in [&FLEXMA_SKILL, &UNITY_SKILL] {
            for (p, body) in s.files {
                assert!(sh.contains(&format!("fetch {} \"{p}\"", s.name)), "{p}");
                assert!(!body.is_empty());
            }
        }
        let ps = install_ps1("https://f.example", &[&FLEXMA_SKILL, &UNITY_SKILL]);
        assert!(ps.contains("'unity-ui-to-flexma/scripts/FlexmaClient.cs'"));
        assert!(ps.contains("'unity-ui-to-flexma/scripts/FlexmaUIToolkitExport.cs'"));
        // The default installer stays flexma-only.
        assert!(!install_sh("https://f.example", &[&FLEXMA_SKILL]).contains("unity-ui-to-flexma"));
    }

    #[test]
    fn opencode_is_set_up_like_the_others() {
        let sh = install_sh("https://f.example", &[&FLEXMA_SKILL]);
        assert!(sh.contains("install_to \"${XDG_CONFIG_HOME:-$HOME/.config}/opencode/skills\""));
        assert!(sh.contains("opencode mcp add flexma --url https://f.example/mcp && opencode mcp auth flexma"));
        assert!(install_ps1("https://f.example", &[&FLEXMA_SKILL]).contains("Join-Path $HOME '.config\\opencode\\skills'"));
        let s = setup_snippets("https://f.example");
        let oauth: Value = serde_json::from_str(s["opencodeConfig"].as_str().unwrap()).unwrap();
        assert_eq!(oauth["mcp"]["flexma"], json!({ "type": "remote", "url": "https://f.example/mcp" }));
        let token: Value = serde_json::from_str(s["opencodeTokenConfig"].as_str().unwrap()).unwrap();
        assert_eq!(token["mcp"]["flexma"]["headers"]["Authorization"], "Bearer {env:FLEXMA_TOKEN}");
        assert_eq!(token["mcp"]["flexma"]["oauth"], false);
        assert!(s["opencodeTokenCommand"].as_str().unwrap().ends_with("--header 'Authorization=Bearer {env:FLEXMA_TOKEN}'"));
    }

    #[test]
    fn skill_frontmatter_names_match() {
        for s in SKILLS {
            let md = s.files.iter().find(|(p, _)| *p == "SKILL.md").expect("SKILL.md").1;
            assert!(md.contains(&format!("\nname: {}\n", s.name)), "{} frontmatter", s.name);
        }
    }
}
