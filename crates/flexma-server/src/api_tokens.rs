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
        ("scripts/Runtime/FlexmaGradient.cs", include_str!("../../../skills/flexma-to-unity/scripts/Runtime/FlexmaGradient.cs")),
        ("scripts/Editor/FlexmaImporter.cs", include_str!("../../../skills/flexma-to-unity/scripts/Editor/FlexmaImporter.cs")),
        ("scripts/Editor/FlexmaImportCheck.cs", include_str!("../../../skills/flexma-to-unity/scripts/Editor/FlexmaImportCheck.cs")),
        ("scripts/Editor/FlexmaImportWindow.cs", include_str!("../../../skills/flexma-to-unity/scripts/Editor/FlexmaImportWindow.cs")),
    ],
};

pub const SKILLS: &[&Skill] = &[&FLEXMA_SKILL, &UNITY_SKILL, &FLEXMA_TO_UNITY_SKILL];

/// Optional installers, `/agents/install/<name>.sh|.ps1`: each also installs
/// (or updates) the `flexma` skill it builds on.
const ADD_ONS: &[&Skill] = &[&UNITY_SKILL, &FLEXMA_TO_UNITY_SKILL];

impl Skill {
    /// A hash of every file, so any edit is a new version. Installed copies
    /// name it in their SKILL.md (`stamped`), and the MCP server names the
    /// latest, so agents can tell theirs is out of date.
    pub fn version(&self) -> &'static str {
        static VERSIONS: std::sync::OnceLock<Vec<(&'static str, String)>> = std::sync::OnceLock::new();
        let all = VERSIONS.get_or_init(|| {
            SKILLS
                .iter()
                .map(|s| {
                    let text: String = s.files.iter().map(|(p, body)| format!("{p}\0{body}\0")).collect();
                    (s.name, sha256_hex(&text)[..8].to_string())
                })
                .collect()
        });
        all.iter().find(|(n, _)| *n == self.name).map(|(_, v)| v.as_str()).unwrap_or_default()
    }

    /// SKILL.md as installed: with its version and how to update it right
    /// after the frontmatter (which has to stay first).
    fn stamped(&self, md: &str, base: &str) -> String {
        let end = md.strip_prefix("---\n").and_then(|rest| rest.find("\n---\n")).map_or(0, |i| i + 9);
        let (front, body) = md.split_at(end);
        format!(
            "{front}\n> Skill version `{v}`, from {base}. The Flexma MCP server names the latest version of each skill; if it names \
             another one for `{name}`, this copy is out of date: tell the user, and offer to update it with \
             `curl -fsSL {base}/agents/install.sh | sh` (Windows PowerShell: `irm {base}/agents/install.ps1 | iex`).\n{body}",
            v = self.version(),
            name = self.name,
        )
    }
}

/// When each skill's current version first ran on this server, from
/// `skill_versions` (filled in by `record_skill_versions` at startup).
static UPDATED: std::sync::OnceLock<Vec<(&'static str, i64)>> = std::sync::OnceLock::new();

/// Note the skills' versions at startup, so a new one counts as an update
/// from now on.
pub fn record_skill_versions(c: &rusqlite::Connection) -> rusqlite::Result<()> {
    let mut out = Vec::new();
    for s in SKILLS {
        c.execute(
            "INSERT OR IGNORE INTO skill_versions (skill, version, first_seen) VALUES (?1, ?2, ?3)",
            params![s.name, s.version(), now()],
        )?;
        let at: i64 = c.query_row(
            "SELECT first_seen FROM skill_versions WHERE skill = ?1 AND version = ?2",
            params![s.name, s.version()],
            |r| r.get(0),
        )?;
        out.push((s.name, at));
    }
    let _ = UPDATED.set(out);
    Ok(())
}

fn updated_at(skill: &Skill) -> i64 {
    UPDATED.get().and_then(|u| u.iter().find(|(n, _)| *n == skill.name)).map_or(0, |(_, at)| *at)
}

/// For agents (MCP instructions and tool results): the latest versions,
/// and what to do when theirs differ.
pub fn skill_versions_note(base: &str) -> String {
    let versions: Vec<String> = SKILLS.iter().map(|s| format!("{} {}", s.name, s.version())).collect();
    format!(
        "Latest Flexma skill versions: {}. An installed skill names its version near the top of its SKILL.md. If a Flexma skill \
         you loaded names another version, or none, it is out of date: tell the user once and offer to update it with \
         `curl -fsSL {base}/agents/install.sh | sh` (Windows: `irm {base}/agents/install.ps1 | iex`), which updates every \
         Flexma skill installed.",
        versions.join(", "),
    )
}

/// `GET /api/skills`: each skill's version, when it last changed, and
/// whether that's news to this person (since they last opened the AI dialog).
pub async fn skills(State(st): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let uid = user.id.clone();
    let seen: i64 = st
        .db
        .call(move |c| Ok(c.query_row("SELECT COALESCE(skills_seen_at, created_at) FROM users WHERE id = ?1", params![uid], |r| r.get(0))?))
        .await?;
    let skills: Vec<Value> = SKILLS
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "version": s.version(),
                "updatedAt": ts(updated_at(s)),
                "updated": updated_at(s) > seen,
            })
        })
        .collect();
    Ok(Json(json!({ "skills": skills })))
}

/// `POST /api/skills/seen`: the person has seen the updates; clears the dot.
pub async fn skills_seen(State(st): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let uid = user.id.clone();
    st.db
        .call(move |c| Ok(c.execute("UPDATE users SET skills_seen_at = ?1 WHERE id = ?2", params![now(), uid])?))
        .await?;
    skills(State(st), user).await
}

pub async fn skill_file(State(st): State<AppState>, headers: HeaderMap, Path((skill, path)): Path<(String, String)>) -> Response {
    let Some(s) = SKILLS.iter().find(|s| s.name == skill) else {
        return (axum::http::StatusCode::NOT_FOUND, "not found").into_response();
    };
    match s.files.iter().find(|(p, _)| *p == path) {
        Some((p, body)) if *p == "SKILL.md" => {
            ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], s.stamped(body, &st.base_url(&headers))).into_response()
        }
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

fn add_ons_besides(skills: &[&Skill]) -> Vec<&'static Skill> {
    ADD_ONS.iter().copied().filter(|a| !skills.iter().any(|s| s.name == a.name)).collect()
}

fn names(skills: &[&Skill]) -> String {
    // No quotes: these go inside the installers' double-quoted messages.
    match skills {
        [s] => format!("the {} skill ({})", s.name, s.version()),
        _ => {
            let n: Vec<String> = skills.iter().map(|s| format!("{} ({})", s.name, s.version())).collect();
            format!("the {} skills", n.join(" and "))
        }
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
    // Add-ons already installed are updated too, so one command keeps every
    // Flexma skill current.
    let mut updates = String::new();
    for a in add_ons_besides(skills) {
        updates.push_str(&format!("  if [ -f \"$ROOT/{}/SKILL.md\" ]; then\n", a.name));
        for (p, _) in a.files {
            updates.push_str(&format!("    fetch {} \"{p}\"\n", a.name));
        }
        updates.push_str(&format!("    echo \"  updated {} ({}) -> $ROOT\"\n  fi\n", a.name, a.version()));
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
{updates}}}
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
    let list = |skills: &[&Skill]| -> String {
        let files: Vec<String> = skills.iter().flat_map(|s| s.files.iter().map(move |(p, _)| format!("'{}/{p}'", s.name))).collect();
        files.join(", ")
    };
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
# Add-ons already installed are updated too.
$FlexmaAddOns = @({add_ons})
function Install-FlexmaSkills([string]$Root) {{
  $AddOns = @($FlexmaAddOns | Where-Object {{ Test-Path (Join-Path $Root (($_ -split '/')[0] + '\SKILL.md')) }})
  foreach ($File in ($FlexmaFiles + $AddOns)) {{
    $Out = Join-Path $Root ($File -replace '/', '\')
    New-Item -ItemType Directory -Force -Path (Split-Path $Out) | Out-Null
    Invoke-WebRequest -UseBasicParsing -Uri "$FlexmaBase/agents/skills/$File" -OutFile $Out
  }}
  Write-Host "  installed -> $Root"
  $AddOns | ForEach-Object {{ ($_ -split '/')[0] }} | Select-Object -Unique | ForEach-Object {{ Write-Host "  updated $_ -> $Root" }}
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
        files = list(skills),
        add_ons = list(&add_ons_besides(skills)),
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
        // The default installer installs only flexma (add-ons only where they already are).
        assert!(!install_sh("https://f.example", &[&FLEXMA_SKILL]).contains("\n  fetch unity-ui-to-flexma"));
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

    #[test]
    fn installed_skills_name_their_version_after_the_frontmatter() {
        for s in SKILLS {
            assert_eq!(s.version().len(), 8);
            let md = s.files.iter().find(|(p, _)| *p == "SKILL.md").unwrap().1;
            let out = s.stamped(md, "https://f.example");
            // Unchanged frontmatter first, then the version, then the skill.
            let end = md.find("\n---\n").unwrap() + 5;
            assert_eq!(&out[..end], &md[..end]);
            let note = format!("\n> Skill version `{}`, from https://f.example.", s.version());
            assert!(out[end..].starts_with(&note), "{}", &out[end..end + 200]);
            assert!(out.ends_with(&md[end..]));
            assert!(out.contains("`curl -fsSL https://f.example/agents/install.sh | sh`"));
        }
        assert_ne!(FLEXMA_SKILL.version(), UNITY_SKILL.version());
    }

    #[test]
    fn installers_update_add_ons_already_installed() {
        let sh = install_sh("https://f.example", &[&FLEXMA_SKILL, &UNITY_SKILL]);
        assert!(sh.contains("  if [ -f \"$ROOT/flexma-to-unity/SKILL.md\" ]; then\n    fetch flexma-to-unity \"SKILL.md\"\n"));
        assert!(!sh.contains("$ROOT/unity-ui-to-flexma/SKILL.md\" ]"));
        let ps = install_ps1("https://f.example", &[&FLEXMA_SKILL]);
        assert!(ps.contains("$FlexmaAddOns = @('unity-ui-to-flexma/SKILL.md', "));
        assert!(ps.contains("'flexma-to-unity/scripts/Editor/FlexmaImportWindow.cs')"));
        // The scripts still parse with versions in their messages.
        for sh in [install_sh("https://f.example", &[&FLEXMA_SKILL]), sh] {
            let dir = std::env::temp_dir().join(format!("flexma-install-{}.sh", new_id()));
            std::fs::write(&dir, &sh).unwrap();
            let ok = std::process::Command::new("sh").arg("-n").arg(&dir).status().unwrap().success();
            let _ = std::fs::remove_file(&dir);
            assert!(ok, "{sh}");
        }
    }
}
