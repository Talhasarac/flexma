//! Paint layers over REST: a file's paint state (tile snapshots and the
//! strokes on top of them) and tile snapshot images, for members and
//! share-link guests. Strokes themselves travel over the file's socket.

use crate::api_share::resolve;
use crate::auth::{file_access, AuthUser, Role};
use crate::error::{AppError, AppResult};
use crate::paint::{self, Kind};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// Most a tile snapshot PNG may weigh.
const MAX_TILE_BYTES: usize = 4 * 1024 * 1024;

async fn state_of(st: &AppState, file_id: String) -> AppResult<Value> {
    let room = st.rooms.get(&file_id).await?;
    let layers = room.read(|doc, _| paint::layers_of(doc)).await?;
    st.db.call(move |c| paint::state(c, &file_id, &layers)).await
}

pub async fn state(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let fid = id.clone();
    st.db.call(move |c| file_access(c, &user.id, &fid, Role::Viewer).map(|_| ())).await?;
    Ok(Json(state_of(&st, id).await?))
}

pub async fn share_state(State(st): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let (file_id, _) = st.db.call(move |c| resolve(c, &token)).await?;
    Ok(Json(state_of(&st, file_id).await?))
}

#[derive(Deserialize)]
pub struct TileQuery {
    kind: Option<String>,
    seq: Option<i64>,
    /// Cache buster (the snapshot's seq): makes the response cacheable for good.
    v: Option<String>,
}

fn kind_of(q: &TileQuery) -> AppResult<Kind> {
    Kind::parse(q.kind.as_deref().unwrap_or("view")).ok_or_else(|| AppError::bad_request("kind must be base or view"))
}

async fn tile_response(st: &AppState, file_id: &str, layer: &str, tx: i32, ty: i32, q: &TileQuery) -> AppResult<Response> {
    let kind = kind_of(q)?;
    let mut path = paint::tile_path(&st.cfg.data_dir, file_id, layer, tx, ty, kind);
    if kind == Kind::View && !path.exists() {
        // No newer look than the base snapshot.
        path = paint::tile_path(&st.cfg.data_dir, file_id, layer, tx, ty, Kind::Base);
    }
    let bytes = tokio::fs::read(&path).await.map_err(|_| AppError::not_found("tile"))?;
    let cache = if q.v.is_some() { "private, max-age=31536000, immutable" } else { "no-cache" };
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, cache)], bytes).into_response())
}

pub async fn tile(
    State(st): State<AppState>,
    user: AuthUser,
    Path((id, layer, tx, ty)): Path<(String, String, i32, i32)>,
    Query(q): Query<TileQuery>,
) -> AppResult<Response> {
    let fid = id.clone();
    st.db.call(move |c| file_access(c, &user.id, &fid, Role::Viewer).map(|_| ())).await?;
    tile_response(&st, &id, &layer, tx, ty, &q).await
}

pub async fn share_tile(
    State(st): State<AppState>,
    Path((token, layer, tx, ty)): Path<(String, String, i32, i32)>,
    Query(q): Query<TileQuery>,
) -> AppResult<Response> {
    let (file_id, _) = st.db.call(move |c| resolve(c, &token)).await?;
    tile_response(&st, &file_id, &layer, tx, ty, &q).await
}

/// A browser uploads a tile snapshot: `kind=base` (what strokes replay onto)
/// or `kind=view` (the latest look, for exports), covering strokes up to `seq`.
pub async fn put_tile(
    State(st): State<AppState>,
    user: AuthUser,
    Path((id, layer, tx, ty)): Path<(String, String, i32, i32)>,
    Query(q): Query<TileQuery>,
    body: Bytes,
) -> AppResult<Json<Value>> {
    let kind = kind_of(&q)?;
    let seq = q.seq.ok_or_else(|| AppError::bad_request("seq is required"))?;
    if body.len() > MAX_TILE_BYTES || !body.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Err(AppError::bad_request("a tile must be a PNG of at most 4 MB"));
    }
    let size = imagesize::blob_size(&body).map_err(|_| AppError::bad_request("unreadable PNG"))?;
    let t = flexma_doc::raster::TILE as usize;
    if size.width != t || size.height != t || tx < 0 || ty < 0 {
        return Err(AppError::bad_request(format!("a tile must be {t}x{t} px")));
    }
    let fid = id.clone();
    st.db.call(move |c| file_access(c, &user.id, &fid, Role::Editor).map(|_| ())).await?;
    let room = st.rooms.get(&id).await?;
    let l2 = layer.clone();
    let is_layer = room.read(move |doc, _| doc.get(&l2).is_some_and(|n| n.kind == flexma_doc::NodeType::Paint)).await?;
    if !is_layer {
        return Err(AppError::not_found("paint layer"));
    }
    let (data_dir, fid, l2) = (st.cfg.data_dir.clone(), id.clone(), layer.clone());
    let latest = i64::try_from(room.paint_seq()).unwrap_or(i64::MAX);
    let kept = st.db.call(move |c| paint::save_tile(c, &data_dir, &fid, &l2, tx, ty, kind, seq, latest, &body)).await?;
    if kept {
        let k = if kind == Kind::Base { "base" } else { "view" };
        room.broadcast(json!({ "t": "tile", "layer": layer, "x": tx, "y": ty, "kind": k, "seq": seq })).await;
    }
    Ok(Json(json!({ "kept": kept })))
}
