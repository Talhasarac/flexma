//! Multiplayer WebSocket endpoint: `/api/files/:id/ws`.

use crate::auth::{authenticate, bearer_token, check_origin, cookie_value, file_access, refuse_oauth, Role, SESSION_COOKIE};
use crate::db::sha256_hex;
use crate::error::{AppError, AppResult};
use crate::rooms::{Credential, JoinInfo, Msg};
use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use flexma_doc::Op;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
enum ClientMsg {
    Ops {
        batch: u64,
        ops: Vec<serde_json::Value>,
    },
    Presence {
        #[serde(default)]
        cursor: Option<[f64; 2]>,
        #[serde(default)]
        selection: Vec<String>,
        #[serde(default)]
        page: Option<String>,
        #[serde(default)]
        viewport: Option<[f64; 4]>,
        #[serde(default)]
        following: Option<String>,
    },
    Ping,
}

#[derive(Deserialize)]
pub struct SocketQuery {
    /// `zlib`: send the document as its own compressed frame after `init`.
    #[serde(default)]
    doc: Option<String>,
    /// `stream`: send it page by page, `page` first.
    #[serde(default)]
    pages: Option<String>,
    #[serde(default)]
    page: Option<String>,
    /// The editor's build (`rooms::web_build`); "" from a dev server.
    #[serde(default)]
    build: Option<String>,
}

/// Which editor build a socket comes from, against the one being served.
/// Two builds can disagree on automatic fixes (text measuring) and correct
/// each other forever, so only the served one edits.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Build {
    /// The one being served, or nothing to compare (no web app, a dev server).
    Current,
    /// Another one: it delivers its edits and reloads, or is stopped.
    Other,
    /// From before editors sent their build: stopped at once.
    Unknown,
}

impl Build {
    /// `sent` by an editor, against the `served` one.
    fn of(served: &str, sent: Option<&str>) -> Build {
        match sent {
            _ if served.is_empty() => Build::Current,
            None => Build::Unknown,
            Some(b) if b.is_empty() || b == served => Build::Current,
            Some(_) => Build::Other,
        }
    }
}

/// What an editor from another build gets before its socket closes.
const OUTDATED: &str = r#"{"t":"error","error":"Flexma was updated. Reload the page to keep editing.","fatal":true}"#;

/// How long an editor from another build may stay to deliver its edits.
const OUTDATED_GRACE: Duration = Duration::from_secs(15);

impl SocketQuery {
    fn build(&self) -> Build {
        Build::of(&crate::rooms::web_build(), self.build.as_deref())
    }

    fn doc_frame(&self) -> bool {
        self.doc.as_deref() == Some("zlib")
    }

    fn stream_pages(&self) -> Option<Option<String>> {
        (self.doc_frame() && self.pages.as_deref() == Some("stream")).then(|| self.page.clone().filter(|p| p.len() <= 64))
    }
}

/// zlib-compress a document for its binary frame (JSON shrinks about 10×).
pub(crate) fn zlib(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 8), flate2::Compression::new(6));
    let _ = z.write_all(data);
    z.finish().unwrap_or_default()
}

pub async fn handler(
    State(st): State<AppState>,
    Path(file_id): Path<String>,
    Query(q): Query<SocketQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> AppResult<Response> {
    let user = authenticate(&st.db, &headers).await?.ok_or_else(AppError::unauthorized)?;
    refuse_oauth(&user)?;
    // The handshake is a GET, but the socket can edit: a page on another site
    // must not open one with the signed-in person's cookie.
    if !user.via_token {
        check_origin(&headers)?;
    }
    // The room re-checks this later, so revoking it ends the socket.
    let credential = match (user.via_token, bearer_token(&headers), cookie_value(&headers, SESSION_COOKIE)) {
        (true, Some(t), _) => Credential::Token(sha256_hex(&t)),
        (false, _, Some(s)) => Credential::Session(sha256_hex(&s)),
        _ => return Err(AppError::unauthorized()),
    };
    let uid = user.id.clone();
    let fid = file_id.clone();
    let (_, _, role) = st.db.call(move |c| file_access(c, &uid, &fid, Role::Viewer)).await?;
    let room = st.rooms.get(&file_id).await?;
    let build = q.build();
    Ok(ws.max_message_size(64 * 1024 * 1024).on_upgrade(move |socket| async move {
        let info = JoinInfo { user_id: user.id, name: user.name, color: user.color, role, credential, doc_frame: q.doc_frame(), stream_pages: q.stream_pages() };
        run(socket, room, info, build).await;
    }))
}

/// Anonymous, view-only socket for a public share link.
pub async fn share_handler(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Query(q): Query<SocketQuery>,
    ws: WebSocketUpgrade,
) -> AppResult<Response> {
    let t = token.clone();
    let (file_id, _) = st.db.call(move |c| crate::api_share::resolve(c, &t)).await?;
    let room = st.rooms.get(&file_id).await?;
    let build = q.build();
    Ok(ws.max_message_size(1024 * 1024).on_upgrade(move |socket| async move {
        let info = JoinInfo {
            user_id: format!("guest:{}", crate::db::random_string(6)),
            name: "Guest".into(),
            color: "#8a8a8f".into(),
            role: Role::Viewer,
            credential: Credential::Share(token),
            doc_frame: q.doc_frame(),
            stream_pages: q.stream_pages(),
        };
        run(socket, room, info, build).await;
    }))
}

pub(crate) async fn run(socket: WebSocket, room: crate::rooms::RoomHandle, info: JoinInfo, build: Build) {
    let (mut sink, mut stream) = socket.split();
    if build == Build::Unknown {
        let _ = sink.send(Message::Text(OUTDATED.into())).await;
        let _ = sink.close().await;
        return;
    }
    let (out_tx, mut out_rx) = mpsc::channel::<Msg>(crate::rooms::CLIENT_QUEUE);
    let own = out_tx.clone();
    let client_id = match room.join(info, out_tx).await {
        Ok(id) => id,
        Err(e) => {
            let _ = sink
                .send(Message::Text(format!(r#"{{"t":"error","error":"{}"}}"#, e.message).into()))
                .await;
            return;
        }
    };

    // Writer: room -> socket, plus keepalive pings.
    let mut writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(25));
        ping.tick().await;
        loop {
            tokio::select! {
                m = out_rx.recv() => match m {
                    Some(Msg::Text(t)) => {
                        if sink.send(Message::Text(t.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Msg::Doc(json)) => {
                        // Tens of milliseconds for a big file: not on the async workers.
                        let Ok(bytes) = tokio::task::spawn_blocking(move || zlib(json.as_bytes())).await else { break };
                        if sink.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                },
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = sink.close().await;
    });

    // Reader: socket -> room. Another build gets a while to deliver its edits.
    let deadline = (build == Build::Other).then(|| tokio::time::Instant::now() + OUTDATED_GRACE);
    let mut outdated = false;
    loop {
        let next = match deadline {
            Some(at) => tokio::select! {
                m = stream.next() => m,
                _ = tokio::time::sleep_until(at) => {
                    outdated = true;
                    break;
                }
            },
            None => stream.next().await,
        };
        let Some(Ok(m)) = next else { break };
        let text = match m {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        // Paint-layer messages (`strokeLive`, `stroke`, `strokeUndo`) go to the
        // room as they are; it validates them.
        if text.contains(r#""t":"stroke"#) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if v.get("t").and_then(|t| t.as_str()).is_some_and(|t| t.starts_with("stroke")) {
                    room.paint(client_id.clone(), v).await;
                    continue;
                }
            }
        }
        match serde_json::from_str::<ClientMsg>(&text) {
            Ok(ClientMsg::Ops { batch, ops }) => {
                // Parse ops individually so one malformed op cannot block the ack.
                let ops: Vec<Op> = ops
                    .into_iter()
                    .filter_map(|v| match serde_json::from_value::<Op>(v) {
                        Ok(op) => Some(op),
                        Err(e) => {
                            tracing::debug!("dropping malformed op: {e}");
                            None
                        }
                    })
                    .collect();
                room.client_ops(client_id.clone(), batch, ops).await
            }
            Ok(ClientMsg::Presence { cursor, selection, page, viewport, following }) => {
                let mut selection = selection;
                selection.truncate(500);
                let viewport = viewport.filter(|v| v[2] > 0.0 && v[3] > 0.0);
                let following = following.filter(|u| u.len() <= 64);
                room.presence(client_id.clone(), crate::rooms::PresenceUpdate { cursor, selection, page, viewport, following }).await
            }
            Ok(ClientMsg::Ping) => {}
            Err(e) => tracing::debug!("bad client message: {e}"),
        }
        if writer.is_finished() {
            break;
        }
    }
    room.leave(client_id);
    if outdated {
        let _ = own.send(Msg::Text(OUTDATED.into())).await;
        drop(own);
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut writer).await;
    }
    writer.abort();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_served_build_edits() {
        let served = "index-B.js";
        assert!(Build::of(served, Some("index-B.js")) == Build::Current);
        assert!(Build::of(served, Some("index-A.js")) == Build::Other);
        assert!(Build::of(served, None) == Build::Unknown, "an editor from before builds were sent");
        assert!(Build::of(served, Some("")) == Build::Current, "a dev server's editor");
        assert!(Build::of("", None) == Build::Current, "no web app to compare with");
    }
}
