//! Multiplayer WebSocket endpoint: `/api/files/:id/ws`.

use crate::auth::{authenticate, file_access, Role};
use crate::error::{AppError, AppResult};
use crate::rooms::{JoinInfo, Msg};
use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
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
    },
    Ping,
}

pub async fn handler(
    State(st): State<AppState>,
    Path(file_id): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> AppResult<Response> {
    let user = authenticate(&st.db, &headers).await?.ok_or_else(AppError::unauthorized)?;
    let uid = user.id.clone();
    let fid = file_id.clone();
    let (_, _, role) = st.db.call(move |c| file_access(c, &uid, &fid, Role::Viewer)).await?;
    let room = st.rooms.get(&file_id).await?;
    Ok(ws.max_message_size(64 * 1024 * 1024).on_upgrade(move |socket| async move {
        let info = JoinInfo { user_id: user.id, name: user.name, color: user.color, role };
        run(socket, room, info).await;
    }))
}

async fn run(socket: WebSocket, room: crate::rooms::RoomHandle, info: JoinInfo) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Msg>(crate::rooms::CLIENT_QUEUE);
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
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(25));
        ping.tick().await;
        loop {
            tokio::select! {
                m = out_rx.recv() => match m {
                    Some(m) => {
                        if sink.send(Message::Text(m.to_string().into())).await.is_err() {
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

    // Reader: socket -> room.
    while let Some(Ok(m)) = stream.next().await {
        let text = match m {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
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
            Ok(ClientMsg::Presence { cursor, selection, page }) => {
                let mut selection = selection;
                selection.truncate(500);
                room.presence(client_id.clone(), cursor, selection, page).await
            }
            Ok(ClientMsg::Ping) => {}
            Err(e) => tracing::debug!("bad client message: {e}"),
        }
        if writer.is_finished() {
            break;
        }
    }
    room.leave(client_id);
    writer.abort();
}
