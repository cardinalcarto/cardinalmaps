//! QRank (https://qrank.wmcloud.org/): Wikidata entities ranked by the page
//! views of their Wikipedia articles over a year, for POI importance.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;

use crate::importance::wikidata_number;

/// Each entity's QRank, sorted by entity number, so a lookup is a binary
/// search over 16 bytes per entity.
pub struct QRank {
    ranks: Vec<(u64, u64)>,
}

impl QRank {
    /// Reads `qrank.csv` or `qrank.csv.gz`: a `Entity,QRank` header, then
    /// lines like `Q60,123456`.
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let reader: Box<dyn Read> =
            if path.extension().is_some_and(|e| e == "gz") { Box::new(GzDecoder::new(file)) } else { Box::new(file) };
        Self::parse(BufReader::new(reader)).with_context(|| format!("reading {}", path.display()))
    }

    /// Reads the CSV from `reader`.
    pub fn parse(reader: impl BufRead) -> Result<Self> {
        let mut lines = reader.lines();
        let header = lines.next().transpose()?;
        if header.as_deref().map(str::trim) != Some("Entity,QRank") {
            bail!("expected a header of Entity,QRank");
        }
        let mut ranks = Vec::new();
        for line in lines {
            let line = line?;
            let Some((entity, rank)) = line.split_once(',') else {
                continue;
            };
            if let (Some(entity), Ok(rank)) = (wikidata_number(entity), rank.trim().parse()) {
                ranks.push((entity, rank));
            }
        }
        ranks.sort_unstable();
        ranks.dedup_by_key(|(entity, _)| *entity);
        Ok(Self { ranks })
    }

    /// How many entities have a QRank.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ranks.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranks.is_empty()
    }

    /// The QRank of a Wikidata ID such as `Q60`.
    #[must_use]
    pub fn get(&self, wikidata: &str) -> Option<u64> {
        let entity = wikidata_number(wikidata)?;
        let i = self.ranks.binary_search_by_key(&entity, |(e, _)| *e).ok()?;
        Some(self.ranks[i].1)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn looks_up_entities() {
        let csv = "Entity,QRank\nQ60,371366713\nQ5083,1234\nnot a line\n";
        let qrank = QRank::parse(csv.as_bytes()).unwrap();
        assert_eq!(qrank.get("Q60"), Some(371_366_713));
        assert_eq!(qrank.get("Q5083"), Some(1234));
        assert_eq!(qrank.get("Q1"), None);
    }
}
