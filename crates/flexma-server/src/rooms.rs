//! Real-time document rooms. Each open file is owned by one actor task that holds
//! the authoritative in-memory `Document`, orders incoming ops, rebroadcasts them
//! (the echo doubles as the sender's ack), tracks presence and periodically
//! snapshots to SQLite. No locks around documents; everything goes through the
//! actor's mailbox. Agents (MCP) edit through the same pipeline.

use crate::auth::Role;
use crate::db::{now, random_string, Db};
use crate::error::{AppError, AppResult};
use flexma_doc::{components, layout};
use flexma_doc::{Document, NodeId, Op};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

pub type Msg = Arc<str>;

/// Bounded per-client outbound queue. A client that falls this far behind is
/// disconnected and will resync from a fresh snapshot on reconnect.
pub const CLIENT_QUEUE: usize = 2048;
const SAVE_EVERY: Duration = Duration::from_secs(2);
const IDLE_CLOSE_SECS: i64 = 90;
const AGENT_PRESENCE_TTL: Duration = Duration::from_secs(12);
const AUTO_VERSION_SECS: i64 = 15 * 60;

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
    pub is_agent: bool,
    /// Anonymous viewer via a public share link.
    pub is_guest: bool,
}

pub struct JoinInfo {
    pub user_id: String,
    pub name: String,
    pub color: String,
    pub role: Role,
    /// Set for anonymous viewers who came in through a public share link.
    pub share_token: Option<String>,
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
}

impl<'a> Tx<'a> {
    fn new(doc: &'a mut Document) -> Self {
        Tx { doc, applied: vec![], changed: vec![], removed: vec![], touched: vec![], undo: vec![] }
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

    /// Re-expand affected instances and run auto layout (so results can report final geometry).
    pub fn relayout(&mut self) {
        let synced = components::sync_instances(self.doc, &self.changed, &self.removed);
        let mut changed = std::mem::take(&mut self.changed);
        changed.extend(synced);
        let styled = flexma_doc::styles::sync_styles(self.doc, &changed);
        changed.extend(styled);
        self.removed.clear();
        layout::relayout(self.doc, &changed);
        self.changed = changed;
    }
}

type ReadFn = Box<dyn FnOnce(&Document, &[Peer]) + Send>;
type MutateFn = Box<dyn FnOnce(&mut Tx) -> Result<Value, String> + Send>;

enum Cmd {
    Join { info: JoinInfo, out: mpsc::Sender<Msg>, reply: oneshot::Sender<String> },
    Leave { client_id: String },
    ClientOps { client_id: String, batch: u64, ops: Vec<Op> },
    Presence { client_id: String, cursor: Option<[f64; 2]>, selection: Vec<String>, page: Option<String> },
    Mutate { agent: AgentInfo, f: MutateFn, reply: oneshot::Sender<Result<Value, String>> },
    UndoAgent { agent: AgentInfo, steps: usize, reply: oneshot::Sender<Result<Value, String>> },
    Read { f: ReadFn },
    Flush { reply: oneshot::Sender<()> },
    /// Send a message to every connected client (e.g. "comments changed").
    Broadcast { msg: Msg },
    /// A paint-layer message from a client (`strokeLive`, `stroke`, `strokeUndo`).
    Paint { client_id: String, msg: Value },
    /// Disconnect guests who joined through a share link that was turned off.
    KickShare { token: String },
    /// Stop the actor. `discard` skips the final save (used when the stored
    /// document was replaced underneath, e.g. a version restore).
    Shutdown { notify: Option<String>, discard: bool },
}

struct Stats {
    clients: AtomicUsize,
    last_active: AtomicI64,
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

    pub async fn presence(&self, client_id: String, cursor: Option<[f64; 2]>, selection: Vec<String>, page: Option<String>) {
        let _ = self.tx.send(Cmd::Presence { client_id, cursor, selection, page }).await;
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

    /// Disconnect guests that joined through `token`.
    pub async fn kick_share(&self, token: String) {
        let _ = self.tx.send(Cmd::KickShare { token }).await;
    }

    /// Notify every connected client.
    pub async fn paint(&self, client_id: String, msg: Value) {
        let _ = self.tx.send(Cmd::Paint { client_id, msg }).await;
    }

    pub async fn broadcast(&self, msg: Value) {
        let _ = self.tx.send(Cmd::Broadcast { msg: Arc::from(msg.to_string()) }).await;
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

    pub async fn mutate<F>(&self, agent: AgentInfo, f: F) -> AppResult<Result<Value, String>>
    where
        F: FnOnce(&mut Tx) -> Result<Value, String> + Send + 'static,
    {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Cmd::Mutate { agent, f: Box::new(f), reply }).await.map_err(|_| closed())?;
        rx.await.map_err(|_| closed())
    }
}

impl Rooms {
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
}

impl Rooms {
    pub fn new(db: Db) -> Arc<Rooms> {
        let rooms = Arc::new(Rooms { db, rooms: Mutex::new(HashMap::new()), blocked: Mutex::new(Default::default()) });
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
            if let Some(h) = map.remove(&id) {
                let _ = h.tx.try_send(Cmd::Shutdown { notify: None, discard: false });
            }
        }
    }

    /// Close a room immediately (e.g. the file was deleted).
    pub fn close(&self, file_id: &str, notify: Option<String>) {
        if let Some(h) = self.rooms.lock().unwrap().remove(file_id) {
            let _ = h.tx.try_send(Cmd::Shutdown { notify, discard: false });
        }
    }

    /// Close a room without saving its in-memory state and wait for it to stop.
    pub async fn discard_and_wait(&self, file_id: &str, notify: Option<String>) {
        let h = self.rooms.lock().unwrap().remove(file_id);
        if let Some(h) = h {
            let _ = h.tx.send(Cmd::Shutdown { notify, discard: true }).await;
            let deadline = Instant::now() + Duration::from_secs(5);
            while !h.tx.is_closed() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
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

    pub async fn shutdown_all(&self) {
        let handles: Vec<RoomHandle> = self.rooms.lock().unwrap().drain().map(|(_, h)| h).collect();
        for h in handles {
            let _ = h.tx.send(Cmd::Shutdown { notify: None, discard: false }).await;
            // Wait for the actor to finish saving.
            let deadline = Instant::now() + Duration::from_secs(5);
            while !h.tx.is_closed() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }

    pub fn block(&self, file_id: &str) {
        self.blocked.lock().unwrap().insert(file_id.to_string());
    }

    pub fn unblock(&self, file_id: &str) {
        self.blocked.lock().unwrap().remove(file_id);
    }

    pub async fn get(&self, file_id: &str) -> AppResult<RoomHandle> {
        if self.blocked.lock().unwrap().contains(file_id) {
            return Err(closed());
        }
        if let Some(h) = self.rooms.lock().unwrap().get(file_id) {
            if !h.tx.is_closed() {
                return Ok(h.clone());
            }
        }
        let fid = file_id.to_string();
        let loaded: Option<(String, i64, u64)> = self
            .db
            .call(move |c| {
                let row: Option<(String, i64)> = c
                    .query_row("SELECT document, version FROM files WHERE id = ?1", params![fid], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                match row {
                    Some((doc, version)) => Ok(Some((doc, version, crate::paint::last_seq(c, &fid)?))),
                    None => Ok(None),
                }
            })
            .await?;
        let (json, version, paint_seq) = loaded.ok_or_else(|| AppError::not_found("file"))?;
        let mut doc = Document::from_json(&json).map_err(|e| AppError::internal(format!("corrupt document {file_id}: {e}")))?;
        components::sync_all(&mut doc);
        flexma_doc::styles::sync_all(&mut doc);
        layout::relayout_all(&mut doc);

        let mut map = self.rooms.lock().unwrap();
        if let Some(h) = map.get(file_id) {
            if !h.tx.is_closed() {
                return Ok(h.clone());
            }
        }
        let (tx, rx) = mpsc::channel(4096);
        let stats = Arc::new(Stats { clients: AtomicUsize::new(0), last_active: AtomicI64::new(now()) });
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
        };
        tokio::spawn(room.run(rx));
        let h = RoomHandle { tx, stats };
        map.insert(file_id.to_string(), h.clone());
        Ok(h)
    }
}

struct Client {
    out: mpsc::Sender<Msg>,
    peer: Peer,
    role: Role,
    share_token: Option<String>,
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
}

fn msg(v: Value) -> Msg {
    Arc::from(v.to_string())
}

impl Room {
    async fn run(mut self, mut rx: mpsc::Receiver<Cmd>) {
        let mut tick = tokio::time::interval(SAVE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Cmd::Shutdown { notify, discard }) => {
                        if discard {
                            self.dirty = false;
                        }
                        rx.close();
                        // Drain anything already queued so callers get answers.
                        while let Ok(cmd) = rx.try_recv() {
                            if !matches!(cmd, Cmd::Shutdown { .. }) {
                                self.handle(cmd);
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
            }
        }
        if self.dirty {
            self.save().await;
        }
        tracing::debug!("room {} closed", self.file_id);
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
                    is_agent: false,
                    is_guest: info.share_token.is_some(),
                };
                let init = json!({
                    "t": "init",
                    "clientId": client_id,
                    "version": self.version,
                    "role": info.role.as_str(),
                    "doc": self.doc.snapshot(),
                    "peers": self.peers(),
                });
                if out.try_send(msg(init)).is_err() {
                    return;
                }
                self.broadcast(msg(json!({ "t": "presence", "peer": peer })), None);
                self.clients.insert(client_id.clone(), Client { out, peer, role: info.role, share_token: info.share_token });
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
            Cmd::Presence { client_id, cursor, selection, page } => {
                let Some(c) = self.clients.get_mut(&client_id) else { return };
                c.peer.cursor = cursor;
                c.peer.selection = selection;
                c.peer.page = page;
                let m = msg(json!({ "t": "presence", "peer": c.peer }));
                self.broadcast(m, Some(&client_id));
            }
            Cmd::Mutate { agent, f, reply } => {
                let mut tx = Tx::new(&mut self.doc);
                let result = f(&mut tx);
                tx.relayout();
                tx.settle_undo();
                let Tx { applied, touched, undo, .. } = tx;
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
                let gone: Vec<String> =
                    self.clients.iter().filter(|(_, c)| c.share_token.as_deref() == Some(token.as_str())).map(|(id, _)| id.clone()).collect();
                let bye = msg(json!({ "t": "error", "error": "This share link is no longer active.", "fatal": true }));
                for id in gone {
                    if let Some(c) = self.clients.remove(&id) {
                        let _ = c.out.try_send(bye.clone());
                    }
                    self.broadcast(msg(json!({ "t": "leave", "clientId": id })), None);
                }
                self.stats.clients.store(self.clients.len(), Ordering::Relaxed);
            }
            Cmd::Shutdown { .. } | Cmd::Flush { .. } => {}
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
        let synced = components::sync_instances(&mut self.doc, &changed, &removed);
        changed.extend(synced);
        let styled = flexma_doc::styles::sync_styles(&mut self.doc, &changed);
        changed.extend(styled);
        layout::relayout(&mut self.doc, &changed);
        if !accepted.is_empty() {
            self.version += 1;
            self.dirty = true;
        }
        self.broadcast(
            msg(json!({ "t": "ops", "from": client_id, "batch": batch, "ops": accepted })),
            None,
        );
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
        let json = self.doc.to_json();
        let version = self.version as i64;
        let fid = self.file_id.clone();
        let res = self
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE files SET document = ?1, version = ?2, updated_at = ?3 WHERE id = ?4",
                    params![json, version, now(), fid],
                )?;
                // Automatic version history: keep a snapshot at most every 15 minutes of editing.
                let last: Option<i64> = c
                    .query_row(
                        "SELECT MAX(created_at) FROM file_versions WHERE file_id = ?1 AND label IS NULL",
                        params![fid],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                if last.map(|t| now() - t >= AUTO_VERSION_SECS).unwrap_or(true) {
                    c.execute(
                        "INSERT INTO file_versions (id, file_id, label, document, version, created_by, created_at)
                         VALUES (?1, ?2, NULL, ?3, ?4, NULL, ?5)",
                        params![crate::db::new_id(), fid, json, version, now()],
                    )?;
                    // Keep the newest 200 automatic versions.
                    c.execute(
                        "DELETE FROM file_versions WHERE file_id = ?1 AND label IS NULL AND id NOT IN
                         (SELECT id FROM file_versions WHERE file_id = ?1 AND label IS NULL ORDER BY created_at DESC LIMIT 200)",
                        params![fid],
                    )?;
                }
                Ok(())
            })
            .await;
        match res {
            Ok(()) => self.dirty = false,
            Err(e) => tracing::error!("saving {} failed: {}", self.file_id, e.message),
        }
    }
}
