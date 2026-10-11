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
        }
    }
}
