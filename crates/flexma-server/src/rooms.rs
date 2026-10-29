//! Real-time document rooms. Each open file is owned by one actor task that holds
//! the authoritative in-memory `Document`, orders incoming ops, rebroadcasts them
//! (the echo doubles as the sender's ack), tracks presence and periodically
//! snapshots to SQLite. No locks around documents; everything goes through the
//! actor's mailbox. Agents (MCP) edit through the same pipeline.

use crate::auth::{file_access, Role};
use crate::db::{now, random_string, Db};
use crate::error::{AppError, AppResult};
use crate::render::{FontDb, TextMeasurer};
use flexma_doc::{components, layout};
use flexma_doc::{Document, NodeId, Op};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// What a socket's writer sends: a JSON text frame, or a file's document as
/// one zlib-compressed binary frame (compressed by the writer, not the room).
#[derive(Clone, Debug)]
pub enum Msg {
    Text(Arc<str>),
    Doc(Arc<str>),
}

impl std::ops::Deref for Msg {
    type Target = str;

    /// The frame's JSON (for `Doc`, the document before compression).
    fn deref(&self) -> &str {
        match self {
            Msg::Text(t) | Msg::Doc(t) => t,
        }
    }
}

impl std::fmt::Display for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

/// Bounded per-client outbound queue. A client that falls this far behind is
/// disconnected and will resync from a fresh snapshot on reconnect.
pub const CLIENT_QUEUE: usize = 2048;
/// Files with more pages than this load whole (each page is a frame in the queue).
const MAX_STREAMED_PAGES: usize = 256;
const SAVE_EVERY: Duration = Duration::from_secs(2);
const IDLE_CLOSE_SECS: i64 = 90;
const AGENT_PRESENCE_TTL: Duration = Duration::from_secs(12);
const AUTO_VERSION_SECS: i64 = 15 * 60;
/// How often a room re-checks its sockets' access even when nothing says it
/// changed (expired share links and sessions, a password reset from the CLI).
const RECHECK_EVERY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub client_id: String,
    pub user_id: String,
    pub name: String,
    pub color: String,
    pub cursor: Option<[f64; 2]>,
    pub selection: Vec<String>,
    pub page: Option<String>,
    /// The world area on the person's screen, `[x, y, w, h]`, so others can
    /// follow their view.
    pub viewport: Option<[f64; 4]>,
    /// The user whose view this person follows.
    pub following: Option<String>,
    pub is_agent: bool,
    /// Anonymous viewer via a public share link.
    pub is_guest: bool,
}

/// What a client reports about itself (see `Peer`).
pub struct PresenceUpdate {
    pub cursor: Option<[f64; 2]>,
    pub selection: Vec<String>,
    pub page: Option<String>,
    pub viewport: Option<[f64; 4]>,
    pub following: Option<String>,
}

pub struct JoinInfo {
    pub user_id: String,
    pub name: String,
    pub color: String,
    pub role: Role,
    pub credential: Credential,
    /// The client takes the document as its own zlib frame after `init`
    /// (`?doc=zlib`); older clients get it inside `init`.
    pub doc_frame: bool,
    /// With `doc_frame`: the client takes the document page by page
    /// (`&pages=stream`), this page first (`&page=`, else the first page).
    pub stream_pages: Option<Option<String>>,
}

/// What a socket signed in with. The room re-checks it when access changes,
/// so revoking it (or the person's access to the file) ends the socket too.
#[derive(Clone, Debug, PartialEq)]
pub enum Credential {
    /// Hash of the session cookie.
    Session(String),
    /// Hash of the API token.
    Token(String),
    /// A public share link (anonymous guests).
    Share(String),
}

/// Re-checking a socket's access against the database.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    Keep,
    /// Still allowed in, with another role: close it so it joins again.
    Rejoin,
    /// No longer allowed in: tell it why and close it for good.
    Revoke(&'static str),
}

const SIGNED_OUT: &str = "You were signed out. Sign in again to keep working on this file.";
const NO_ACCESS: &str = "You no longer have access to this file.";
const LINK_GONE: &str = "This share link is no longer active.";

/// Whether `user_id`, signed in with `credential`, may still be in `file_id`'s
/// room with `role`. Database errors keep the socket (the next check retries).
pub fn verdict(c: &rusqlite::Connection, file_id: &str, user_id: &str, role: Role, credential: &Credential) -> Verdict {
    let denied = |e: &AppError| matches!(e.status, axum::http::StatusCode::NOT_FOUND | axum::http::StatusCode::FORBIDDEN);
    let signed_in = match credential {
        Credential::Share(token) => {
            return match crate::api_share::resolve(c, token) {
                Ok((f, _)) if f == file_id => Verdict::Keep,
                Ok(_) => Verdict::Revoke(LINK_GONE),
                Err(e) if denied(&e) => Verdict::Revoke(LINK_GONE),
                Err(_) => Verdict::Keep,
            };
        }
        Credential::Session(hash) => c
            .query_row(
                "SELECT 1 FROM sessions WHERE token_hash = ?1 AND user_id = ?2 AND expires_at > ?3",
                params![hash, user_id, now()],
                |_| Ok(()),
            )
            .optional(),
        Credential::Token(hash) => c
            .query_row(
                "SELECT 1 FROM api_tokens WHERE token_hash = ?1 AND user_id = ?2 AND (expires_at IS NULL OR expires_at > ?3)",
                params![hash, user_id, now()],
                |_| Ok(()),
            )
            .optional(),
    };
    match signed_in {
        Ok(Some(())) => {}
        Ok(None) => return Verdict::Revoke(SIGNED_OUT),
        Err(_) => return Verdict::Keep,
    }
    match file_access(c, user_id, file_id, Role::Viewer) {
        Ok((_, _, now_role)) if now_role == role => Verdict::Keep,
        Ok(_) => Verdict::Rejoin,
        Err(e) if denied(&e) => Verdict::Revoke(NO_ACCESS),
        Err(_) => Verdict::Keep,
    }
}

#[derive(Clone)]
pub struct AgentInfo {
    /// Stable key for this agent's presence (e.g. token id).
    pub key: String,
    pub user_id: String,
    pub name: String,
    pub color: String,
}

/// One step to reverse an agent's change. Undo only reverts what nobody has
/// touched since, so people's later edits win.
enum UndoStep {
    /// Set `key` back to `old` if it still has the value the agent left.
    Prop { id: NodeId, key: String, old: Value, new: Value },
    /// Remove a node the agent created (if it still exists).
    Remove { id: NodeId },
    /// Re-create nodes the agent deleted, parents first.
    Restore { nodes: Vec<flexma_doc::Node> },
}

/// One agent tool call's worth of changes.
struct UndoBatch {
    summary: String,
    steps: Vec<UndoStep>,
}

/// Agent changes kept for undo, per agent per file (memory only).
const UNDO_KEEP: usize = 30;

/// A transaction handed to agent mutations: apply ops against the live doc.
pub struct Tx<'a> {
    pub doc: &'a mut Document,
    applied: Vec<Op>,
    changed: Vec<NodeId>,
    removed: Vec<NodeId>,
    touched: Vec<NodeId>,
    undo: Vec<UndoStep>,
    /// The workspace's fonts, to measure text boxes that are still estimates.
    measurer: Option<TextMeasurer>,
}

impl<'a> Tx<'a> {
    fn new(doc: &'a mut Document) -> Self {
        Tx { doc, applied: vec![], changed: vec![], removed: vec![], touched: vec![], undo: vec![], measurer: None }
    }

    /// Remember how to reverse `op` (call before applying it).
    fn record_undo(&mut self, op: &Op) {
        match op {
            Op::Create { node } => self.undo.push(UndoStep::Remove { id: node.id.clone() }),
            Op::Delete { id } => {
                let mut nodes = Vec::new();
                self.doc.walk(id, &mut |n| nodes.push(n.clone()));
                if !nodes.is_empty() {
                    self.undo.push(UndoStep::Restore { nodes });
                }
            }
            Op::Update { id, props } => {
                let Some(node) = self.doc.get(id) else { return };
                for key in props.keys() {
                    // Several edits to one prop in a batch undo to the first value.
                    let seen = self.undo.iter().any(|s| matches!(s, UndoStep::Prop { id: i, key: k, .. } if i == id && k == key));
                    if !seen {
                        let old = node.get_prop(key).unwrap_or(Value::Null);
                        self.undo.push(UndoStep::Prop { id: id.clone(), key: key.clone(), old, new: Value::Null });
                    }
                }
            }
        }
    }

    /// After layout: note the values the agent's changes ended up with.
    fn settle_undo(&mut self) {
        for step in &mut self.undo {
            if let UndoStep::Prop { id, key, new, .. } = step {
                *new = self.doc.get(id).and_then(|n| n.get_prop(key)).unwrap_or(Value::Null);
            }
        }
    }

    fn apply_routed(&mut self, op: Op) -> Result<(), String> {
        self.record_undo(&op);
        let eff = self.doc.apply(&op)?;
        match &op {
            Op::Delete { .. } => {}
            other => self.touched.push(other.target().to_string()),
        }
        self.changed.extend(eff.changed);
        self.removed.extend(eff.removed);
        self.applied.push(op);
        Ok(())
    }

    /// Apply one op (edits aimed inside instances become instance overrides).
    pub fn apply(&mut self, op: Op) -> Result<(), String> {
        for op in components::route_ops(self.doc, flexma_doc::styles::route_ops(self.doc, vec![op])) {
            self.apply_routed(op)?;
        }
        Ok(())
    }

    /// Apply many ops; returns per-op errors (index, message) without stopping.
    pub fn apply_all(&mut self, ops: Vec<Op>) -> Vec<(usize, String)> {
        let mut errs = Vec::new();
        for (i, op) in components::route_ops(self.doc, flexma_doc::styles::route_ops(self.doc, ops)).into_iter().enumerate() {
            if let Err(e) = self.apply_routed(op) {
                errs.push((i, e));
            }
        }
        errs
    }

    /// Re-expand affected instances, measure text and run auto layout (so
    /// results can report final geometry).
    pub fn relayout(&mut self) {
        self.sync_and_layout();
        self.measure_text();
    }

    fn sync_and_layout(&mut self) {
        let synced = components::sync_instances(self.doc, &self.changed, &self.removed);
        let mut changed = std::mem::take(&mut self.changed);
        changed.extend(synced);
        let styled = flexma_doc::styles::sync_styles(self.doc, &changed);
        changed.extend(styled);
        self.removed.clear();
        // Laid-out nodes too: text that layout rewrapped needs measuring.
        let laid = layout::relayout(self.doc, &changed);
        changed.extend(laid);
        self.changed = changed;
    }

    /// Give text boxes that are still estimates (`needsMeasure`) their real
    /// size from the workspace's fonts, like the first browser to open the
    /// file would, then lay out again. Text whose font the server lacks keeps
    /// `needsMeasure`, so a browser still measures it.
    fn measure_text(&mut self) {
        let Some(m) = self.measurer.take() else { return };
        // A wrapped box's height follows the width layout gives it, which can
        // follow other boxes: a few rounds settle it.
        for _ in 0..3 {
            let ids: std::collections::BTreeSet<&NodeId> = self.changed.iter().collect();
            let mut ops = Vec::new();
            for id in ids {
                let Some(n) = self.doc.get(id) else { continue };
                if n.kind != flexma_doc::NodeType::Text || !n.needs_measure {
                    continue;
                }
                let (w, h) = m.measure_box(n);
                let mut props = serde_json::Map::new();
                if (w - n.width).abs() > 0.01 {
                    props.insert("width".into(), json!(w));
                }
                if (h - n.height).abs() > 0.01 {
                    props.insert("height".into(), json!(h));
                }
                if m.has_font(n) {
                    props.insert("needsMeasure".into(), json!(false));
                    if n.text_auto_resize == flexma_doc::TextAutoResize::Height {
                        props.insert("measuredWidth".into(), json!(n.width));
                    }
                }
                if !props.is_empty() {
                    ops.push(Op::Update { id: id.clone(), props });
                }
            }
            if ops.is_empty() {
                break;
            }
            self.apply_all(ops);
            self.sync_and_layout();
        }
        self.measurer = Some(m);
    }
}

type ReadFn = Box<dyn FnOnce(&Document, &[Peer]) + Send>;
type MutateFn = Box<dyn FnOnce(&mut Tx) -> Result<Value, String> + Send>;

enum Cmd {
    Join { info: JoinInfo, out: mpsc::Sender<Msg>, reply: oneshot::Sender<String> },
    Leave { client_id: String },
    ClientOps { client_id: String, batch: u64, ops: Vec<Op> },
    Presence { client_id: String, update: PresenceUpdate },
    Mutate { agent: AgentInfo, fonts: Option<Arc<FontDb>>, f: MutateFn, reply: oneshot::Sender<Result<Value, String>> },
    UndoAgent { agent: AgentInfo, steps: usize, reply: oneshot::Sender<Result<Value, String>> },
    Read { f: ReadFn },
    Flush { reply: oneshot::Sender<()> },
    /// Send a message to every connected client (e.g. "comments changed").
    Broadcast { msg: Msg },
    /// A paint-layer message from a client (`strokeLive`, `stroke`, `strokeUndo`).
    Paint { client_id: String, msg: Value },
    /// Disconnect guests who joined through a share link that was turned off.
    KickShare { token: String },
    /// Re-check every socket's access (see `verdict`).
    Recheck,
    /// Stop the actor. `discard` skips the final save (used when the stored
    /// document was replaced underneath, e.g. a version restore).
    Shutdown { notify: Option<String>, discard: bool, done: Option<oneshot::Sender<bool>> },
}

struct Stats {
    clients: AtomicUsize,
    last_active: AtomicI64,
    /// The last stroke number handed out (`Room::paint_seq`).
    paint_seq: AtomicU64,
}

#[derive(Clone)]
pub struct RoomHandle {
    tx: mpsc::Sender<Cmd>,
    stats: Arc<Stats>,
}

fn closed() -> AppError {
    AppError::new(axum::http::StatusCode::SERVICE_UNAVAILABLE, "document is reloading, please retry")
}

impl RoomHandle {
    pub async fn join(&self, info: JoinInfo, out: mpsc::Sender<Msg>) -> AppResult<String> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Cmd::Join { info, out, reply }).await.map_err(|_| closed())?;
        rx.await.map_err(|_| closed())
    }

    pub fn leave(&self, client_id: String) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Cmd::Leave { client_id }).await;
        });
    }

    pub async fn client_ops(&self, client_id: String, batch: u64, ops: Vec<Op>) {
        let _ = self.tx.send(Cmd::ClientOps { client_id, batch, ops }).await;
    }

    pub async fn presence(&self, client_id: String, update: PresenceUpdate) {
        let _ = self.tx.send(Cmd::Presence { client_id, update }).await;
    }

    /// Run a read-only closure against the live document.
    pub async fn read<T, F>(&self, f: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Document, &[Peer]) -> T + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let f: ReadFn = Box::new(move |d, p| {
            let _ = tx.send(f(d, p));
        });
        self.tx.send(Cmd::Read { f }).await.map_err(|_| closed())?;
        rx.await.map_err(|_| closed())
    }

    /// The last stroke number the room handed out. A tile snapshot can't cover
    /// strokes beyond it.
    pub fn paint_seq(&self) -> u64 {
        self.stats.paint_seq.load(Ordering::Relaxed)
    }

    /// Disconnect guests that joined through `token`.
    pub async fn kick_share(&self, token: String) {
        let _ = self.tx.send(Cmd::KickShare { token }).await;
    }

    /// Notify every connected client.
    pub async fn paint(&self, client_id: String, msg: Value) {
        let _ = self.tx.send(Cmd::Paint { client_id, msg }).await;
    }

    pub async fn broadcast(&self, msg: Value) {
        let _ = self.tx.send(Cmd::Broadcast { msg: Msg::Text(Arc::from(msg.to_string())) }).await;
    }

    /// Persist the document now.
    pub async fn flush(&self) -> AppResult<()> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Cmd::Flush { reply }).await.map_err(|_| closed())?;
        rx.await.map_err(|_| closed())
    }

    /// Run an agent mutation transactionally on the actor; ops are broadcast live.
    /// Undo the agent's last `steps` changes in this file.
    pub async fn undo_agent(&self, agent: AgentInfo, steps: usize) -> AppResult<Result<Value, String>> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Cmd::UndoAgent { agent, steps, reply }).await.map_err(|_| closed())?;
        rx.await.map_err(|_| closed())
    }

    /// Run an agent's edit. With the workspace's `fonts`, text boxes the edit
    /// leaves as estimates get measured (see `Tx::measure_text`).
    pub async fn mutate<F>(&self, agent: AgentInfo, fonts: Option<Arc<FontDb>>, f: F) -> AppResult<Result<Value, String>>
    where
        F: FnOnce(&mut Tx) -> Result<Value, String> + Send + 'static,
    {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Cmd::Mutate { agent, fonts, f: Box::new(f), reply }).await.map_err(|_| closed())?;
        rx.await.map_err(|_| closed())
    }
}

impl Rooms {
    /// Re-check the sockets in every open room. Call after changing who may
    /// open what: roles, memberships, accounts, passwords, sessions, tokens.
    pub fn recheck_all(&self) {
        for h in self.rooms.lock().unwrap().values() {
            // A full queue means a busy room; the periodic check covers it.
            let _ = h.tx.try_send(Cmd::Recheck);
        }
    }

    /// Kick share-link guests from a file's room, if it is open.
    pub async fn kick_share_if_open(&self, file_id: &str, token: &str) {
        let h = self.rooms.lock().unwrap().get(file_id).cloned();
        if let Some(h) = h {
            h.kick_share(token.to_string()).await;
        }
    }

    /// Notify clients of a file only if its room is open (no need to load it).
    pub async fn notify_if_open(&self, file_id: &str, msg: Value) {
        let h = self.rooms.lock().unwrap().get(file_id).cloned();
        if let Some(h) = h {
            h.broadcast(msg).await;
        }
    }
}

pub struct Rooms {
    db: Db,
    rooms: Mutex<HashMap<String, RoomHandle>>,
    /// Files temporarily unavailable (e.g. while a version is being restored).
    blocked: Mutex<std::collections::HashSet<String>>,
    /// Every room actor still running, by file, including rooms already
    /// out of `rooms` but still saving (closed when idle). Each ends with
    /// whether its last save went through.
    actors: Mutex<Vec<(String, tokio::task::JoinHandle<bool>)>>,
    /// Counts restores and deletions: a room read from the database before
    /// one isn't opened (`get` reads the file again).
    epoch: AtomicU64,
}

impl Rooms {
    pub fn new(db: Db) -> Arc<Rooms> {
        let rooms = Arc::new(Rooms { db, rooms: Mutex::new(HashMap::new()), blocked: Mutex::new(Default::default()), actors: Mutex::new(Vec::new()), epoch: AtomicU64::new(0) });
        let reaper = rooms.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            loop {
                tick.tick().await;
                reaper.reap_idle();
            }
        });
        rooms
    }

    fn reap_idle(&self) {
        self.actors.lock().unwrap().retain(|(_, a)| !a.is_finished());
        let t = now();
        let mut map = self.rooms.lock().unwrap();
        let idle: Vec<String> = map
            .iter()
            .filter(|(_, h)| {
                h.tx.is_closed()
                    || (h.stats.clients.load(Ordering::Relaxed) == 0
                        && t - h.stats.last_active.load(Ordering::Relaxed) > IDLE_CLOSE_SECS)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for id in idle {
            // A room whose queue is full is busy after all, and closes on a
            // later tick: dropped from the map without Shutdown, it would
            // run on where nothing can stop it.
            let full = map.get(&id).is_some_and(|h| matches!(h.tx.try_send(Cmd::Shutdown { notify: None, discard: false, done: None }), Err(mpsc::error::TrySendError::Full(_))));
            if !full {
                map.remove(&id);
            }
        }
    }

    /// Close a room immediately (e.g. the file was deleted).
    pub fn close(&self, file_id: &str, notify: Option<String>) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        if let Some(h) = self.rooms.lock().unwrap().remove(file_id) {
            // Sent even when the room's queue is full (`try_send` would drop it).
            tokio::spawn(async move {
                let _ = h.tx.send(Cmd::Shutdown { notify, discard: false, done: None }).await;
            });
        }
    }

    /// Close a room without saving its in-memory state and wait for it to stop.
    pub async fn discard_and_wait(&self, file_id: &str, notify: Option<String>) {
        let h = self.rooms.lock().unwrap().remove(file_id);
        if let Some(h) = h {
            let (done, stopped) = oneshot::channel();
            let _ = h.tx.send(Cmd::Shutdown { notify, discard: true, done: Some(done) }).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), stopped).await;
        }
    }

    /// Flush all rooms (graceful shutdown).
    /// Save every open document now (e.g. right before a backup).
    pub async fn flush_all(&self) {
        let handles: Vec<RoomHandle> = self.rooms.lock().unwrap().values().cloned().collect();
        for h in handles {
            let _ = h.flush().await;
        }
    }

    /// Stop every room and wait until each actor has finished its final
    /// save, rooms already closing (when idle) included. Returns whether all
    /// of the saves went through. (A room's channel closes before it saves,
    /// so that alone proves nothing.)
    pub async fn shutdown_all(&self) -> bool {
        let handles: Vec<RoomHandle> = self.rooms.lock().unwrap().drain().map(|(_, h)| h).collect();
        for h in handles {
            let _ = h.tx.send(Cmd::Shutdown { notify: None, discard: false, done: None }).await;
        }
        let actors = std::mem::take(&mut *self.actors.lock().unwrap());
        let mut all = true;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        for (file_id, actor) in actors {
            match tokio::time::timeout_at(deadline, actor).await {
                Ok(Ok(true)) => {}
                _ => {
                    tracing::error!("room {file_id} didn't finish its final save");
                    all = false;
                }
            }
        }
        all
    }

    pub fn block(&self, file_id: &str) {
        self.blocked.lock().unwrap().insert(file_id.to_string());
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub fn unblock(&self, file_id: &str) {
        self.blocked.lock().unwrap().remove(file_id);
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub async fn get(&self, file_id: &str) -> AppResult<RoomHandle> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            // Taken first: a restore or deletion from here on moves it, and
            // what this reads then isn't opened.
            let epoch = self.epoch.load(Ordering::SeqCst);
            if self.blocked.lock().unwrap().contains(file_id) {
                return Err(closed());
            }
            if let Some(h) = self.rooms.lock().unwrap().get(file_id) {
                if !h.tx.is_closed() {
                    return Ok(h.clone());
                }
            }
            // A room closing for this file may not have saved yet: open the
            // file once it has, or it would miss that save.
            if self.actors.lock().unwrap().iter().any(|(f, a)| f == file_id && !a.is_finished()) {
                if Instant::now() > deadline {
                    return Err(closed());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            let fid = file_id.to_string();
            let loaded = self
                .db
                .call(move |c| match crate::doc_store::read(c, &fid)? {
                    Some(stored) => Ok(Some((stored, crate::paint::last_seq(c, &fid)?))),
                    None => Ok(None),
                })
                .await?;
            let (stored, paint_seq) = loaded.ok_or_else(|| AppError::not_found("file"))?;
            let version = stored.version;
            let part_hashes = stored.parts.iter().map(|(name, json)| (name.clone(), hash_str(json))).collect();
            let mut doc = Document::from_json(&stored.json).map_err(|e| AppError::internal(format!("corrupt document {file_id}: {e}")))?;
            components::sync_all(&mut doc);
            flexma_doc::styles::sync_all(&mut doc);
            layout::relayout_all(&mut doc);

            let mut map = self.rooms.lock().unwrap();
            if let Some(h) = map.get(file_id) {
                if !h.tx.is_closed() {
                    return Ok(h.clone());
                }
            }
            // Restored or deleted while we read it: what we read is gone.
            if self.epoch.load(Ordering::SeqCst) != epoch || self.blocked.lock().unwrap().contains(file_id) {
                continue;
            }
            let (tx, rx) = mpsc::channel(4096);
            let stats = Arc::new(Stats { clients: AtomicUsize::new(0), last_active: AtomicI64::new(now()), paint_seq: AtomicU64::new(paint_seq) });
            let room = Room {
                file_id: file_id.to_string(),
                doc,
                version: version as u64,
                dirty: false,
                clients: HashMap::new(),
                agents: HashMap::new(),
                agent_undo: HashMap::new(),
                paint_seq,
                paint_writer: crate::paint::spawn_writer(self.db.clone()),
                stroke_authors: HashMap::new(),
                db: self.db.clone(),
                stats: stats.clone(),
                part_hashes,
                saved_parts: false,
            };
            let actor = tokio::spawn(room.run(rx));
            self.actors.lock().unwrap().push((file_id.to_string(), actor));
            let h = RoomHandle { tx, stats };
            map.insert(file_id.to_string(), h.clone());
            return Ok(h);
        }
    }
}

struct Client {
    out: mpsc::Sender<Msg>,
    peer: Peer,
    role: Role,
    credential: Credential,
}

struct Room {
    file_id: String,
    doc: Document,
    version: u64,
    dirty: bool,
    clients: HashMap<String, Client>,
    agents: HashMap<String, (Peer, Instant)>,
    /// Recent agent changes, newest last, by agent key.
    agent_undo: HashMap<String, std::collections::VecDeque<UndoBatch>>,
    /// The last brush stroke number handed out (strokes are drawn in this order).
    paint_seq: u64,
    paint_writer: mpsc::UnboundedSender<crate::paint::Write>,
    /// Who painted each stroke this session (only the painter may undo it).
    stroke_authors: HashMap<String, String>,
    db: Db,
    stats: Arc<Stats>,
    /// Hashes of the stored parts (see `doc_store`): a save writes only the
    /// parts whose JSON changed.
    part_hashes: HashMap<String, u64>,
    /// This room saved parts, so on closing it stores the whole document too.
    saved_parts: bool,
}

/// A part's JSON fingerprint, to tell which parts a save must write (kept
/// in memory only).
fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn msg(v: Value) -> Msg {
    Msg::Text(Arc::from(v.to_string()))
}

impl Room {
    /// The room's actor. Ends with whether its last save went through.
    async fn run(mut self, mut rx: mpsc::Receiver<Cmd>) -> bool {
        let mut tick = tokio::time::interval(SAVE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut recheck = tokio::time::interval_at(tokio::time::Instant::now() + RECHECK_EVERY, RECHECK_EVERY);
        recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut discarded = false;
        // Who waits for the final save (shutdown requests carry a reply).
        let mut confirm: Vec<oneshot::Sender<bool>> = Vec::new();
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Cmd::Shutdown { notify, discard, done }) => {
                        discarded = discard;
                        confirm.extend(done);
                        if discard {
                            self.dirty = false;
                        }
                        rx.close();
                        // Drain anything already queued so callers get answers.
                        while let Ok(cmd) = rx.try_recv() {
                            match cmd {
                                Cmd::Shutdown { done, .. } => confirm.extend(done),
                                cmd => self.handle(cmd),
                            }
                        }
                        if discard {
                            self.dirty = false;
                        }
                        match notify.as_deref() {
                            Some("reload") => self.broadcast(msg(json!({ "t": "reload" })), None),
                            Some(n) => self.broadcast(msg(json!({ "t": "error", "error": n, "fatal": true })), None),
                            None => {}
                        }
                        break;
                    }
                    Some(Cmd::Flush { reply }) => {
                        if self.dirty {
                            self.save().await;
                        }
                        let _ = reply.send(());
                    }
                    Some(Cmd::Recheck) => self.recheck().await,
                    Some(cmd) => {
                        self.stats.last_active.store(now(), Ordering::Relaxed);
                        self.handle(cmd);
                    }
                    None => break,
                },
                _ = tick.tick() => {
                    self.expire_agents();
                    if self.dirty {
                        self.save().await;
                    }
                }
                _ = recheck.tick() => self.recheck().await,
            }
        }
        // The last save; retried, as another write may hold the database.
        for attempt in 0..3 {
            if !self.dirty {
                break;
            }
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            self.save().await;
        }
        let mut saved = !self.dirty;
        // Servers from before parts read files.document: keep it current, once
        // per session rather than on every save.
        if self.saved_parts && !discarded && saved {
            let (fid, json, version) = (self.file_id.clone(), self.doc.to_json(), self.version as i64);
            if let Err(e) = self.db.call(move |c| Ok(crate::doc_store::write_whole(c, &fid, &json, version)?)).await {
                tracing::error!("storing {} whole failed: {}", self.file_id, e.message);
                saved = false;
            }
        }
        for done in confirm {
            let _ = done.send(saved);
        }
        tracing::debug!("room {} closed", self.file_id);
        saved
    }

    fn peers(&self) -> Vec<Peer> {
        self.clients
            .values()
            .map(|c| c.peer.clone())
            .chain(self.agents.values().map(|(p, _)| p.clone()))
            .collect()
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Join { info, out, reply } => {
                let client_id = random_string(8);
                let peer = Peer {
                    client_id: client_id.clone(),
                    user_id: info.user_id,
                    name: info.name,
                    color: info.color,
                    cursor: None,
                    selection: vec![],
                    page: None,
                    viewport: None,
                    following: None,
                    is_agent: false,
                    is_guest: matches!(info.credential, Credential::Share(_)),
                };
                let mut init = json!({
                    "t": "init",
                    "clientId": client_id,
                    "version": self.version,
                    "role": info.role.as_str(),
                    "peers": self.peers(),
                });
                // The document is most of what a client downloads on open: as
                // its own frame it goes compressed (about a tenth of the size).
                let frames = if !info.doc_frame {
                    init["doc"] = json!(self.doc.snapshot());
                    Vec::new()
                } else if let Some(first) = info.stream_pages.filter(|_| self.doc.pages().len() <= MAX_STREAMED_PAGES) {
                    // The page they open (with what every page needs) comes
                    // first and shows at once; the other pages follow. All are
                    // queued now, so no ops come between them.
                    let pages = self.doc.pages();
                    let first = first.filter(|p| pages.contains(p)).unwrap_or_else(|| pages[0].clone());
                    let (head, rest) = flexma_doc::parts::stream(&self.doc, &first);
                    let frames: Vec<Msg> = std::iter::once(head).chain(rest).map(|f| Msg::Doc(Arc::from(f))).collect();
                    init["docFrame"] = json!("zlib");
                    init["page"] = json!(first);
                    init["parts"] = json!(frames.len());
                    frames
                } else {
                    init["docFrame"] = json!("zlib");
                    vec![Msg::Doc(Arc::from(self.doc.to_json()))]
                };
                if out.try_send(msg(init)).is_err() || frames.into_iter().any(|f| out.try_send(f).is_err()) {
                    return;
                }
                self.broadcast(msg(json!({ "t": "presence", "peer": peer })), None);
                self.clients.insert(client_id.clone(), Client { out, peer, role: info.role, credential: info.credential });
                self.stats.clients.store(self.clients.len(), Ordering::Relaxed);
                let _ = reply.send(client_id);
            }
            Cmd::Leave { client_id } => {
                if self.clients.remove(&client_id).is_some() {
                    self.stats.clients.store(self.clients.len(), Ordering::Relaxed);
                    self.broadcast(msg(json!({ "t": "leave", "clientId": client_id })), None);
                }
            }
            Cmd::ClientOps { client_id, batch, ops } => self.client_ops(client_id, batch, ops),
            Cmd::Presence { client_id, update } => {
                let Some(c) = self.clients.get_mut(&client_id) else { return };
                c.peer.cursor = update.cursor;
                c.peer.selection = update.selection;
                c.peer.page = update.page;
                c.peer.viewport = update.viewport;
                c.peer.following = update.following;
                let m = msg(json!({ "t": "presence", "peer": c.peer }));
                self.broadcast(m, Some(&client_id));
            }
            Cmd::Mutate { agent, fonts, f, reply } => {
                let mut tx = Tx::new(&mut self.doc);
                tx.measurer = fonts.map(TextMeasurer::new);
                let result = f(&mut tx);
                tx.relayout();
                tx.settle_undo();
                let Tx { applied, touched, undo, .. } = tx;
                self.doc.clear_fresh();
                if !applied.is_empty() {
                    let summary = match &result {
                        Ok(v) => v.as_str().unwrap_or("change").lines().next().unwrap_or("change").trim().trim_end_matches(':').to_string(),
                        Err(_) => "partial change".to_string(),
                    };
                    let history = self.agent_undo.entry(agent.key.clone()).or_default();
                    history.push_back(UndoBatch { summary, steps: undo });
                    while history.len() > UNDO_KEEP {
                        history.pop_front();
                    }
                    self.agent_changed(agent, applied, touched);
                }
                let _ = reply.send(result);
            }
            Cmd::UndoAgent { agent, steps, reply } => {
                let result = self.undo_agent(agent, steps);
                let _ = reply.send(result);
            }
            Cmd::Read { f } => {
                let peers = self.peers();
                f(&self.doc, &peers);
            }
            Cmd::Broadcast { msg } => self.broadcast(msg, None),
            Cmd::Paint { client_id, msg } => self.paint_msg(&client_id, msg),
            Cmd::KickShare { token } => {
                let link = Credential::Share(token);
                let gone: Vec<String> = self.clients.iter().filter(|(_, c)| c.credential == link).map(|(id, _)| id.clone()).collect();
                for id in gone {
                    self.drop_client(&id, Some(LINK_GONE));
                }
            }
            Cmd::Shutdown { .. } | Cmd::Flush { .. } | Cmd::Recheck => {}
        }
    }

    /// Disconnect a socket. With `bye` the client is told why and stays
    /// away; without, it reconnects and joins again with fresh access.
    fn drop_client(&mut self, client_id: &str, bye: Option<&str>) {
        let Some(c) = self.clients.remove(client_id) else { return };
        if let Some(b) = bye {
            let _ = c.out.try_send(msg(json!({ "t": "error", "error": b, "fatal": true })));
        }
        // Dropping `c.out` ends the socket's writer, which closes the socket.
        drop(c);
        self.stats.clients.store(self.clients.len(), Ordering::Relaxed);
        self.broadcast(msg(json!({ "t": "leave", "clientId": client_id })), None);
    }

    /// Re-check every socket's sign-in and access against the database.
    async fn recheck(&mut self) {
        let who: Vec<(String, String, Role, Credential)> =
            self.clients.iter().map(|(id, c)| (id.clone(), c.peer.user_id.clone(), c.role, c.credential.clone())).collect();
        if who.is_empty() {
            return;
        }
        let file_id = self.file_id.clone();
        let verdicts = self
            .db
            .call(move |c| Ok(who.into_iter().map(|(id, user, role, cred)| (id, verdict(c, &file_id, &user, role, &cred))).collect::<Vec<_>>()))
            .await;
        let Ok(verdicts) = verdicts else { return };
        for (id, v) in verdicts {
            match v {
                Verdict::Keep => {}
                Verdict::Rejoin => self.drop_client(&id, None),
                Verdict::Revoke(why) => self.drop_client(&id, Some(why)),
            }
        }
    }

    fn client_ops(&mut self, client_id: String, batch: u64, ops: Vec<Op>) {
        let Some(client) = self.clients.get(&client_id) else { return };
        if !client.role.can_edit() {
            let corrections: Vec<Op> = ops.iter().flat_map(|op| self.doc.correction_for(op)).collect();
            let out = client.out.clone();
            let _ = out.try_send(msg(json!({ "t": "ops", "from": client_id, "batch": batch, "ops": [] })));
            let _ = out.try_send(msg(json!({ "t": "reject", "batch": batch, "error": "you have view-only access" })));
            let _ = out.try_send(msg(json!({ "t": "ops", "from": "server", "batch": 0, "ops": corrections })));
            return;
        }
        let mut accepted = Vec::with_capacity(ops.len());
        let mut rejected = Vec::new();
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        for op in components::route_ops(&self.doc, flexma_doc::styles::route_ops(&self.doc, ops)) {
            match self.doc.apply(&op) {
                Ok(eff) => {
                    changed.extend(eff.changed);
                    removed.extend(eff.removed);
                    accepted.push(op);
                }
                Err(e) => rejected.push((op, e)),
            }
        }
        // Ops that set values the nodes already have change nothing; a
        // client re-sending them (an old editor's measuring loop) costs no save.
        let any = !changed.is_empty() || !removed.is_empty();
        let synced = components::sync_instances(&mut self.doc, &changed, &removed);
        changed.extend(synced);
        let styled = flexma_doc::styles::sync_styles(&mut self.doc, &changed);
        changed.extend(styled);
        layout::relayout(&mut self.doc, &changed);
        self.doc.clear_fresh();
        let echo = msg(json!({ "t": "ops", "from": client_id, "batch": batch, "ops": accepted }));
        if any {
            self.version += 1;
            self.dirty = true;
            self.broadcast(echo, None);
        } else if let Some(c) = self.clients.get(&client_id) {
            // Nothing changed: only the sender needs it, as its acknowledgement.
            let _ = c.out.try_send(echo);
        }
        if !rejected.is_empty() {
            let errors: Vec<String> = rejected.iter().map(|(_, e)| e.clone()).collect();
            let corrections: Vec<Op> = rejected.iter().flat_map(|(op, _)| self.doc.correction_for(op)).collect();
            if let Some(c) = self.clients.get(&client_id) {
                let _ = c.out.try_send(msg(json!({ "t": "reject", "batch": batch, "error": errors.join("; ") })));
                let _ = c.out.try_send(msg(json!({ "t": "ops", "from": "server", "batch": 0, "ops": corrections })));
            }
        }
    }

    /// Brush strokes: live chunks are relayed to the others; finished strokes
    /// get the next number, are stored and go to everyone (the painter's copy
    /// is the acknowledgement).
    fn paint_msg(&mut self, client_id: &str, m: Value) {
        let Some(c) = self.clients.get(client_id) else { return };
        if !c.role.can_edit() {
            return;
        }
        let user_id = c.peer.user_id.clone();
        let reject = |this: &mut Room, id: &Value, error: &str| {
            if let Some(c) = this.clients.get(client_id) {
                let _ = c.out.try_send(msg(json!({ "t": "paintReject", "id": id, "error": error })));
            }
        };
        let is_paint_layer = |doc: &Document, id: &str| doc.get(id).is_some_and(|n| n.kind == flexma_doc::NodeType::Paint);
        match m.get("t").and_then(Value::as_str) {
            Some("strokeLive") => {
                let ok = m.get("layer").and_then(Value::as_str).is_some_and(|l| is_paint_layer(&self.doc, l))
                    && m.get("points").and_then(Value::as_array).is_some_and(|p| p.len() <= 4000);
                if ok {
                    let mut out = m;
                    out["from"] = json!(client_id);
                    self.broadcast(msg(out), Some(client_id));
                }
            }
            Some("stroke") => {
                let stroke = match m.get("stroke").cloned().map(serde_json::from_value::<flexma_doc::raster::Stroke>) {
                    Some(Ok(s)) => s,
                    _ => return reject(self, &json!(null), "malformed stroke"),
                };
                let id = json!(stroke.id);
                if let Err(e) = stroke.validate() {
                    return reject(self, &id, &e);
                }
                if !is_paint_layer(&self.doc, &stroke.layer) {
                    return reject(self, &id, "that layer isn't a paint layer");
                }
                if self.stroke_authors.contains_key(&stroke.id) {
                    return;
                }
                let stroke = flexma_doc::raster::Stroke { brush: stroke.brush.sanitized(), ..stroke };
                self.paint_seq += 1;
                let seq = self.paint_seq;
                self.stats.paint_seq.store(seq, Ordering::Relaxed);
                self.stroke_authors.insert(stroke.id.clone(), user_id.clone());
                let out = json!({ "t": "stroke", "seq": seq, "stroke": stroke, "from": client_id, "user": user_id });
                let _ = self.paint_writer.send(crate::paint::Write::Insert { file_id: self.file_id.clone(), seq, user_id, stroke: Box::new(stroke) });
                self.broadcast(msg(out), None);
            }
            Some("strokeUndo") => {
                let Some(id) = m.get("id").and_then(Value::as_str).map(str::to_string) else { return };
                let undone = m.get("undone").and_then(Value::as_bool).unwrap_or(true);
                if self.stroke_authors.get(&id) != Some(&user_id) {
                    return reject(self, &json!(id), "only the painter can undo a stroke, during the session it was painted in");
                }
                let _ = self.paint_writer.send(crate::paint::Write::Undo { file_id: self.file_id.clone(), id: id.clone(), undone });
                self.broadcast(msg(json!({ "t": "strokeUndo", "id": id, "undone": undone, "from": client_id })), None);
            }
            _ => {}
        }
    }

    fn broadcast(&mut self, m: Msg, except: Option<&str>) {
        let mut dead = Vec::new();
        for (id, c) in &self.clients {
            if Some(id.as_str()) == except {
                continue;
            }
            if c.out.try_send(m.clone()).is_err() {
                dead.push(id.clone());
            }
        }
        for id in dead {
            tracing::warn!("dropping slow or closed client {id} in room {}", self.file_id);
            self.clients.remove(&id);
            self.stats.clients.store(self.clients.len(), Ordering::Relaxed);
            let leave = msg(json!({ "t": "leave", "clientId": id }));
            for c in self.clients.values() {
                let _ = c.out.try_send(leave.clone());
            }
        }
    }

    /// Broadcast an agent's applied ops and show where it worked.
    fn agent_changed(&mut self, agent: AgentInfo, applied: Vec<Op>, touched: Vec<NodeId>) {
        self.version += 1;
        self.dirty = true;
        let from = format!("agent-{}", agent.key);
        self.broadcast(msg(json!({ "t": "ops", "from": from, "batch": 0, "ops": applied })), None);
        let mut sel: Vec<String> = touched.into_iter().filter(|id| self.doc.contains(id)).collect();
        sel.dedup();
        // Highlight only the outermost touched nodes.
        let set: std::collections::HashSet<String> = sel.iter().cloned().collect();
        sel.retain(|id| !self.doc.ancestors(id).iter().any(|a| set.contains(a)));
        sel.truncate(200);
        let page = sel.first().and_then(|id| self.doc.page_of(id));
        let cursor = sel.first().map(|id| {
            let b = self.doc.world_bounds(id);
            [b.right(), b.bottom()]
        });
        let peer = Peer {
            client_id: from,
            user_id: agent.user_id,
            name: agent.name,
            color: agent.color,
            cursor,
            selection: sel,
            page,
            viewport: None,
            following: None,
            is_agent: true,
            is_guest: false,
        };
        self.broadcast(msg(json!({ "t": "presence", "peer": peer })), None);
        self.agents.insert(agent.key, (peer, Instant::now()));
    }

    /// Reverse an agent's last `steps` changes in this file, newest first.
    /// Each change is undone against the state the previous undo left, so a
    /// create followed by a delete (or an edit) reverses cleanly.
    fn undo_agent(&mut self, agent: AgentInfo, steps: usize) -> Result<Value, String> {
        let history = self.agent_undo.entry(agent.key.clone()).or_default();
        if history.is_empty() {
            return Err("there's nothing to undo: no changes of yours are recorded in this file since the server started".into());
        }
        let batches: Vec<UndoBatch> = (0..steps).filter_map(|_| history.pop_back()).collect();
        let (mut applied, mut touched) = (Vec::new(), Vec::new());
        let (mut kept, mut failed) = (0usize, 0usize);
        for batch in &batches {
            let mut ops = Vec::new();
            let mut restored: HashMap<NodeId, flexma_doc::Node> = HashMap::new();
            for step in batch.steps.iter().rev() {
                match step {
                    UndoStep::Prop { id, key, old, new } => {
                        // A node this batch restores is checked against its snapshot.
                        let current = restored.get(id).or_else(|| self.doc.get(id)).and_then(|n| n.get_prop(key));
                        match current {
                            Some(cur) if cur == *new => {
                                let mut props = serde_json::Map::new();
                                props.insert(key.clone(), old.clone());
                                ops.push(Op::Update { id: id.clone(), props });
                            }
                            Some(_) => kept += 1,
                            None => {}
                        }
                    }
                    UndoStep::Remove { id } => {
                        if self.doc.contains(id) {
                            ops.push(Op::Delete { id: id.clone() });
                        }
                    }
                    UndoStep::Restore { nodes } => {
                        for n in nodes {
                            let parent_ok = n.parent.as_deref().map(|p| self.doc.contains(p) || restored.contains_key(p)).unwrap_or(false);
                            if !self.doc.contains(&n.id) && parent_ok {
                                restored.insert(n.id.clone(), n.clone());
                                ops.push(Op::Create { node: n.clone() });
                            }
                        }
                    }
                }
            }
            let mut tx = Tx::new(&mut self.doc);
            failed += tx.apply_all(ops).len();
            tx.relayout();
            let Tx { applied: a, touched: t, .. } = tx;
            self.doc.clear_fresh();
            applied.extend(a);
            touched.extend(t);
        }
        let undone: Vec<String> = batches.iter().map(|b| format!("\"{}\"", b.summary)).collect();
        if !applied.is_empty() {
            self.agent_changed(agent, applied, touched);
        }
        let mut out = format!("Undid {} change{}: {}.", batches.len(), if batches.len() == 1 { "" } else { "s" }, undone.join(", "));
        if kept > 0 {
            out.push_str(&format!(" Kept {kept} propert{} that someone changed after you.", if kept == 1 { "y" } else { "ies" }));
        }
        if failed > 0 {
            out.push_str(&format!(" {failed} step(s) couldn't be applied."));
        }
        Ok(json!(out))
    }

    fn expire_agents(&mut self) {
        let stale: Vec<String> = self
            .agents
            .iter()
            .filter(|(_, (_, t))| t.elapsed() > AGENT_PRESENCE_TTL)
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            if let Some((p, _)) = self.agents.remove(&k) {
                self.broadcast(msg(json!({ "t": "leave", "clientId": p.client_id })), None);
            }
        }
    }

    async fn save(&mut self) {
        let version = self.version as i64;
        let fid = self.file_id.clone();
        // Every part's JSON; only those that differ from what's stored are written.
        let parts: Vec<(String, String)> = flexma_doc::parts::split(&self.doc).named().map(|(name, nodes)| (name, flexma_doc::parts::to_json(nodes))).collect();
        let hashes: HashMap<String, u64> = parts.iter().map(|(name, json)| (name.clone(), hash_str(json))).collect();
        let changed: Vec<bool> = parts.iter().map(|(name, _)| self.part_hashes.get(name) != hashes.get(name)).collect();
        let res = self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let write: Vec<(&str, &str)> = parts.iter().zip(&changed).filter(|(_, ch)| **ch).map(|((name, json), _)| (name.as_str(), json.as_str())).collect();
                let all: Vec<&str> = parts.iter().map(|(name, _)| name.as_str()).collect();
                crate::doc_store::write_parts(&tx, &fid, version, &write, &all)?;
                // Automatic version history: keep a snapshot at most every 15 minutes of editing.
                let last: Option<i64> = tx
                    .query_row(
                        "SELECT MAX(created_at) FROM file_versions WHERE file_id = ?1 AND label IS NULL",
                        params![fid],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                if last.map(|t| now() - t >= AUTO_VERSION_SECS).unwrap_or(true) {
                    let json = flexma_doc::parts::assemble(parts.iter().map(|(_, json)| json.as_str()));
                    tx.execute(
                        "INSERT INTO file_versions (id, file_id, label, document, version, created_by, created_at)
                         VALUES (?1, ?2, NULL, ?3, ?4, NULL, ?5)",
                        params![crate::db::new_id(), fid, json, version, now()],
                    )?;
                    // Keep the newest 200 automatic versions.
                    tx.execute(
                        "DELETE FROM file_versions WHERE file_id = ?1 AND label IS NULL AND id NOT IN
                         (SELECT id FROM file_versions WHERE file_id = ?1 AND label IS NULL ORDER BY created_at DESC LIMIT 200)",
                        params![fid],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await;
        match res {
            Ok(()) => {
                self.dirty = false;
                self.part_hashes = hashes;
                self.saved_parts = true;
            }
            Err(e) => tracing::error!("saving {} failed: {}", self.file_id, e.message),
        }
    }
}

#[cfg(test)]
mod access_tests {
    use super::*;
    use crate::db::sha256_hex;

    const LINK: &str = "abcdefghijklmnopqrstuvwxyz012345";

    /// Editor "u" (session "s", API token "t") in the workspace of file "f",
    /// which has a public link.
    async fn setup(name: &str) -> (Db, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("flexma-access-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("t.db")).unwrap();
        let doc = Document::new_empty().to_json();
        db.call(move |c| {
            c.execute_batch(
                "INSERT INTO users (id, email, name, password_hash, color, created_at) VALUES ('u', 'u@x', 'U', '-', '#000', 0);
                 INSERT INTO workspaces (id, name, created_at) VALUES ('w', 'W', 0);
                 INSERT INTO workspace_members (workspace_id, user_id, role, created_at) VALUES ('w', 'u', 'editor', 0);
                 INSERT INTO projects (id, workspace_id, name, created_at) VALUES ('p', 'w', 'P', 0);",
            )?;
            c.execute("INSERT INTO files (id, project_id, name, document, created_at, updated_at) VALUES ('f', 'p', 'F', ?1, 0, 0)", params![doc])?;
            c.execute("INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, 'u', 0, ?2)", params![sha256_hex("s"), now() + 3600])?;
            c.execute(
                "INSERT INTO api_tokens (id, user_id, name, prefix, token_hash, created_at) VALUES ('t1', 'u', 'CI', 'fx', ?1, 0)",
                params![sha256_hex("t")],
            )?;
            c.execute("INSERT INTO file_shares (file_id, token, created_at) VALUES ('f', ?1, 0)", params![LINK])?;
            Ok(())
        })
        .await
        .unwrap();
        (db, dir)
    }

    async fn sql(db: &Db, q: &'static str) {
        db.call(move |c| Ok(c.execute_batch(q)?)).await.unwrap();
    }

    async fn check(db: &Db, role: Role, cred: Credential) -> Verdict {
        db.call(move |c| Ok(verdict(c, "f", "u", role, &cred))).await.unwrap()
    }

    #[tokio::test]
    async fn verdicts_follow_sign_in_and_access() {
        let (db, dir) = setup("verdicts").await;
        let session = || Credential::Session(sha256_hex("s"));
        let token = || Credential::Token(sha256_hex("t"));
        let link = || Credential::Share(LINK.into());
        assert_eq!(check(&db, Role::Editor, session()).await, Verdict::Keep);
        assert_eq!(check(&db, Role::Editor, token()).await, Verdict::Keep);
        assert_eq!(check(&db, Role::Viewer, link()).await, Verdict::Keep);

        sql(&db, "UPDATE workspace_members SET role = 'viewer'").await;
        assert_eq!(check(&db, Role::Editor, session()).await, Verdict::Rejoin, "demoted");
        sql(&db, "DELETE FROM api_tokens").await;
        assert_eq!(check(&db, Role::Viewer, token()).await, Verdict::Revoke(SIGNED_OUT), "token revoked");
        sql(&db, "DELETE FROM workspace_members").await;
        assert_eq!(check(&db, Role::Viewer, session()).await, Verdict::Revoke(NO_ACCESS), "removed");
        sql(&db, "DELETE FROM sessions").await;
        assert_eq!(check(&db, Role::Viewer, session()).await, Verdict::Revoke(SIGNED_OUT), "signed out");
        sql(&db, "UPDATE file_shares SET expires_at = 1").await;
        assert_eq!(check(&db, Role::Viewer, link()).await, Verdict::Revoke(LINK_GONE), "link expired");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Join the room for "f" as "u"; returns the socket's outbound queue.
    async fn join(rooms: &Rooms, role: Role) -> mpsc::Receiver<Msg> {
        let (out, mut rx) = mpsc::channel(CLIENT_QUEUE);
        let info = JoinInfo { user_id: "u".into(), name: "U".into(), color: "#000".into(), role, credential: Credential::Session(sha256_hex("s")), doc_frame: false, stream_pages: None };
        rooms.get("f").await.unwrap().join(info, out).await.unwrap();
        assert!(rx.recv().await.unwrap().contains(r#""t":"init""#));
        rx
    }

    /// Shutting down waits for each room's last save, even while another
    /// write holds the database (the room's channel closes before it saves).
    #[tokio::test]
    async fn shutdown_waits_for_the_last_save() {
        let (db, dir) = setup("shutdown").await;
        let rooms = Rooms::new(db.clone());
        let room = rooms.get("f").await.unwrap();
        let (out, _rx) = mpsc::channel(CLIENT_QUEUE);
        let info = JoinInfo { user_id: "u".into(), name: "U".into(), color: "#000".into(), role: Role::Editor, credential: Credential::Session(sha256_hex("s")), doc_frame: false, stream_pages: None };
        let me = room.join(info, out).await.unwrap();
        let mut a = flexma_doc::Node::new("a:1", flexma_doc::NodeType::Frame);
        a.parent = Some("0:1".into());
        room.client_ops(me, 1, vec![Op::Create { node: a }]).await;
        // Another write holds the database for a while.
        let busy = db.clone();
        let holder = tokio::spawn(async move { busy.call(|_| Ok(std::thread::sleep(Duration::from_millis(1500)))).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let t = Instant::now();
        assert!(rooms.shutdown_all().await, "the room confirmed its save");
        assert!(t.elapsed() >= Duration::from_millis(1200), "returned after the save, not when the channel closed ({:?})", t.elapsed());
        let saved: String = db.call(|c| crate::doc_store::read_json(c, "f")).await.unwrap();
        assert!(Document::from_json(&saved).unwrap().contains("a:1"), "saved before shutdown returned");
        holder.await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Rooms save documents in parts and write only the parts that changed;
    /// closing stores the whole document too, and a file saved whole later
    /// (by an older server) is read whole again.
    #[tokio::test]
    async fn documents_are_saved_in_parts() {
        let (db, dir) = setup("parts").await;
        sql(
            &db,
            "CREATE TABLE part_writes (part TEXT);
             CREATE TRIGGER part_insert AFTER INSERT ON file_parts BEGIN INSERT INTO part_writes VALUES (NEW.part); END;
             CREATE TRIGGER part_update AFTER UPDATE ON file_parts BEGIN INSERT INTO part_writes VALUES (NEW.part); END;",
        )
        .await;
        let writes = |db: &Db| {
            let db = db.clone();
            async move {
                db.call(|c| {
                    let w: Vec<String> = c.prepare("SELECT part FROM part_writes ORDER BY part")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
                    c.execute("DELETE FROM part_writes", [])?;
                    Ok(w)
                })
                .await
                .unwrap()
            }
        };
        let node = |id: &str, kind: flexma_doc::NodeType, parent: &str, name: &str| {
            let mut n = flexma_doc::Node::new(id, kind);
            n.parent = Some(parent.into());
            n.name = name.into();
            n.index = id.into();
            Op::Create { node: n }
        };
        let rooms = Rooms::new(db.clone());
        let room = rooms.get("f").await.unwrap();
        let (out, _rx) = mpsc::channel(CLIENT_QUEUE);
        let info = JoinInfo { user_id: "u".into(), name: "U".into(), color: "#000".into(), role: Role::Editor, credential: Credential::Session(sha256_hex("s")), doc_frame: false, stream_pages: None };
        let me = room.join(info, out).await.unwrap();
        use flexma_doc::NodeType::{Frame, Page};
        room.client_ops(me.clone(), 1, vec![node("p:2", Page, flexma_doc::ROOT_ID, "Two"), node("a:1", Frame, "0:1", "A"), node("b:1", Frame, "p:2", "B")]).await;
        room.flush().await.unwrap();
        // The first save writes every part.
        assert_eq!(writes(&db).await, ["page:0:1", "page:p:2", "shared"]);
        // A change on page two writes page two only.
        room.client_ops(me.clone(), 2, vec![Op::update1("b:1", "name", json!("B2"))]).await;
        room.flush().await.unwrap();
        assert_eq!(writes(&db).await, ["page:p:2"]);
        let (version, parts_version): (i64, Option<i64>) = db.call(|c| Ok(c.query_row("SELECT version, parts_version FROM files WHERE id = 'f'", [], |r| Ok((r.get(0)?, r.get(1)?)))?)).await.unwrap();
        assert_eq!(Some(version), parts_version);
        // Reopened from its parts; closing stored it whole as well.
        rooms.shutdown_all().await;
        let whole: String = db.call(|c| Ok(c.query_row("SELECT document FROM files WHERE id = 'f'", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(Document::from_json(&whole).unwrap().get("b:1").map(|n| n.name.clone()), Some("B2".into()));
        let rooms = Rooms::new(db.clone());
        let name = rooms.get("f").await.unwrap().read(|d, _| d.get("b:1").map(|n| n.name.clone())).await.unwrap();
        assert_eq!(name.as_deref(), Some("B2"));
        rooms.shutdown_all().await;
        // An older server saves it whole (its version moves past the parts).
        let older = Document::new_empty().to_json();
        db.call(move |c| Ok(c.execute("UPDATE files SET document = ?1, version = version + 1 WHERE id = 'f'", params![older])?)).await.unwrap();
        let rooms = Rooms::new(db.clone());
        let has = rooms.get("f").await.unwrap().read(|d, _| d.contains("b:1")).await.unwrap();
        assert!(!has, "the whole document wins over older parts");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A room closing because it was idle can still be saving an edit that
    /// arrived just before (its first try can fail, and it retries a second
    /// later): opening the file again reads it after that save, and shutting
    /// down waits for it.
    #[tokio::test]
    async fn closing_rooms_finish_saving_first() {
        let (db, dir) = setup("closing").await;
        sql(
            &db,
            "CREATE TABLE failing (x);
             CREATE TRIGGER fail_insert BEFORE INSERT ON file_parts WHEN EXISTS (SELECT 1 FROM failing) BEGIN SELECT RAISE(ABORT, 'disk full'); END;
             CREATE TRIGGER fail_update BEFORE UPDATE ON file_parts WHEN EXISTS (SELECT 1 FROM failing) BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
        )
        .await;
        let rooms = Rooms::new(db.clone());
        // An agent's edit is queued as the reaper finds the room idle (the
        // room marks itself active only once it handles the edit), and the
        // room's first try at saving it fails.
        let close_after_edit = |id: &'static str| {
            let rooms = rooms.clone();
            async move {
                let room = rooms.get("f").await.unwrap();
                sql(&rooms.db, "INSERT INTO failing VALUES (1)").await;
                room.stats.last_active.store(now() - IDLE_CLOSE_SECS - 1, Ordering::Relaxed);
                let (reply, edited) = oneshot::channel();
                let agent = AgentInfo { key: "a".into(), user_id: "u".into(), name: "Agent".into(), color: "#000".into() };
                let f = Box::new(move |tx: &mut Tx| {
                    let mut n = flexma_doc::Node::new(id, flexma_doc::NodeType::Rectangle);
                    n.parent = Some("0:1".into());
                    tx.apply(Op::Create { node: n })?;
                    Ok(json!("ok"))
                });
                room.tx.try_send(Cmd::Mutate { agent, fonts: None, f, reply }).unwrap();
                rooms.reap_idle();
                edited.await.unwrap().unwrap();
                assert!(rooms.rooms.lock().unwrap().is_empty(), "closed as idle");
            }
        };
        close_after_edit("late:1").await;
        let again = rooms.clone();
        let reopen = tokio::spawn(async move { again.get("f").await.unwrap().read(|d, _| d.contains("late:1")).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let reopen_waited = !reopen.is_finished();
        sql(&db, "DELETE FROM failing").await;
        let reopened = reopen.await.unwrap();
        close_after_edit("late:2").await;
        let all = rooms.clone();
        let shutdown = tokio::spawn(async move { all.shutdown_all().await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let shutdown_waited = !shutdown.is_finished();
        sql(&db, "DELETE FROM failing").await;
        let saved = shutdown.await.unwrap();
        let stored = Document::from_json(&db.call(|c| crate::doc_store::read_json(c, "f")).await.unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        assert!(reopen_waited && reopened, "opened after the closing room saved");
        assert!(shutdown_waited && saved, "shut down after the closing room saved");
        assert!(stored.contains("late:1") && stored.contains("late:2"));
    }

    /// Lets a test run a future until its first wait ends.
    struct NotifyWake(Arc<tokio::sync::Notify>);

    impl std::task::Wake for NotifyWake {
        fn wake(self: Arc<Self>) {
            self.0.notify_one();
        }
    }

    /// An open that read the file before a version was restored reads it
    /// again, rather than opening (and later saving) what was replaced.
    #[tokio::test]
    async fn opening_during_a_restore_reads_the_restored_file() {
        use axum::extract::{Path, State};
        use std::future::Future;
        let (db, dir) = setup("restore-race").await;
        db.call(|c| {
            let mut doc = Document::new_empty();
            let mut n = flexma_doc::Node::new("restored", flexma_doc::NodeType::Rectangle);
            n.parent = Some("0:1".into());
            doc.apply(&Op::Create { node: n }).unwrap();
            c.execute("INSERT INTO file_versions (id, file_id, label, document, version, created_by, created_at) VALUES ('v', 'f', 'Checkpoint', ?1, 1, 'u', 0)", params![doc.to_json()])?;
            Ok(())
        })
        .await
        .unwrap();
        let rooms = Rooms::new(db.clone());
        // Run an open until it has read the file.
        let read = Arc::new(tokio::sync::Notify::new());
        let waker = std::task::Waker::from(Arc::new(NotifyWake(read.clone())));
        let mut open = Box::pin(rooms.get("f"));
        assert!(open.as_mut().poll(&mut std::task::Context::from_waker(&waker)).is_pending());
        read.notified().await;
        let cfg = crate::config::Config {
            addr: "unused".into(),
            data_dir: dir.clone(),
            web_dir: dir.clone(),
            public_url: None,
            allow_signup: false,
            instance_name: "test".into(),
            max_upload_bytes: 1_000_000,
            secure_cookies: false,
            setup_token: None,
            max_export_px: 1024,
        };
        let st = crate::state::AppState { cfg: Arc::new(cfg), db: db.clone(), rooms: rooms.clone() };
        let user = crate::auth::AuthUser {
            id: "u".into(),
            email: "u@x".into(),
            name: "U".into(),
            is_admin: false,
            color: "#000".into(),
            via_token: false,
            via_oauth: false,
            token_name: None,
        };
        let _ = crate::api_versions::restore(State(st), user, Path(("f".into(), "v".into()))).await.unwrap();
        let room = open.await.unwrap();
        let agent = AgentInfo { key: "a".into(), user_id: "u".into(), name: "Agent".into(), color: "#000".into() };
        let edit = room.mutate(agent, None, |tx| {
            let mut n = flexma_doc::Node::new("edit:1", flexma_doc::NodeType::Rectangle);
            n.parent = Some("0:1".into());
            tx.apply(Op::Create { node: n })?;
            Ok(json!("ok"))
        });
        edit.await.unwrap().unwrap();
        assert!(rooms.shutdown_all().await);
        let saved = Document::from_json(&db.call(|c| crate::doc_store::read_json(c, "f")).await.unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        assert!(saved.contains("restored") && saved.contains("edit:1"));
    }

    /// A restore that blocks the file just after an open checked it still
    /// keeps that open from opening the old file. (The open is held at the
    /// room map, which it locks after checking the block.)
    #[tokio::test]
    async fn opens_racing_a_restore_never_open_the_old_file() {
        use std::future::Future;
        let (db, dir) = setup("restore-block").await;
        let rooms = Rooms::new(db.clone());
        let read = Arc::new(tokio::sync::Notify::new());
        let waker = std::task::Waker::from(Arc::new(NotifyWake(read.clone())));
        let map = rooms.rooms.lock().unwrap();
        let (other, runtime) = (rooms.clone(), tokio::runtime::Handle::current());
        let (polled_tx, polled) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _entered = runtime.enter();
            let mut open = Box::pin(async move { other.get("f").await });
            let pending = open.as_mut().poll(&mut std::task::Context::from_waker(&waker)).is_pending();
            polled_tx.send((open, pending)).unwrap();
        });
        // Time for the open to pass the check and wait for the map.
        std::thread::sleep(Duration::from_millis(100));
        rooms.block("f");
        drop(map);
        let (open, pending) = polled.recv_timeout(Duration::from_secs(5)).unwrap();
        thread.join().unwrap();
        assert!(pending, "setup: the open checked the block before it was set");
        read.notified().await;
        rooms.discard_and_wait("f", Some("reload".into())).await;
        let opened_while_blocked = open.await.is_ok();
        db.call(|c| {
            let mut doc = Document::new_empty();
            let mut n = flexma_doc::Node::new("restored", flexma_doc::NodeType::Rectangle);
            n.parent = Some("0:1".into());
            doc.apply(&Op::Create { node: n }).unwrap();
            c.execute("UPDATE files SET document = ?1, version = version + 1 WHERE id = 'f'", params![doc.to_json()])?;
            Ok(())
        })
        .await
        .unwrap();
        rooms.unblock("f");
        let room = rooms.get("f").await.unwrap();
        let agent = AgentInfo { key: "a".into(), user_id: "u".into(), name: "Agent".into(), color: "#000".into() };
        let edit = room.mutate(agent, None, |tx| {
            let mut n = flexma_doc::Node::new("later:1", flexma_doc::NodeType::Rectangle);
            n.parent = Some("0:1".into());
            tx.apply(Op::Create { node: n })?;
            Ok(json!("ok"))
        });
        edit.await.unwrap().unwrap();
        assert!(rooms.shutdown_all().await);
        let saved = Document::from_json(&db.call(|c| crate::doc_store::read_json(c, "f")).await.unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        assert!(!opened_while_blocked && saved.contains("restored") && saved.contains("later:1"));
    }

    /// A room whose command queue is full isn't closed as idle, and closing
    /// it (a deleted file) still reaches it: a room out of the map but never
    /// told to stop would keep the file from opening again.
    #[tokio::test]
    async fn rooms_with_a_full_queue_still_close() {
        let (db, dir) = setup("full-queue").await;
        let rooms = Rooms::new(db.clone());
        let room = rooms.get("f").await.unwrap();
        // Queued before the room handles any of it, while it still looks idle.
        while room.tx.try_send(Cmd::Read { f: Box::new(|_, _| {}) }).is_ok() {}
        room.stats.last_active.store(now() - IDLE_CLOSE_SECS - 1, Ordering::Relaxed);
        rooms.reap_idle();
        let again = tokio::time::timeout(Duration::from_secs(5), rooms.get("f")).await.expect("opened").unwrap();
        assert!(again.tx.same_channel(&room.tx), "still the open room");
        rooms.close("f", None);
        let reopened = tokio::time::timeout(Duration::from_secs(5), rooms.get("f")).await.expect("opened again").unwrap();
        assert!(!reopened.tx.same_channel(&room.tx));
        assert!(rooms.shutdown_all().await);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A page can have any id, even the shared part's name.
    #[tokio::test]
    async fn a_page_named_like_the_shared_part_is_kept() {
        let (db, dir) = setup("shared-page").await;
        let rooms = Rooms::new(db.clone());
        let room = rooms.get("f").await.unwrap();
        let (out, _rx) = mpsc::channel(CLIENT_QUEUE);
        let info = JoinInfo { user_id: "u".into(), name: "U".into(), color: "#000".into(), role: Role::Editor, credential: Credential::Session(sha256_hex("s")), doc_frame: false, stream_pages: None };
        let me = room.join(info, out).await.unwrap();
        let mut page = flexma_doc::Node::new(flexma_doc::parts::SHARED, flexma_doc::NodeType::Page);
        page.parent = Some(flexma_doc::ROOT_ID.into());
        let mut rect = flexma_doc::Node::new("r:1", flexma_doc::NodeType::Rectangle);
        rect.parent = Some("0:1".into());
        room.client_ops(me, 1, vec![Op::Create { node: page }, Op::Create { node: rect }]).await;
        room.flush().await.unwrap();
        assert!(rooms.shutdown_all().await);
        let json = db.call(|c| crate::doc_store::read_json(c, "f")).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
        let stored = Document::from_json(&json).unwrap();
        assert!(stored.contains(flexma_doc::parts::SHARED) && stored.contains("r:1"), "{json}");
    }

    /// Version history, restoring, duplicating and thumbnails read the parts
    /// while `files.document` is behind them.
    #[tokio::test]
    async fn versions_copies_and_thumbnails_use_the_parts() {
        use axum::extract::{Path, State};
        use axum::Json;
        let (db, dir) = setup("parts-api").await;
        let rooms = Rooms::new(db.clone());
        let cfg = crate::config::Config {
            addr: "unused".into(),
            data_dir: dir.clone(),
            web_dir: dir.clone(),
            public_url: None,
            allow_signup: false,
            instance_name: "test".into(),
            max_upload_bytes: 1_000_000,
            secure_cookies: false,
            setup_token: None,
            max_export_px: 1024,
        };
        let st = crate::state::AppState { cfg: Arc::new(cfg), db: db.clone(), rooms: rooms.clone() };
        let user = || crate::auth::AuthUser {
            id: "u".into(),
            email: "u@x".into(),
            name: "U".into(),
            is_admin: false,
            color: "#000".into(),
            via_token: false,
            via_oauth: false,
            token_name: None,
        };
        // The room drops a client whose socket is gone, so the socket comes back too.
        let open = || async {
            let (out, socket) = mpsc::channel(CLIENT_QUEUE);
            let info = JoinInfo { user_id: "u".into(), name: "U".into(), color: "#000".into(), role: Role::Editor, credential: Credential::Session(sha256_hex("s")), doc_frame: false, stream_pages: None };
            let room = rooms.get("f").await.unwrap();
            let me = room.join(info, out).await.unwrap();
            (room, me, socket)
        };
        let name = |json: &str| Document::from_json(json).unwrap().get("r:1").map(|n| n.name.clone()).unwrap_or_default();
        let whole = || async { db.call(|c| Ok(c.query_row::<String, _, _>("SELECT document FROM files WHERE id = 'f'", [], |r| r.get(0))?)).await.unwrap() };
        let (room, me, _socket) = open().await;
        let mut rect = flexma_doc::Node::new("r:1", flexma_doc::NodeType::Rectangle);
        rect.parent = Some("0:1".into());
        (rect.name, rect.width, rect.height) = ("saved".into(), 100.0, 100.0);
        rect.fills = vec![flexma_doc::Paint::solid(flexma_doc::Color::BLACK)];
        room.client_ops(me.clone(), 1, vec![Op::Create { node: rect }]).await;
        room.flush().await.unwrap();
        assert_eq!(name(&whole().await), "", "the whole copy waits for the room to close");
        let auto: String = db.call(|c| Ok(c.query_row("SELECT document FROM file_versions WHERE file_id = 'f' AND label IS NULL", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(name(&auto), "saved");
        let req = serde_json::from_value(json!({ "label": "checkpoint" })).unwrap();
        let Json(checkpoint) = crate::api_versions::create(State(st.clone()), user(), Path("f".into()), Json(req)).await.unwrap();
        room.client_ops(me, 2, vec![Op::update1("r:1", "name", json!("current"))]).await;
        room.flush().await.unwrap();
        let Json(copy) = crate::api_files::duplicate_file(State(st.clone()), user(), Path("f".into())).await.unwrap();
        assert_eq!(name(&crate::api_files::current_doc_json(&st, copy["id"].as_str().unwrap()).await.unwrap()), "current");
        crate::api_files::thumbnail_response(&st, "f").await.unwrap();
        let png: Option<Vec<u8>> = db.call(|c| Ok(c.query_row("SELECT thumbnail_png FROM files WHERE id = 'f' AND thumbnail_version = version", [], |r| r.get(0))?)).await.unwrap();
        assert!(png.is_some(), "a thumbnail of the current parts");
        let vid = checkpoint["id"].as_str().unwrap().to_string();
        let _ = crate::api_versions::restore(State(st.clone()), user(), Path(("f".into(), vid))).await.unwrap();
        assert_eq!(name(&crate::api_files::current_doc_json(&st, "f").await.unwrap()), "saved");
        let before: String = db.call(|c| Ok(c.query_row("SELECT document FROM file_versions WHERE file_id = 'f' AND label = 'Before restore'", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(name(&before), "current");
        // A late whole-document write from the closed room loses to the restore.
        db.call(|c| Ok(crate::doc_store::write_whole(c, "f", r#"{"nodes":[]}"#, 2)?)).await.unwrap();
        assert_eq!(name(&db.call(|c| crate::doc_store::read_json(c, "f")).await.unwrap()), "saved");
        let (room, me, _socket) = open().await;
        room.client_ops(me, 1, vec![Op::update1("r:1", "name", json!("after restore"))]).await;
        assert!(rooms.shutdown_all().await);
        let stored = db.call(|c| crate::doc_store::read_json(c, "f")).await.unwrap();
        assert_eq!((name(&stored), name(&whole().await)), ("after restore".into(), "after restore".into()));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// With `&pages=stream`, the page asked for comes first (with what every
    /// page needs) and the other pages follow, one frame each.
    #[tokio::test]
    async fn pages_can_stream_in_after_the_first() {
        let (db, dir) = setup("stream").await;
        let rooms = Rooms::new(db.clone());
        let room = rooms.get("f").await.unwrap();
        let (out, _rx) = mpsc::channel(CLIENT_QUEUE);
        let info = |stream_pages| JoinInfo { user_id: "u".into(), name: "U".into(), color: "#000".into(), role: Role::Editor, credential: Credential::Session(sha256_hex("s")), doc_frame: true, stream_pages };
        let me = room.join(info(None), out).await.unwrap();
        let mut page2 = flexma_doc::Node::new("p:2", flexma_doc::NodeType::Page);
        page2.parent = Some(flexma_doc::ROOT_ID.into());
        page2.index = "b".into();
        let mut a = flexma_doc::Node::new("a:1", flexma_doc::NodeType::Frame);
        a.parent = Some("0:1".into());
        let mut b = flexma_doc::Node::new("b:1", flexma_doc::NodeType::Frame);
        b.parent = Some("p:2".into());
        room.client_ops(me, 1, vec![Op::Create { node: page2 }, Op::Create { node: a }, Op::Create { node: b }]).await;
        let (out, mut rx) = mpsc::channel(CLIENT_QUEUE);
        room.join(info(Some(Some("p:2".into()))), out).await.unwrap();
        let init: Value = serde_json::from_str(&next(&mut rx).await.unwrap()).unwrap();
        assert_eq!((init["page"].as_str(), init["parts"].as_u64()), (Some("p:2"), Some(2)));
        let Some(Msg::Doc(first)) = next(&mut rx).await else { panic!("the first part") };
        let first = Document::from_json(&first).unwrap();
        assert!(first.contains("b:1") && !first.contains("a:1") && first.contains("0:1"), "page two, and every page in the list");
        let Some(Msg::Doc(rest)) = next(&mut rx).await else { panic!("page one") };
        let part: flexma_doc::parts::PagePart = serde_json::from_str(&rest).unwrap();
        assert_eq!(part.page, "0:1");
        assert_eq!(part.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["a:1"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// With `?doc=zlib`, `init` leaves the document out and it follows as
    /// its own frame, which the socket writer compresses.
    #[tokio::test]
    async fn the_document_can_follow_init_as_its_own_frame() {
        let (db, dir) = setup("docframe").await;
        let rooms = Rooms::new(db.clone());
        let (out, mut rx) = mpsc::channel(CLIENT_QUEUE);
        let info = JoinInfo { user_id: "u".into(), name: "U".into(), color: "#000".into(), role: Role::Editor, credential: Credential::Session(sha256_hex("s")), doc_frame: true, stream_pages: None };
        rooms.get("f").await.unwrap().join(info, out).await.unwrap();
        let init: Value = serde_json::from_str(&next(&mut rx).await.unwrap()).unwrap();
        assert_eq!(init["t"], "init");
        assert_eq!(init["docFrame"], "zlib");
        assert!(init.get("doc").is_none());
        let Some(Msg::Doc(json)) = next(&mut rx).await else { panic!("the document frame") };
        let doc = Document::from_json(&json).unwrap();
        assert!(!doc.pages().is_empty());
        // What the writer sends unpacks to the same JSON.
        let packed = crate::ws::zlib(json.as_bytes());
        assert!(packed.len() < json.len());
        let mut back = String::new();
        std::io::Read::read_to_string(&mut flate2::read::ZlibDecoder::new(&packed[..]), &mut back).unwrap();
        assert_eq!(back, &*json);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The next message on the socket, or None once the room closed it.
    async fn next(rx: &mut mpsc::Receiver<Msg>) -> Option<Msg> {
        tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("the room answered")
    }

    #[tokio::test]
    async fn rooms_close_sockets_whose_access_changed() {
        let (db, dir) = setup("rooms").await;
        let rooms = Rooms::new(db.clone());

        // A new role: closed without a message, so the client reconnects.
        let mut rx = join(&rooms, Role::Editor).await;
        sql(&db, "UPDATE workspace_members SET role = 'viewer'").await;
        rooms.recheck_all();
        assert!(next(&mut rx).await.is_none());

        // Signed out: told why, then closed.
        let mut rx = join(&rooms, Role::Viewer).await;
        sql(&db, "DELETE FROM sessions").await;
        rooms.recheck_all();
        let bye = next(&mut rx).await.unwrap();
        assert!(bye.contains(r#""fatal":true"#) && bye.contains("signed out"), "{bye}");
        assert!(next(&mut rx).await.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod measure_tests {
    use super::*;
    use flexma_doc::{Node, NodeType};

    /// Agent edits leave text boxes as estimates; the room measures them with
    /// the workspace's fonts before results are reported.
    #[test]
    fn agent_text_gets_measured() {
        let mut doc = Document::new_empty();
        let mut t = Node::new("t:1", NodeType::Text);
        t.parent = Some("0:1".into());
        (t.characters, t.font_size, t.width, t.height, t.needs_measure) = ("AVATAR Toyota Wave. Tuesday, LTA yield 7.4%".into(), 14.0, 400.0, 17.0, true);
        let mut missing = t.clone();
        (missing.id, missing.font_family) = ("t:2".into(), "No Such Font".into());
        let mut tx = Tx::new(&mut doc);
        tx.measurer = Some(TextMeasurer::new(crate::render::base_fontdb()));
        tx.apply_all(vec![Op::Create { node: t }, Op::Create { node: missing }]);
        tx.relayout();
        let n = doc.get("t:1").unwrap();
        // 308.92 px in Chrome (see render::measure_tests).
        assert_eq!((n.width, n.needs_measure), (309.0, false));
        let m = doc.get("t:2").unwrap();
        assert!(m.needs_measure, "a browser still measures text in a font the server lacks");
    }
}
