//! Flexma server: a self-hosted collaborative design tool.
//!
//! Serves the web app, the JSON API (`/api`), real-time multiplayer sockets and
//! an MCP endpoint (`/mcp`) so Claude Code, Codex and other agents can design
//! alongside people.

mod api_admin;
mod api_auth;
mod api_comments;
mod api_files;
mod api_fonts;
mod api_paint;
mod api_share;
mod api_tokens;
mod api_versions;
mod api_workspaces;
mod auth;
mod backup;
mod config;
mod db;
mod doc_store;
mod error;
mod fonts;
mod icons;
mod mcp;
mod paint;
mod oauth;
mod render;
mod rooms;
mod state;
mod svg_import;
mod unity;
mod uploads;
mod ws;

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{header, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::Router;
use std::sync::Arc;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

fn usage() -> ! {
    eprintln!(
        "flexma {}\n\nUSAGE:\n  flexma                       run the server\n  flexma reset-password EMAIL  set a new password for a user (prompts on stdin)\n  flexma restore FILE.zip      replace the data with a backup from the Admin page (stop the server first)\n\nENVIRONMENT:\n  FLEXMA_ADDR            listen address (default 0.0.0.0:8080)\n  FLEXMA_DATA_DIR        database + uploads (default ./data)\n  FLEXMA_WEB_DIR         built web app (default ./web/dist)\n  FLEXMA_PUBLIC_URL      public base URL, e.g. https://design.example.com\n  FLEXMA_ALLOW_SIGNUP    allow open sign-up (default false: invite-only after the first admin)\n  FLEXMA_INSTANCE_NAME   name shown on the login page\n  FLEXMA_MAX_UPLOAD_MB   image upload limit (default 25)\n  FLEXMA_FONTS_DIR       extra fonts for server-side PNG rendering",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(2)
}

/// File in the data directory holding the first-run setup token.
pub const SETUP_TOKEN_FILE: &str = "setup-token";

/// Write a file only the server's user can read.
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)?.write_all(format!("{contents}\n").as_bytes())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "flexma=info,tower_http=warn".into()),
        )
        // No color codes when logging to journald or a file.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();

    let cfg = config::Config::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Restore before opening the database it replaces.
    if args.first().map(String::as_str) == Some("restore") {
        let zip = args.get(1).cloned().unwrap_or_else(|| usage());
        match backup::restore(&cfg.data_dir, std::path::Path::new(&zip), &cfg.addr) {
            Ok(msg) => {
                println!("{msg}");
                return;
            }
            Err(e) => {
                eprintln!("restore failed: {e}");
                std::process::exit(1);
            }
        }
    }
    std::fs::create_dir_all(cfg.data_dir.join("assets")).expect("cannot create data directory");
    render::set_data_dir(&cfg.data_dir);
    let db = db::Db::open(&cfg.data_dir.join("flexma.db")).expect("cannot open database");

    match args.first().map(String::as_str) {
        None => {}
        Some("reset-password") => {
            let email = args.get(1).cloned().unwrap_or_else(|| usage());
            reset_password(&db, &email).await;
            return;
        }
        Some(_) => usage(),
    }

    db.call(|c| Ok(api_tokens::record_skill_versions(c)?)).await.expect("cannot record skill versions");
    let mut cfg = cfg;
    let users: i64 = db
        .call(|c| Ok(c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?))
        .await
        .expect("cannot read users");
    if users == 0 {
        // Logs can rotate away quickly on small servers, so the token is also
        // kept in a file in the data directory (stable across restarts, and
        // deleted once the first account exists).
        let path = cfg.data_dir.join(SETUP_TOKEN_FILE);
        let saved = std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string()).filter(|s| s.len() >= 16);
        let token = cfg.setup_token.clone().or(saved).unwrap_or_else(|| db::random_string(24));
        if let Err(e) = write_private(&path, &token) {
            tracing::warn!("could not save the setup token to {}: {e}", path.display());
        }
        let base = cfg.public_url.clone().unwrap_or_else(|| format!("http://{}", cfg.addr.replace("0.0.0.0", "localhost")));
        tracing::warn!("no accounts yet; create the admin account at {base}/setup?token={token} (token also saved in {})", path.display());
        cfg.setup_token = Some(token);
    }

    let rooms = rooms::Rooms::new(db.clone());
    let state = state::AppState { cfg: Arc::new(cfg.clone()), db, rooms: rooms.clone() };
    tokio::spawn(api_files::backfill_image_hashes(state.clone()));

    let api = Router::new()
        .route("/setup", get(api_auth::setup))
        .route("/auth/signup", post(api_auth::signup))
        .route("/auth/login", post(api_auth::login))
        .route("/auth/logout", post(api_auth::logout))
        .route("/me", get(api_auth::me).patch(api_auth::update_me))
        .route("/workspaces", get(api_workspaces::list).post(api_workspaces::create))
        .route(
            "/workspaces/{id}",
            get(api_workspaces::get).patch(api_workspaces::update).delete(api_workspaces::delete),
        )
        .route("/workspaces/{id}/members", get(api_workspaces::members))
        .route(
            "/workspaces/{id}/members/{user}",
            patch(api_workspaces::update_member).delete(api_workspaces::remove_member),
        )
        .route(
            "/workspaces/{id}/invites",
            get(api_workspaces::list_invites).post(api_workspaces::create_invite),
        )
        .route("/invites/{id}", delete(api_workspaces::delete_invite))
        .route("/invites/by-token/{token}", get(api_workspaces::invite_info))
        .route("/invites/by-token/{token}/accept", post(api_workspaces::accept_invite))
        .route(
            "/workspaces/{id}/projects",
            get(api_workspaces::list_projects).post(api_workspaces::create_project),
        )
        .route("/workspaces/{id}/files", get(api_files::list_workspace_files))
        .route(
            "/projects/{id}",
            patch(api_workspaces::update_project).delete(api_workspaces::delete_project),
        )
        .route("/projects/{id}/files", get(api_files::list_project_files).post(api_files::create_file))
        .route(
            "/files/{id}",
            get(api_files::get_file).patch(api_files::update_file).delete(api_files::delete_file),
        )
        .route("/files/{id}/duplicate", post(api_files::duplicate_file))
        .route("/files/{id}/document", get(api_files::get_document))
        .route("/files/{id}/thumbnail.png", get(api_files::thumbnail))
        .route("/files/{id}/export.svg", get(api_files::export_svg))
        .route("/files/{id}/export.png", get(api_files::export_png))
        .route("/files/{id}/export.unity", get(api_files::export_unity))
        .route("/files/{id}/assets", post(api_files::upload_asset))
        .route("/files/{id}/ws", get(ws::handler))
        .route("/files/{id}/versions", get(api_versions::list).post(api_versions::create))
        .route("/files/{id}/comments", get(api_comments::list).post(api_comments::create))
        .route("/files/{id}/share", get(api_share::get).put(api_share::update))
        .route("/share/{token}", get(api_share::info))
        .route("/share/{token}/ws", get(ws::share_handler))
        .route("/share/{token}/assets/{aid}", get(api_share::asset))
        .route("/share/{token}/export.svg", get(api_share::export_svg))
        .route("/share/{token}/export.png", get(api_share::export_png))
        .route("/share/{token}/thumbnail.png", get(api_share::thumbnail))
        .route("/files/{id}/paint", get(api_paint::state))
        .route("/files/{id}/paint/{layer}/{tx}/{ty}", get(api_paint::tile).put(api_paint::put_tile))
        .route("/share/{token}/paint", get(api_paint::share_state))
        .route("/share/{token}/paint/{layer}/{tx}/{ty}", get(api_paint::share_tile))
        .route("/share/{token}/fonts", get(api_fonts::share_list))
        .route("/share/{token}/fonts/{fid}", get(api_fonts::share_file))
        .route("/workspaces/{id}/fonts", get(api_fonts::list).post(api_fonts::upload).delete(api_fonts::delete_family))
        .route("/workspaces/{id}/fonts/google", post(api_fonts::add_google))
        .route("/font-uploads/{token}", post(api_fonts::upload_with_link))
        .route("/image-uploads/{token}", post(api_files::upload_image_with_link))
        .route("/node-uploads/{token}", post(mcp::upload_nodes_with_link))
        .route("/png-exports/{token}", get(api_files::export_png_with_link))
        .route("/unity-exports/{token}", get(api_files::export_unity_with_link))
        .route("/fonts/{id}", get(api_fonts::file))
        .route("/google-fonts", get(api_fonts::google_catalogue))
        .route("/google-fonts/preview", get(api_fonts::google_preview))
        .route("/comments/{id}", patch(api_comments::update).delete(api_comments::delete))
        .route("/files/{id}/versions/{vid}/preview.png", get(api_versions::preview))
        .route("/files/{id}/versions/{vid}/restore", post(api_versions::restore))
        .route("/assets/{id}", get(api_files::get_asset))
        .route("/tokens", get(api_tokens::list).post(api_tokens::create))
        .route("/tokens/{id}", delete(api_tokens::delete))
        .route("/agent-setup", get(api_tokens::agent_setup))
        .route("/skills", get(api_tokens::skills))
        .route("/skills/seen", post(api_tokens::skills_seen))
        .route("/admin/users", get(api_admin::list_users))
        .route("/admin/users/{id}", patch(api_admin::update_user).delete(api_admin::delete_user))
        .route("/admin/users/{id}/reset-password", post(api_admin::reset_password))
        .route("/admin/settings", get(api_admin::get_settings).patch(api_admin::update_settings))
        .route("/admin/backup", get(api_admin::backup))
        .route("/admin/backup/info", get(api_admin::backup_info))
        .route("/health", get(|| async { "ok" }))
        .fallback(|| async { error::AppError::not_found("endpoint") })
        .layer(DefaultBodyLimit::max(cfg.max_upload_bytes + 1024 * 1024));

    let index = cfg.web_dir.join("index.html");
    rooms::set_web_index(&index);
    let app = Router::new()
        .nest("/api", api)
        .route("/mcp", post(mcp::handle).get(mcp::get_not_allowed).delete(mcp::get_not_allowed))
        .route("/.well-known/oauth-protected-resource", get(oauth::protected_resource))
        .route("/.well-known/oauth-protected-resource/mcp", get(oauth::protected_resource))
        .route("/.well-known/oauth-authorization-server", get(oauth::authorization_server))
        .route("/.well-known/oauth-authorization-server/mcp", get(oauth::authorization_server))
        .route("/.well-known/openid-configuration", get(oauth::authorization_server))
        .route("/oauth/register", post(oauth::register))
        .route("/oauth/authorize", get(oauth::authorize_get).post(oauth::authorize_post))
        .route("/oauth/token", post(oauth::token))
        .route("/oauth/revoke", post(oauth::revoke))
        .route("/agents/install.sh", get(api_tokens::install_script))
        .route("/agents/install.ps1", get(api_tokens::install_script_ps1))
        .route("/agents/install/{file}", get(api_tokens::add_on_installer))
        .route("/agents/skills/{skill}/{*path}", get(api_tokens::skill_file));

    let app = if index.exists() {
        app.fallback_service(ServeDir::new(&cfg.web_dir).fallback(ServeFile::new(&index)))
    } else {
        tracing::warn!("web app not found at {} — build it with `npm run build` in web/", cfg.web_dir.display());
        app.fallback(|| async {
            Html("<h1>Flexma server is running</h1><p>The web app is not built. Run <code>npm install && npm run build</code> in <code>web/</code>, or set FLEXMA_WEB_DIR.</p>")
        })
    };

    let app = app
        .layer(middleware::from_fn(cache_headers))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cfg.addr).await.expect("cannot bind address");
    tracing::info!("Flexma listening on http://{}", cfg.addr);
    let stopping = Arc::new(tokio::sync::Notify::new());
    let stop = stopping.clone();
    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
        stop_signal().await;
        tracing::info!("shutting down");
        stop.notify_one();
    });
    tokio::select! {
        r = serve => r.expect("server error"),
        // Open WebSockets (editors, the tunnel) would keep the graceful drain
        // waiting until systemd kills the process, before files are saved.
        _ = async {
            stopping.notified().await;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        } => tracing::info!("closing open connections"),
    }
    if rooms.shutdown_all().await {
        tracing::info!("files saved");
    } else {
        tracing::error!("some files weren't saved before exiting");
    }
    // Don't wait for a render still running on the blocking pool.
    std::process::exit(0);
}

/// Ctrl-C, or SIGTERM (what `systemctl stop` and `docker stop` send).
async fn stop_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

/// Long-lived caching for Vite's content-hashed bundles; never cache the HTML shell.
async fn cache_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    if path.starts_with("/assets/") {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    } else if !path.starts_with("/api/") && !path.starts_with("/mcp") && !path.starts_with("/oauth") && !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    res.into_response()
}

async fn reset_password(db: &db::Db, email: &str) {
    use std::io::BufRead;
    eprint!("New password for {email}: ");
    let mut pw = String::new();
    std::io::stdin().lock().read_line(&mut pw).expect("read password");
    let pw = pw.trim().to_string();
    if pw.len() < 8 {
        eprintln!("password must be at least 8 characters");
        std::process::exit(1);
    }
    let hash = auth::hash_password(&pw).expect("hash");
    let email = email.to_lowercase();
    let found = db
        .call(move |c| {
            let tx = c.transaction()?;
            let id: Option<String> = rusqlite::OptionalExtension::optional(tx.query_row(
                "SELECT id FROM users WHERE email = ?1",
                rusqlite::params![email],
                |r| r.get(0),
            ))?;
            let Some(id) = id else { return Ok(false) };
            tx.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", rusqlite::params![hash, id])?;
            auth::sign_out_everywhere(&tx, &id, None)?;
            tx.commit()?;
            Ok(true)
        })
        .await
        .expect("db");
    if !found {
        eprintln!("no user with that email");
        std::process::exit(1);
    }
    eprintln!("password updated; their sessions and API tokens were signed out");
}
