//! Full-server backups: one .zip with a consistent copy of the database plus
//! uploaded images and fonts. Admins download it from the Admin page and put
//! it back with `flexma restore <file.zip>`.
//!
//! Layout inside the zip (under one top-level folder):
//!   flexma.db   the database (a single self-contained file, no WAL)
//!   assets/     uploaded images
//!   fonts/      workspace font files
//!   README.txt

use crate::db::random_string;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use rusqlite::params;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub const DB_FILE: &str = "flexma.db";
/// Folders in the data directory that belong in a backup. (`cache/` is
/// rebuilt on demand and left out.)
const DIRS: &[&str] = &["assets", "fonts"];

const README: &str = "Flexma backup
=============

Everything on the server: accounts, workspaces, every design with its version
history and comments, share links, uploaded images (assets/) and fonts
(fonts/).

It includes password hashes and login tokens, so keep it somewhere private.

To restore it on a server (this replaces the data there; the old data is kept
in a pre-restore-* folder next to it):

  systemctl stop flexma
  FLEXMA_DATA_DIR=/var/lib/flexma /opt/flexma/flexma restore /path/to/this.zip
  systemctl start flexma

With Docker, copy the zip into the data volume and run
  docker compose run --rm flexma flexma restore /data/<file>.zip
";

/// One backup at a time; each one reads the whole data directory.
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What a backup will contain, for the Admin page.
pub async fn info(st: &AppState) -> AppResult<serde_json::Value> {
    let db_bytes: i64 = st
        .db
        .call(|c| {
            let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            Ok(pages * size)
        })
        .await?;
    let data_dir = st.cfg.data_dir.clone();
    let dirs = tokio::task::spawn_blocking(move || DIRS.iter().map(|d| (*d, dir_stats(&data_dir.join(d)))).collect::<Vec<_>>())
        .await
        .map_err(AppError::internal)?;
    let mut out = serde_json::json!({ "databaseBytes": db_bytes });
    for (name, (files, bytes)) in dirs {
        out[format!("{name}Files")] = files.into();
        out[format!("{name}Bytes")] = bytes.into();
    }
    Ok(out)
}

fn dir_stats(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return (0, 0) };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .fold((0, 0), |(n, b), m| (n + 1, b + m.len()))
}

/// Build a backup. Returns an already-unlinked temporary file (so it
/// disappears once the download finishes), its size and a download name.
pub async fn create(st: &AppState) -> AppResult<(File, u64, String)> {
    let _one_at_a_time = RUNNING.lock().await;
    // Open documents save every couple of seconds; make it "now".
    st.rooms.flush_all().await;

    let data_dir = st.cfg.data_dir.clone();
    let tmp = data_dir.join("tmp");
    tokio::fs::create_dir_all(&tmp).await.map_err(AppError::internal)?;
    let id = random_string(10);
    let db_copy = tmp.join(format!("backup-{id}.db"));
    let zip_path = tmp.join(format!("backup-{id}.zip"));

    // A consistent snapshot without the WAL, while the server keeps running.
    let target = db_copy.to_string_lossy().to_string();
    st.db.call(move |c| Ok(c.execute("VACUUM INTO ?1", params![target])?)).await?;

    let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M").to_string();
    // e.g. flexma-backup-design.example.com-2026-10-11-1830.zip
    let host = st.cfg.public_url.as_deref().map(|u| {
        u.split("://")
            .nth(1)
            .unwrap_or(u)
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' })
            .collect::<String>()
    });
    let root = format!("flexma-backup-{stamp}");
    let filename = match host {
        Some(h) => format!("flexma-backup-{h}-{stamp}.zip"),
        None => format!("{root}.zip"),
    };

    let (zp, dbc) = (zip_path.clone(), db_copy.clone());
    let result = tokio::task::spawn_blocking(move || write_zip(&zp, &dbc, &data_dir, &root))
        .await
        .map_err(AppError::internal)?;
    let _ = tokio::fs::remove_file(&db_copy).await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&zip_path).await;
        return Err(AppError::internal(e));
    }
    let file = File::open(&zip_path).map_err(AppError::internal)?;
    let size = file.metadata().map_err(AppError::internal)?.len();
    // The open handle keeps the data readable; the name goes away now.
    let _ = std::fs::remove_file(&zip_path);
    Ok((file, size, filename))
}

fn write_zip(out: &Path, db: &Path, data_dir: &Path, root: &str) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;
    let err = |e: &dyn std::fmt::Display| e.to_string();
    let mut z = zip::ZipWriter::new(BufWriter::new(File::create(out).map_err(|e| err(&e))?));
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let when = zip::DateTime::from_date_and_time(
        now.year() as u16,
        now.month() as u8,
        now.day() as u8,
        now.hour() as u8,
        now.minute() as u8,
        now.second() as u8,
    )
    .unwrap_or_default();
    let base = SimpleFileOptions::default().large_file(true).last_modified_time(when);
    let deflate = base.compression_method(CompressionMethod::Deflated);
    // Images are already compressed.
    let stored = base.compression_method(CompressionMethod::Stored);

    z.start_file(format!("{root}/README.txt"), deflate).map_err(|e| err(&e))?;
    z.write_all(README.as_bytes()).map_err(|e| err(&e))?;
    z.start_file(format!("{root}/{DB_FILE}"), deflate).map_err(|e| err(&e))?;
    std::io::copy(&mut File::open(db).map_err(|e| err(&e))?, &mut z).map_err(|e| err(&e))?;
    for dir in DIRS {
        let Ok(entries) = std::fs::read_dir(data_dir.join(dir)) else { continue };
        let mut names: Vec<_> = entries.filter_map(Result::ok).filter(|e| e.path().is_file()).map(|e| e.file_name()).collect();
        names.sort();
        for name in names {
            let name = name.to_string_lossy();
            let opts = if *dir == "assets" { stored } else { deflate };
            z.start_file(format!("{root}/{dir}/{name}"), opts).map_err(|e| err(&e))?;
            let mut f = File::open(data_dir.join(dir).join(name.as_ref())).map_err(|e| err(&e))?;
            std::io::copy(&mut f, &mut z).map_err(|e| err(&e))?;
        }
    }
    z.finish().map_err(|e| err(&e))?.flush().map_err(|e| err(&e))?;
    Ok(())
}

/// `flexma restore <file.zip>`: replace the data directory's database, images
/// and fonts with a backup. The current ones are moved to `pre-restore-*`.
pub fn restore(data_dir: &Path, zip_path: &Path, addr: &str) -> Result<String, String> {
    // Refuse while the server is running: it would keep writing the old database.
    let probe = addr.replace("0.0.0.0", "127.0.0.1");
    if std::net::TcpStream::connect(&probe).is_ok() {
        return Err(format!("Flexma is still running on {addr}. Stop it first (e.g. systemctl stop flexma), then run restore again."));
    }
    let file = File::open(zip_path).map_err(|e| format!("can't open {}: {e}", zip_path.display()))?;
    let mut z = zip::ZipArchive::new(file).map_err(|e| format!("not a zip file: {e}"))?;

    // Find the top-level folder holding flexma.db.
    let root = (0..z.len())
        .filter_map(|i| z.by_index(i).ok().and_then(|f| f.enclosed_name()))
        .find(|p| p.file_name().map(|n| n == DB_FILE).unwrap_or(false) && p.components().count() <= 2)
        .map(|p| p.parent().map(Path::to_path_buf).unwrap_or_default())
        .ok_or("this zip has no flexma.db; is it a Flexma backup?")?;

    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let aside = data_dir.join(format!("pre-restore-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let mut moved = Vec::new();
    for name in ["flexma.db", "flexma.db-wal", "flexma.db-shm", "assets", "fonts", "cache"] {
        let from = data_dir.join(name);
        if from.exists() {
            std::fs::create_dir_all(&aside).map_err(|e| e.to_string())?;
            std::fs::rename(&from, aside.join(name)).map_err(|e| format!("couldn't move {}: {e}", from.display()))?;
            moved.push(name);
        }
    }

    let mut counts = (0usize, 0usize);
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
        let Some(path) = entry.enclosed_name() else { continue };
        let Ok(rel) = path.strip_prefix(&root) else { continue };
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        // Only the database and files directly inside assets/ and fonts/.
        let dest: PathBuf = match parts.as_slice() {
            [f] if f == DB_FILE => data_dir.join(DB_FILE),
            [d, f] if DIRS.contains(&d.as_str()) && !entry.is_dir() => {
                if d == "assets" { counts.0 += 1 } else { counts.1 += 1 }
                data_dir.join(d).join(f)
            }
            _ => continue,
        };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&dest).map_err(|e| format!("couldn't write {}: {e}", dest.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    match_owner(data_dir);
    Ok(format!(
        "Restored the database, {} images and {} font files into {}.{}",
        counts.0,
        counts.1,
        data_dir.display(),
        if moved.is_empty() { String::new() } else { format!("\nThe previous data was moved to {}.", aside.display()) }
    ))
}

/// Give restored files the data directory's owner, so the service user (not
/// root, who ran the restore) can write them.
#[cfg(unix)]
fn match_owner(data_dir: &Path) {
    use std::os::unix::fs::{chown, MetadataExt};
    let Ok(meta) = std::fs::metadata(data_dir) else { return };
    let (uid, gid) = (Some(meta.uid()), Some(meta.gid()));
    let _ = chown(data_dir.join(DB_FILE), uid, gid);
    for dir in DIRS {
        let path = data_dir.join(dir);
        let _ = chown(&path, uid, gid);
        if let Ok(entries) = std::fs::read_dir(&path) {
            for e in entries.filter_map(Result::ok) {
                let _ = chown(e.path(), uid, gid);
            }
        }
    }
}

#[cfg(not(unix))]
fn match_owner(_: &Path) {}
