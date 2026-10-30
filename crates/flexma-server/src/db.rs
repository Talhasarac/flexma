//! SQLite storage. One connection in WAL mode behind a mutex; queries run on the
//! blocking thread pool so they never stall the async runtime.

use crate::error::{AppError, AppResult};
use rand::Rng;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r#"
    CREATE TABLE users (
        id TEXT PRIMARY KEY,
        email TEXT NOT NULL UNIQUE COLLATE NOCASE,
        name TEXT NOT NULL,
        password_hash TEXT NOT NULL,
        is_admin INTEGER NOT NULL DEFAULT 0,
        color TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE sessions (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL
    );
    CREATE TABLE api_tokens (
        id TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        prefix TEXT NOT NULL,
        token_hash TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        last_used_at INTEGER
    );
    CREATE TABLE workspaces (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        created_by TEXT,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE workspace_members (
        workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
        user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        role TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (workspace_id, user_id)
    );
    CREATE INDEX workspace_members_user ON workspace_members(user_id);
    CREATE TABLE invites (
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
        email TEXT,
        role TEXT NOT NULL,
        token TEXT NOT NULL UNIQUE,
        created_by TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL
    );
    CREATE TABLE projects (
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX projects_workspace ON projects(workspace_id);
    CREATE TABLE files (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        document TEXT NOT NULL,
        version INTEGER NOT NULL DEFAULT 0,
        thumbnail TEXT,
        created_by TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX files_project ON files(project_id);
    CREATE TABLE assets (
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
        file_id TEXT,
        mime TEXT NOT NULL,
        width INTEGER,
        height INTEGER,
        size INTEGER NOT NULL,
        created_by TEXT,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX assets_workspace ON assets(workspace_id);
    "#,
    // 2: OAuth 2.1 for remote MCP clients (Claude Code, Codex, ...)
    r#"
    ALTER TABLE api_tokens ADD COLUMN expires_at INTEGER;
    ALTER TABLE api_tokens ADD COLUMN oauth_client_id TEXT;
    CREATE TABLE oauth_clients (
        client_id TEXT PRIMARY KEY,
        client_name TEXT NOT NULL,
        redirect_uris TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE oauth_codes (
        code_hash TEXT PRIMARY KEY,
        client_id TEXT NOT NULL,
        user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        redirect_uri TEXT NOT NULL,
        code_challenge TEXT NOT NULL,
        scope TEXT,
        expires_at INTEGER NOT NULL
    );
    CREATE TABLE oauth_refresh_tokens (
        token_hash TEXT PRIMARY KEY,
        client_id TEXT NOT NULL,
        user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        api_token_id TEXT,
        expires_at INTEGER NOT NULL
    );
    "#,
    // 3: version history
    r#"
    CREATE TABLE file_versions (
        id TEXT PRIMARY KEY,
        file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        label TEXT,
        document TEXT NOT NULL,
        version INTEGER NOT NULL,
        created_by TEXT,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX file_versions_file ON file_versions(file_id, created_at);
    "#,
    // 4: comments
    r#"
    CREATE TABLE comments (
        id TEXT PRIMARY KEY,
        file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        parent_id TEXT REFERENCES comments(id) ON DELETE CASCADE,
        page_id TEXT,
        node_id TEXT,
        x REAL NOT NULL DEFAULT 0,
        y REAL NOT NULL DEFAULT 0,
        author_id TEXT,
        author_name TEXT NOT NULL,
        author_color TEXT NOT NULL,
        is_agent INTEGER NOT NULL DEFAULT 0,
        body TEXT NOT NULL,
        resolved INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX comments_file ON comments(file_id, created_at);
    "#,
    // 5: instance settings (admin-editable, override env defaults)
    r#"
    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    );
    "#,
    // 6: public view-only share links (one per file)
    r#"
    CREATE TABLE file_shares (
        file_id TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
        token TEXT NOT NULL UNIQUE,
        created_by TEXT,
        created_at INTEGER NOT NULL,
        expires_at INTEGER
    );
    "#,
    // 7: workspace fonts (uploaded files and Google Fonts), and PNG
    // thumbnails rendered on demand with those fonts.
    r#"
    CREATE TABLE fonts (
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
        family TEXT NOT NULL,
        weight INTEGER NOT NULL,
        weight_max INTEGER NOT NULL,
        italic INTEGER NOT NULL,
        source TEXT NOT NULL,
        format TEXT NOT NULL,
        size INTEGER NOT NULL,
        created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
        created_at INTEGER NOT NULL,
        UNIQUE (workspace_id, family, weight, italic)
    );
    CREATE INDEX fonts_workspace ON fonts(workspace_id);
    ALTER TABLE files ADD COLUMN thumbnail_png BLOB;
    ALTER TABLE files ADD COLUMN thumbnail_version INTEGER;
    UPDATE files SET thumbnail = NULL;
    "#,
    // Image content hashes, so uploading the same picture again reuses it.
    r#"
    ALTER TABLE assets ADD COLUMN sha256 TEXT;
    CREATE INDEX assets_sha256 ON assets(workspace_id, sha256);
    "#,
    // 9: paint layers: the brush-stroke log (in server order) and the tile
    // snapshots (PNGs on disk under data/paint) that strokes replay onto.
    r#"
    CREATE TABLE paint_strokes (
        file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        seq INTEGER NOT NULL,
        id TEXT NOT NULL,
        layer_id TEXT NOT NULL,
        user_id TEXT,
        data TEXT NOT NULL,
        x0 REAL NOT NULL,
        y0 REAL NOT NULL,
        x1 REAL NOT NULL,
        y1 REAL NOT NULL,
        undone INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (file_id, seq)
    );
    CREATE UNIQUE INDEX paint_strokes_id ON paint_strokes(file_id, id);
    CREATE TABLE paint_tiles (
        file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        layer_id TEXT NOT NULL,
        tx INTEGER NOT NULL,
        ty INTEGER NOT NULL,
        base_seq INTEGER,
        view_seq INTEGER,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (file_id, layer_id, tx, ty)
    );
    "#,
    // 10: files are 'design' or 'painting' (shows the brush and eraser).
    r#"
    ALTER TABLE files ADD COLUMN kind TEXT NOT NULL DEFAULT 'design';
    "#,
    // 11: documents stored in parts (see flexma_doc::parts and doc_store), so
    // a save rewrites only the parts that changed. They are the document
    // while parts_version equals version; otherwise files.document is.
    r#"
    ALTER TABLE files ADD COLUMN parts_version INTEGER;
    CREATE TABLE file_parts (
        file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        part TEXT NOT NULL,
        document TEXT NOT NULL,
        PRIMARY KEY (file_id, part)
    );
    "#,
    // 12: when each agent skill version first ran here, and when each person
    // last saw the skill updates (the AI button's dot).
    r#"
    CREATE TABLE skill_versions (
        skill TEXT NOT NULL,
        version TEXT NOT NULL,
        first_seen INTEGER NOT NULL,
        PRIMARY KEY (skill, version)
    );
    ALTER TABLE users ADD COLUMN skills_seen_at INTEGER;
    "#,
];

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let db = Db { conn: Arc::new(Mutex::new(conn)) };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let v = i as i64 + 1;
            if v > version {
                let tx = conn.transaction()?;
                tx.execute_batch(sql)?;
                tx.pragma_update(None, "user_version", v)?;
                tx.commit()?;
                tracing::info!("applied database migration {v}");
            }
        }
        Ok(())
    }

    /// Run a closure with the connection on the blocking pool.
    pub async fn call<F, R>(&self, f: F) -> AppResult<R>
    where
        F: FnOnce(&mut Connection) -> AppResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut guard)
        })
        .await
        .map_err(AppError::internal)?
    }
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn ts(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Random base-62 string.
pub fn random_string(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

/// Short random id for rows.
pub fn new_id() -> String {
    random_string(12)
}

pub fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// A pleasant random color for avatars / cursors.
pub fn random_color() -> String {
    const COLORS: &[&str] = &[
        "#6d5dfc", "#f24e1e", "#0fa958", "#1abcfe", "#ff7262", "#a259ff", "#ffb800", "#00b5a5", "#e9488d", "#3d7cf5",
    ];
    COLORS[rand::thread_rng().gen_range(0..COLORS.len())].to_string()
}
