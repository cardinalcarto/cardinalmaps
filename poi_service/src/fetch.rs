//! Downloading published releases into a local directory, so `import`,
//! `patch`, `covers` and `status` can read them. Clients find releases over
//! plain HTTPS without listing the bucket: `latest.json` names the newest
//! checkpoint, its `release.json` names its snapshot, and the deltas between
//! are the consecutive sequences after it.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};

use crate::published::{FileInfo, RELEASE_FILE, Release, Releases, SNAPSHOTS, file_info, json_bytes, write_file};

const ATTEMPTS: u32 = 6;

struct Client {
    base: String,
    http: reqwest::Client,
}

impl Client {
    async fn get(&self, path: &str) -> Result<reqwest::Response> {
        let url = format!("{}{path}", self.base);
        let mut delay = Duration::from_secs(2);
        for attempt in 1..=ATTEMPTS {
            match self.http.get(&url).send().await.and_then(|r| r.error_for_status()) {
                Ok(response) => return Ok(response),
                Err(e) if e.status().is_some_and(|s| s.is_client_error()) || attempt == ATTEMPTS => {
                    return Err(e).with_context(|| format!("fetching {url}"));
                }
                Err(e) => tracing::warn!("fetching {url} (attempt {attempt}): {e}; retrying in {delay:?}"),
            }
            tokio::time::sleep(delay).await;
            delay *= 2;
        }
        unreachable!()
    }

    async fn json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let bytes = self.get(path).await?.bytes().await?;
        serde_json::from_slice(&bytes).with_context(|| format!("reading {}{path}", self.base))
    }

    /// Downloads `path` to `dest` unless it's there already as `expected`.
    async fn file(&self, path: &str, dest: &Path, expected: &FileInfo) -> Result<()> {
        if dest.is_file() && &file_info(dest)? == expected {
            return Ok(());
        }
        let partial = dest.with_extension("partial");
        let mut out = std::io::BufWriter::new(std::fs::File::create(&partial)?);
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let mut body = self.get(path).await?.bytes_stream();
        while let Some(chunk) = body.try_next().await? {
            hasher.update(&chunk);
            size += chunk.len() as u64;
            out.write_all(&chunk)?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        let found = FileInfo { size, sha256: hex::encode(hasher.finalize()) };
        ensure!(&found == expected, "{}{path} is {found:?}, but its release says {expected:?}", self.base);
        std::fs::rename(&partial, dest)?;
        tracing::info!("downloaded {path} ({size} bytes)");
        Ok(())
    }

    /// Downloads one release's files, then its `release.json`.
    async fn release(&self, remote: &str, dir: &Path, release: &Release) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        for (name, info) in &release.files {
            ensure!(!name.contains('/') && !name.starts_with('.'), "bad file name {name:?}");
            self.file(&format!("{remote}/{name}"), &dir.join(name), info).await?;
        }
        write_file(&dir.join(RELEASE_FILE), &json_bytes(release)?)
    }
}

/// Downloads what `dir` needs to reach checkpoint `to` (default: the one
/// `latest.json` names) from `url`, the bucket's `releases/` prefix over
/// HTTPS. Files already there are kept.
pub async fn fetch(url: &str, dir: &Path, to: Option<i64>) -> Result<i64> {
    let base = format!("{}/", url.trim_end_matches('/'));
    let http =
        reqwest::Client::builder().user_agent(concat!("cardinal-poi-service/", env!("CARGO_PKG_VERSION"))).build()?;
    let client = Client { base, http };
    let to = match to {
        Some(to) => to,
        None => {
            let latest: serde_json::Value = client.json("latest.json").await?;
            latest.get("poi").and_then(serde_json::Value::as_i64).context("latest.json names no poi release")?
        }
    };
    let releases = Releases::new(dir);
    let target: Release = client.json(&format!("poi/{to}/{RELEASE_FILE}")).await?;
    ensure!(target.sequence == to, "release {to} says it's {}", target.sequence);
    let snapshot = target.snapshot;
    if releases.snapshot_release(snapshot)?.is_none() {
        let remote = format!("poi/{SNAPSHOTS}/{snapshot}");
        let release: Release = client.json(&format!("{remote}/{RELEASE_FILE}")).await?;
        client.release(&remote, &releases.snapshot_dir(snapshot), &release).await?;
    }
    for sequence in snapshot..=to {
        if let Some(local) = releases.daily(sequence)? {
            if sequence == to && local != target {
                bail!("{} has another release {sequence}", dir.display());
            }
            continue;
        }
        let remote = format!("poi/{sequence}");
        let release: Release =
            if sequence == to { target.clone() } else { client.json(&format!("{remote}/{RELEASE_FILE}")).await? };
        client.release(&remote, &releases.daily_dir(sequence), &release).await?;
    }
    // Check the chain links up.
    releases.chain(Some(to), None)?;
    Ok(to)
}
