//! Tiles from a tile server: a TileJSON URL, whose `tiles` template says where each tile
//! is. Responses go through a [`DiskCache`], tiles as the server sent them (usually
//! gzipped), so a tile is fetched once and later revalidated with its ETag.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sparkle::TileId;
use ureq::Agent;
use ureq::http::StatusCode;

use crate::cache::{Cached, DiskCache};
use crate::mbtiles::{Metadata, TileFormat, decompress};

/// Cached tiles older than this are drawn, then checked with the server.
pub const REVALIDATE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// Responses bigger than this are refused.
const MAX_BODY: u64 = 64 << 20;

/// The HTTP client every server shares, so connections are kept alive between tiles.
pub fn agent() -> Agent {
    Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .user_agent(concat!("sparkle/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub fn is_url(path: &str) -> bool {
    path.starts_with("https://") || path.starts_with("http://")
}

/// What the server answered for a tile.
pub enum Fetched {
    /// The tile, as sent (possibly gzipped).
    Tile(Vec<u8>),
    /// The server has nothing here.
    Empty,
    /// Revalidation found the cached tile current.
    Unchanged,
}

pub struct TileServer {
    /// The TileJSON's URL.
    pub url: String,
    /// With `{z}`, `{x}` and `{y}`.
    template: String,
    /// TileJSON's `scheme: tms` counts rows from the bottom.
    tms: bool,
    agent: Agent,
    cache: Arc<DiskCache>,
    /// Tiles being revalidated, so that each is checked once at a time.
    revalidating: Mutex<HashSet<TileId>>,
    /// The last request failed. Logged once until one succeeds, not for every tile.
    unreachable: AtomicBool,
}

impl TileServer {
    /// Read the TileJSON at `url`, from the server if it answers, else from the cache.
    /// `format` is assumed if the TileJSON doesn't name one.
    pub fn open(
        url: &str,
        agent: Agent,
        cache: Arc<DiskCache>,
        format: TileFormat,
    ) -> Result<(Self, Metadata)> {
        let text = match get(&agent, url, None) {
            Ok((StatusCode::OK, Some(body), etag)) => {
                cache.put(url, Some(&body), etag.as_deref())?;
                body
            }
            result => {
                let err = match result {
                    Ok((status, ..)) => anyhow!("HTTP {status}"),
                    Err(err) => err,
                };
                let Some(Cached {
                    data: Some(body), ..
                }) = cache.get(url)?
                else {
                    return Err(err.context(format!("fetching {url}")));
                };
                log::warn!("{url}: {err:#}; using the cached TileJSON");
                body
            }
        };
        let json: serde_json::Value = serde_json::from_slice(&decompress(text)?)
            .with_context(|| format!("{url}: not JSON"))?;
        let template = json["tiles"][0]
            .as_str()
            .with_context(|| format!("{url}: TileJSON without `tiles`"))?
            .to_owned();
        let numbers = |key: &str| -> Option<Vec<f64>> {
            json[key].as_array()?.iter().map(|v| v.as_f64()).collect()
        };
        let zoom = |key: &str, default: u8| json[key].as_u64().map_or(default, |z| z as u8);
        let metadata = Metadata {
            format: match json["format"].as_str() {
                Some(name) => TileFormat::from_name(name).with_context(|| url.to_owned())?,
                // Without `format` (as go-pmtiles serves TileJSON), the tile URLs'
                // extension, such as `.mlt`, says; failing that, `format`.
                None => template_format(&template).unwrap_or(format),
            },
            min_zoom: zoom("minzoom", 0),
            max_zoom: zoom("maxzoom", 22),
            center: numbers("center").and_then(|v| Some((*v.first()?, *v.get(1)?, *v.get(2)?))),
            bounds: numbers("bounds").and_then(|v| v.try_into().ok()),
        };
        let server = Self {
            url: url.to_owned(),
            template,
            tms: json["scheme"] == "tms",
            agent,
            cache,
            revalidating: Mutex::new(HashSet::new()),
            unreachable: AtomicBool::new(false),
        };
        Ok((server, metadata))
    }

    fn tile_url(&self, id: TileId) -> String {
        let y = if self.tms {
            (1u32 << id.z) - 1 - id.y
        } else {
            id.y
        };
        self.template
            .replace("{z}", &id.z.to_string())
            .replace("{x}", &id.x.to_string())
            .replace("{y}", &y.to_string())
    }

    /// The tile as cached, if it is.
    pub fn cached(&self, id: TileId) -> Result<Option<Cached>> {
        self.cache.get(&self.tile_url(id))
    }

    /// Ask the server for a tile, revalidating the cached one if `etag` is given, and
    /// cache the answer. Errors are worth retrying: the server couldn't be reached, or
    /// was unavailable.
    pub fn fetch(&self, id: TileId, etag: Option<&str>) -> Result<Fetched> {
        let url = self.tile_url(id);
        let result = get(&self.agent, &url, etag).and_then(|(status, body, etag)| {
            if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                bail!("HTTP {status}");
            }
            Ok((status, body, etag))
        });
        let (status, body, etag) = match result {
            Ok(response) => {
                if self.unreachable.swap(false, Ordering::Relaxed) {
                    log::info!("{}: reachable again", self.url);
                }
                response
            }
            Err(err) => {
                if !self.unreachable.swap(true, Ordering::Relaxed) {
                    log::warn!("{}: {err:#}; retrying", self.url);
                }
                return Err(err.context(url));
            }
        };
        Ok(match (status, body) {
            (StatusCode::OK, Some(body)) => {
                self.cache.put(&url, Some(&body), etag.as_deref())?;
                Fetched::Tile(body)
            }
            (StatusCode::NOT_MODIFIED, _) => {
                self.cache.revalidated(&url)?;
                Fetched::Unchanged
            }
            (StatusCode::NO_CONTENT | StatusCode::NOT_FOUND, _) => {
                self.cache.put(&url, None, None)?;
                Fetched::Empty
            }
            (status, _) => {
                // Not cached: it may be fixed by the next run.
                log::warn!("{url}: HTTP {status}");
                Fetched::Empty
            }
        })
    }

    /// Note that `id` is being revalidated. False if it already is.
    pub fn start_revalidating(&self, id: TileId) -> bool {
        self.revalidating.lock().unwrap().insert(id)
    }

    pub fn end_revalidating(&self, id: TileId) {
        self.revalidating.lock().unwrap().remove(&id);
    }
}

/// `GET url`, conditional on `etag`: the status, the body of a 200, and its ETag.
fn get(
    agent: &Agent,
    url: &str,
    etag: Option<&str>,
) -> Result<(StatusCode, Option<Vec<u8>>, Option<String>)> {
    // Without ureq's gzip feature, gzipped bodies arrive as they are, which is how the
    // cache keeps them.
    let mut request = agent.get(url).header("Accept-Encoding", "gzip");
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }
    let mut response = request.call()?;
    let status = response.status();
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = if status == StatusCode::OK {
        Some(
            response
                .body_mut()
                .with_config()
                .limit(MAX_BODY)
                .read_to_vec()?,
        )
    } else {
        None
    };
    Ok((status, body, etag))
}

/// The tile format a tile URL template's extension names (`…/{z}/{x}/{y}.mlt`), if any.
fn template_format(template: &str) -> Option<TileFormat> {
    let path = template.split(['?', '#']).next()?;
    let (_, ext) = path.rsplit_once('/')?.1.rsplit_once('.')?;
    TileFormat::from_name(&ext.to_ascii_lowercase()).ok()
}

#[cfg(test)]
mod template_format_tests {
    use super::*;

    #[test]
    fn the_tile_urls_extension_names_the_format() {
        let mlt = template_format("https://tiles.example.org/2026-11-11/planet/{z}/{x}/{y}.mlt");
        assert!(matches!(mlt, Some(TileFormat::Mlt)));
        let mvt = template_format("https://example.org/t/{z}/{x}/{y}.pbf?key=1");
        assert!(matches!(mvt, Some(TileFormat::Mvt)));
        assert!(template_format("http://127.0.0.1:3001/tiles/basemap/{z}/{x}/{y}").is_none());
        assert!(template_format("https://example.org/v1.2/{z}/{x}/{y}").is_none());
    }
}
