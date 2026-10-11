use crate::config::Config;
use crate::db::Db;
use crate::rooms::Rooms;
use axum::http::{header, HeaderMap};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: Db,
    pub rooms: Arc<Rooms>,
}

impl AppState {
    /// Public base URL: configured, or derived from the request.
    pub fn base_url(&self, headers: &HeaderMap) -> String {
        if let Some(u) = &self.cfg.public_url {
            return u.clone();
        }
        let host = headers
            .get("x-forwarded-host")
            .or_else(|| headers.get(header::HOST))
            .and_then(|v| v.to_str().ok())
            .unwrap_or("localhost:8080");
        let proto = headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("http");
        format!("{proto}://{host}")
    }
}
