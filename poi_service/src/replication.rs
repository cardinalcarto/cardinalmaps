//! Reading OSM replication diffs from a server such as
//! <https://planet.openstreetmap.org/replication/day/>.

use std::io::{BufReader, Read};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use flate2::read::MultiGzDecoder;

use crate::osm::{ElementVersion, read_osc};

pub const DEFAULT_URL: &str = "https://planet.openstreetmap.org/replication/day/";

const ATTEMPTS: u32 = 10;
/// The first retry's delay, doubled for each after it up to [`MAX_RETRY_DELAY`].
const RETRY_DELAY: Duration = Duration::from_secs(30);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy)]
pub struct State {
    pub sequence: i64,
    pub timestamp: DateTime<Utc>,
}

pub struct Server {
    base: String,
    client: reqwest::Client,
}

impl Server {
    pub fn new(url: &str) -> Self {
        let base = if url.ends_with('/') { url.to_owned() } else { format!("{url}/") };
        let client = reqwest::Client::builder()
            .user_agent(concat!("cardinal-poi-service/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("an HTTP client with default TLS settings");
        Server { base, client }
    }

    fn path(&self, sequence: i64, suffix: &str) -> String {
        let digits = format!("{sequence:09}");
        format!("{}{}/{}/{}{suffix}", self.base, &digits[..3], &digits[3..6], &digits[6..])
    }

    async fn get(&self, url: &str) -> Result<Vec<u8>> {
        let mut attempt = 1;
        let mut delay = RETRY_DELAY;
        loop {
            let response = self.client.get(url).send().await.and_then(|r| r.error_for_status());
            let result = match response {
                Ok(response) => response.bytes().await,
                Err(e) => Err(e),
            };
            match result {
                Ok(body) => return Ok(body.to_vec()),
                Err(e) if e.status().is_some_and(|s| s.is_client_error()) || attempt == ATTEMPTS => {
                    return Err(e).with_context(|| format!("fetching {url}"));
                }
                Err(e) => tracing::warn!("fetching {url} (attempt {attempt}): {e}; retrying in {delay:?}"),
            }
            attempt += 1;
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(MAX_RETRY_DELAY);
        }
    }

    /// The latest state, or that of `sequence`.
    pub async fn state(&self, sequence: Option<i64>) -> Result<State> {
        let url = match sequence {
            Some(sequence) => self.path(sequence, ".state.txt"),
            None => format!("{}state.txt", self.base),
        };
        parse_state(&String::from_utf8(self.get(&url).await?)?).with_context(|| format!("parsing {url}"))
    }

    /// The newest sequence whose diffs are all at or before `timestamp`, so the
    /// next sequence is the first with changes after it.
    pub async fn sequence_at(&self, timestamp: DateTime<Utc>) -> Result<i64> {
        let latest = self.state(None).await?;
        if latest.timestamp <= timestamp {
            return Ok(latest.sequence);
        }
        // Diffs are roughly evenly spaced, so start from an estimate and step.
        let interval = match self.state(Some(latest.sequence - 1)).await {
            Ok(previous) => (latest.timestamp - previous.timestamp).max(chrono::Duration::seconds(1)),
            Err(_) => bail!("can't find the replication sequence for {timestamp}"),
        };
        let behind = (latest.timestamp - timestamp).num_seconds() / interval.num_seconds();
        let mut sequence = (latest.sequence - behind).max(0);
        while self.state(Some(sequence)).await?.timestamp > timestamp {
            sequence -= 1;
        }
        while sequence < latest.sequence && self.state(Some(sequence + 1)).await?.timestamp <= timestamp {
            sequence += 1;
        }
        Ok(sequence)
    }

    /// Downloads and parses the diff for `sequence`.
    pub async fn diff(&self, sequence: i64) -> Result<Vec<ElementVersion>> {
        let url = self.path(sequence, ".osc.gz");
        let body = self.get(&url).await?;
        tokio::task::spawn_blocking(move || read_gzipped_osc(&body[..]))
            .await?
            .with_context(|| format!("reading {url}"))
    }
}

pub fn read_gzipped_osc(input: impl Read) -> Result<Vec<ElementVersion>> {
    read_osc(BufReader::new(MultiGzDecoder::new(input)))
}

fn parse_state(text: &str) -> Result<State> {
    let (mut sequence, mut timestamp) = (None, None);
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().replace('\\', "");
        match key.trim() {
            "sequenceNumber" => sequence = Some(value.parse()?),
            "timestamp" => timestamp = Some(DateTime::parse_from_rfc3339(&value)?.with_timezone(&Utc)),
            _ => {}
        }
    }
    Ok(State { sequence: sequence.context("no sequenceNumber")?, timestamp: timestamp.context("no timestamp")? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state() {
        let state =
            parse_state("#Sat Oct 05 00:20:43 UTC 2026\nsequenceNumber=5136\ntimestamp=2026-10-05T00\\:00\\:00Z\n")
                .unwrap();
        assert_eq!(state.sequence, 5136);
        assert_eq!(state.timestamp.to_rfc3339(), "2026-10-05T00:00:00+00:00");
    }

    #[test]
    fn builds_paths() {
        let server = Server::new("https://planet.openstreetmap.org/replication/day");
        assert_eq!(server.path(5137, ".osc.gz"), "https://planet.openstreetmap.org/replication/day/000/005/137.osc.gz");
    }
}
