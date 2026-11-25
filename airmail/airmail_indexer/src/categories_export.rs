//! Training data for the categorical search model: one JSON line per named POI
//! with category tags, holding its names grouped by language.
//!
//! A plain `name` belongs to every language Who's On First lists as spoken
//! where the POI is, so POIs in Quebec train both the English and the French
//! model. `name:xx` belongs to language xx only where xx is spoken, so a
//! transliterated `name:en` in Vietnam doesn't teach the English model what
//! Vietnamese words mean in Vietnam.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use airmail::{categories, poi::ToIndexPoi};
use anyhow::Result;
use serde::Serialize;

use crate::importer::PoiSink;

#[derive(Debug, PartialEq, Serialize)]
pub struct TrainingPoi {
    /// ISO 639-3 language code to the POI's names in that language.
    pub names: BTreeMap<String, BTreeSet<String>>,
    /// Category tags as `key=value`, `;` values split.
    pub categories: BTreeSet<String>,
    pub cuisine: Option<String>,
    pub diet: BTreeMap<String, String>,
}

impl TrainingPoi {
    /// `None` if the POI has no category tags or no names in a language
    /// spoken where it is.
    #[must_use]
    pub fn new(tags: &[(String, String)], languages: &[String]) -> Option<Self> {
        let categories: BTreeSet<String> = tags
            .iter()
            .flat_map(|(key, raw)| {
                categories::split_values(raw)
                    .into_iter()
                    .filter(|value| categories::is_category_tag(key, value))
                    .map(move |value| format!("{key}={value}"))
            })
            .collect();
        if categories.is_empty() {
            return None;
        }

        let mut names: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (key, value) in tags {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if key == "name" {
                for language in languages {
                    names
                        .entry(language.clone())
                        .or_default()
                        .insert(value.to_string());
                }
            } else if let Some(tag) = key.strip_prefix("name:") {
                let Some(language) = categories::iso_639_3(tag) else {
                    continue;
                };
                if !tag.contains(':') && languages.iter().any(|l| l == language) {
                    names
                        .entry(language.to_string())
                        .or_default()
                        .insert(value.to_string());
                }
            }
        }
        if names.is_empty() {
            return None;
        }

        let tag = |k: &str| {
            tags.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        Some(Self {
            names,
            categories,
            cuisine: tag("cuisine"),
            diet: tags
                .iter()
                .filter(|(k, _)| k.starts_with("diet:"))
                .cloned()
                .collect(),
        })
    }
}

/// Writes [`TrainingPoi`]s as JSON lines, renaming the file into place when done.
pub struct CategoriesExportSink {
    out: BufWriter<File>,
    partial: PathBuf,
    path: PathBuf,
    written: usize,
}

impl CategoriesExportSink {
    pub fn new(path: &Path) -> Result<Self> {
        let partial = path.with_extension("partial");
        Ok(Self {
            out: BufWriter::new(File::create(&partial)?),
            partial,
            path: path.to_path_buf(),
            written: 0,
        })
    }
}

impl PoiSink for CategoriesExportSink {
    type Prepared = Option<TrainingPoi>;

    fn preparer(
        &self,
    ) -> impl Fn(ToIndexPoi, &[String]) -> Result<Self::Prepared> + Clone + Send + Sync + 'static
    {
        |poi, languages| Ok(TrainingPoi::new(&poi.tags, languages))
    }

    fn add(&mut self, prepared: Self::Prepared) -> Result<()> {
        if let Some(record) = prepared {
            serde_json::to_writer(&mut self.out, &record)?;
            self.out.write_all(b"\n")?;
            self.written += 1;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.out.flush()?;
        std::fs::rename(&self.partial, &self.path)?;
        log::info!("Wrote {} POIs to {}", self.written, self.path.display());
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn languages(codes: &[&str]) -> Vec<String> {
        codes.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn test_plain_name_goes_to_every_spoken_language() {
        let poi = TrainingPoi::new(
            &tags(&[
                ("name", "Chez Ma Tante"),
                ("amenity", "restaurant"),
                ("building", "yes"),
            ]),
            &languages(&["eng", "fra"]),
        )
        .unwrap();
        assert_eq!(poi.names.keys().collect::<Vec<_>>(), vec!["eng", "fra"]);
        assert_eq!(
            poi.categories.iter().collect::<Vec<_>>(),
            vec!["amenity=restaurant"]
        );
    }

    #[test]
    fn test_translated_name_only_where_spoken() {
        let poi = TrainingPoi::new(
            &tags(&[
                ("name", "Bánh Mì Huỳnh Hoa"),
                ("name:en", "Huynh Hoa Banh Mi"),
                ("name:vi", "Bánh Mì Huỳnh Hoa"),
                ("shop", "bakery"),
            ]),
            &languages(&["vie"]),
        )
        .unwrap();
        assert_eq!(poi.names.keys().collect::<Vec<_>>(), vec!["vie"]);
    }

    #[test]
    fn test_requires_category_and_name() {
        assert!(
            TrainingPoi::new(
                &tags(&[("name", "x"), ("building", "yes")]),
                &languages(&["eng"])
            )
            .is_none()
        );
        assert!(TrainingPoi::new(&tags(&[("name", "x"), ("shop", "bakery")]), &[]).is_none());
        assert!(
            TrainingPoi::new(
                &tags(&[("name:de", "x"), ("shop", "bakery")]),
                &languages(&["fra"])
            )
            .is_none()
        );
    }
}
