//! Configuration from environment variables (12-factor style, Docker friendly).

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    /// Listen address, e.g. `0.0.0.0:8080`.
    pub addr: String,
    /// Where the SQLite database and uploaded assets live.
    pub data_dir: PathBuf,
    /// Built web app (`web/dist`).
    pub web_dir: PathBuf,
    /// Public base URL (used in invite links and agent setup snippets). When
    /// unset, it is derived from the request's Host header.
    pub public_url: Option<String>,
    /// Allow anyone to sign up (otherwise invite-only after the first admin).
    pub allow_signup: bool,
    pub instance_name: String,
    /// Max upload size for image assets, bytes.
    pub max_upload_bytes: usize,
    /// Mark the session cookie `Secure` (set when serving over HTTPS).
    pub secure_cookies: bool,
    /// Required to create the first (admin) account, so a fresh public
    /// instance can't be claimed by whoever finds it first. Taken from
    /// `FLEXMA_SETUP_TOKEN`, or generated at startup and logged.
    pub setup_token: Option<String>,
    /// Longest side of a PNG export, px. Each render needs about 4 bytes per
    /// pixel, so lower it on small servers.
    pub max_export_px: u32,
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn env_bool(key: &str, default: bool) -> bool {
    env(key)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

impl Config {
    pub fn from_env() -> Config {
        let public_url = env("FLEXMA_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string());
        let secure_default = public_url.as_deref().map(|u| u.starts_with("https://")).unwrap_or(false);
        Config {
            addr: env("FLEXMA_ADDR").unwrap_or_else(|| "0.0.0.0:8080".into()),
            data_dir: env("FLEXMA_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("./data")),
            web_dir: env("FLEXMA_WEB_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("./web/dist")),
            public_url,
            allow_signup: env_bool("FLEXMA_ALLOW_SIGNUP", false),
            instance_name: env("FLEXMA_INSTANCE_NAME").unwrap_or_else(|| "Flexma".into()),
            max_upload_bytes: env("FLEXMA_MAX_UPLOAD_MB")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(25)
                * 1024
                * 1024,
            secure_cookies: env_bool("FLEXMA_SECURE_COOKIES", secure_default),
            setup_token: env("FLEXMA_SETUP_TOKEN"),
            max_export_px: env("FLEXMA_MAX_EXPORT_PX").and_then(|v| v.parse().ok()).unwrap_or(8192).clamp(256, 16384),
        }
    }
}
