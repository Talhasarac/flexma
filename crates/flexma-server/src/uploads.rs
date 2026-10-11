//! Short-lived upload links for agents. MCP tool calls can't carry file bytes
//! (a font or photo would flood the agent's context), so `upload_font`,
//! `upload_image` and `upload_nodes` hand out a link the agent posts files to
//! with curl.
//!
//! A link is tied to a workspace (and a file, for images) and to the person
//! who authorized the agent. It accepts a set number of files within 15
//! minutes and lives in memory only, so a restart cancels it.

use crate::db::random_string;
use crate::error::{AppError, AppResult};
use axum::http::StatusCode;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(15 * 60);
/// Most files one link accepts (bulk exports upload hundreds of images).
pub const MAX_FILES: u32 = 100;
const TOKEN_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Font,
    Image { file_id: String },
    /// A JSON node tree for `create_nodes`, created as the agent that asked
    /// (its API token name, so undo and presence match its other edits).
    Nodes { file_id: String, parent_id: Option<String>, index: Option<usize>, token_name: Option<String> },
}

impl Kind {
    fn tool(&self) -> &'static str {
        match self {
            Kind::Font => "upload_font",
            Kind::Image { .. } => "upload_image",
            Kind::Nodes { .. } => "upload_nodes",
        }
    }
}

/// What one upload through a link is allowed to do.
#[derive(Debug)]
pub struct Grant {
    pub kind: Kind,
    pub workspace_id: String,
    pub user_id: String,
    /// Uploads left on the link after this one.
    pub remaining: u32,
}

struct Link {
    kind: Kind,
    workspace_id: String,
    user_id: String,
    remaining: u32,
    expires: Instant,
}

fn links() -> &'static Mutex<HashMap<String, Link>> {
    static LINKS: OnceLock<Mutex<HashMap<String, Link>>> = OnceLock::new();
    LINKS.get_or_init(Default::default)
}

/// A new link for `files` uploads (clamped to 1..=MAX_FILES).
pub fn create(kind: Kind, workspace_id: &str, user_id: &str, files: u32) -> String {
    let token = random_string(TOKEN_LEN);
    let now = Instant::now();
    let mut links = links().lock().unwrap();
    links.retain(|_, l| l.expires > now && l.remaining > 0);
    links.insert(
        token.clone(),
        Link {
            kind,
            workspace_id: workspace_id.to_string(),
            user_id: user_id.to_string(),
            remaining: files.clamp(1, MAX_FILES),
            expires: now + TTL,
        },
    );
    token
}

/// Use up one upload of a live link whose kind passes `expect`. Validate the
/// file before calling this, so a broken file doesn't cost an upload.
pub fn take(token: &str, expect: impl Fn(&Kind) -> bool, tool: &str) -> AppResult<Grant> {
    let dead = || AppError::new(StatusCode::NOT_FOUND, format!("this upload link has expired or been used up; get a new one with {tool}"));
    if token.len() != TOKEN_LEN || !token.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(dead());
    }
    let mut links = links().lock().unwrap();
    let link = links
        .get_mut(token)
        .filter(|l| l.expires > Instant::now() && l.remaining > 0 && expect(&l.kind))
        .ok_or_else(dead)?;
    link.remaining -= 1;
    let grant = Grant {
        kind: link.kind.clone(),
        workspace_id: link.workspace_id.clone(),
        user_id: link.user_id.clone(),
        remaining: link.remaining,
    };
    if link.remaining == 0 {
        links.remove(token);
    }
    debug_assert_eq!(grant.kind.tool(), tool);
    Ok(grant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_count_down_and_expire_when_used_up() {
        let t = create(Kind::Image { file_id: "f".into() }, "ws", "u", 2);
        let is_image = |k: &Kind| matches!(k, Kind::Image { .. });
        // The wrong kind of upload doesn't use the link.
        assert!(take(&t, |k| *k == Kind::Font, "upload_font").is_err());
        assert_eq!(take(&t, is_image, "upload_image").unwrap().remaining, 1);
        let last = take(&t, is_image, "upload_image").unwrap();
        assert_eq!((last.remaining, last.workspace_id.as_str()), (0, "ws"));
        assert!(take(&t, is_image, "upload_image").is_err());
    }

    #[test]
    fn links_are_capped() {
        let t = create(Kind::Font, "ws", "u", 999);
        let links = links().lock().unwrap();
        assert_eq!(links.get(&t).unwrap().remaining, MAX_FILES);
        assert_eq!(t.len(), TOKEN_LEN);
    }
}
