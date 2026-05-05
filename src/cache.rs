use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheMode {
    Auto,
    Bypass,
    Refresh,
    Offline,
}

#[derive(Clone, Debug)]
pub struct CacheConfig {
    pub mode: CacheMode,
    pub root: PathBuf,
    pub ttl_seconds: u64,
}

#[derive(Clone, Debug)]
pub struct CacheStore {
    config: CacheConfig,
    auth_scope: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CachedResponse {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub fetched_at: u64,
    pub body: Value,
}

impl CacheConfig {
    pub fn default_root() -> PathBuf {
        if let Some(cache_home) = std::env::var_os("XDG_CACHE_HOME") {
            return PathBuf::from(cache_home).join("ghdump").join("http-cache");
        }

        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".cache")
                .join("ghdump")
                .join("http-cache");
        }

        std::env::temp_dir().join("ghdump").join("http-cache")
    }
}

impl CacheStore {
    pub fn new(config: CacheConfig, token: Option<&str>) -> Self {
        Self {
            config,
            auth_scope: auth_scope(token),
        }
    }

    pub fn mode(&self) -> CacheMode {
        self.config.mode
    }

    pub fn key(&self, method: &str, url: &str, discriminator: &str) -> String {
        let raw = format!("{}{}{}{}", method, url, discriminator, self.auth_scope);
        sha256_hex(raw.as_bytes())
    }

    pub fn read(&self, key: &str) -> anyhow::Result<Option<CachedResponse>> {
        if self.config.mode == CacheMode::Bypass {
            return Ok(None);
        }

        let contents = match cacache_ttl::read_sync(&self.config.root, key) {
            Ok(contents) => contents,
            Err(cacache_ttl::Error::EntryNotFoundOrExpired(_, _)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let entry = serde_json::from_slice(&contents)
            .with_context(|| format!("failed to decode cache entry {key}"))?;

        Ok(Some(entry))
    }

    pub fn write(&self, key: &str, entry: &CachedResponse) -> anyhow::Result<()> {
        if matches!(self.config.mode, CacheMode::Bypass | CacheMode::Offline) {
            return Ok(());
        }

        let contents = serde_json::to_vec(entry).context("failed to serialize cache entry")?;
        cacache_ttl::write_sync(
            &self.config.root,
            key,
            contents,
            std::time::Duration::from_secs(self.config.ttl_seconds),
        )
        .with_context(|| {
            format!(
                "failed to write cache entry {key} to {}",
                self.config.root.display()
            )
        })?;

        Ok(())
    }

    pub fn require_cached<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
        label: &str,
    ) -> anyhow::Result<T> {
        let Some(entry) = self.read(key)? else {
            bail!("cache miss for {label} while running with --offline");
        };

        serde_json::from_value(entry.body).context("failed to decode cached response")
    }
}

impl CachedResponse {
    pub fn new(etag: Option<String>, last_modified: Option<String>, body: Value) -> Self {
        Self {
            etag,
            last_modified,
            fetched_at: now_unix_seconds(),
            body,
        }
    }
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn auth_scope(token: Option<&str>) -> String {
    match token {
        Some(token) if !token.trim().is_empty() => format!("token-{}", token.trim()),
        _ => "anonymous".to_owned(),
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, thread, time::Duration};

    use serde_json::json;

    use super::*;

    fn temp_cache_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ghdump-cache-test-{name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn store(root: PathBuf, mode: CacheMode) -> CacheStore {
        CacheStore::new(
            CacheConfig {
                mode,
                root,
                ttl_seconds: 300,
            },
            Some("test-token"),
        )
    }

    #[test]
    fn stores_and_reads_cached_response_with_cacache() {
        let root = temp_cache_root("read-write");
        let cache = store(root.clone(), CacheMode::Auto);
        let key = cache.key(
            "GET",
            "https://api.github.com/repos/example/repo",
            "version",
        );
        let response = CachedResponse::new(
            Some("\"etag\"".to_owned()),
            Some("Tue, 05 May 2026 12:00:00 GMT".to_owned()),
            json!({"ok": true}),
        );

        cache.write(&key, &response).expect("cache write succeeds");

        let cached = cache
            .read(&key)
            .expect("cache read succeeds")
            .expect("cache entry exists");
        assert_eq!(cached.etag, response.etag);
        assert_eq!(cached.last_modified, response.last_modified);
        assert_eq!(cached.body, response.body);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bypass_mode_does_not_read_cached_entries() {
        let root = temp_cache_root("bypass");
        let writer = store(root.clone(), CacheMode::Auto);
        let key = writer.key(
            "GET",
            "https://api.github.com/repos/example/repo",
            "version",
        );
        writer
            .write(&key, &CachedResponse::new(None, None, json!({"ok": true})))
            .expect("cache write succeeds");

        let bypass = store(root.clone(), CacheMode::Bypass);
        assert!(bypass.read(&key).expect("cache read succeeds").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stores_and_reads_cached_response_with_cacache_ttl() {
        let root = temp_cache_root("ttl-read-write");
        let cache = store(root.clone(), CacheMode::Auto);
        let key = cache.key(
            "POST",
            "https://api.github.com/graphql",
            r#"{"query":"query { viewer { login } }"}"#,
        );
        let response =
            CachedResponse::new(None, None, json!({"data": {"viewer": {"login": "me"}}}));

        cache.write(&key, &response).expect("cache write succeeds");

        let cached = cache
            .read(&key)
            .expect("cache read succeeds")
            .expect("cache entry exists");
        assert_eq!(cached.body, response.body);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ttl_read_removes_expired_cached_response() {
        let root = temp_cache_root("ttl-expired");
        let cache = CacheStore::new(
            CacheConfig {
                mode: CacheMode::Auto,
                root: root.clone(),
                ttl_seconds: 0,
            },
            Some("test-token"),
        );
        let key = cache.key(
            "POST",
            "https://api.github.com/graphql",
            r#"{"query":"query { viewer { login } }"}"#,
        );
        let response =
            CachedResponse::new(None, None, json!({"data": {"viewer": {"login": "me"}}}));
        cache.write(&key, &response).expect("cache write succeeds");
        thread::sleep(Duration::from_millis(2));

        assert!(cache.read(&key).expect("cache read succeeds").is_none());

        let _ = fs::remove_dir_all(root);
    }
}
