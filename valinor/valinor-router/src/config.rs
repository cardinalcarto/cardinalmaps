//! The router's configuration: the stages' parts of mainline's `valhalla.json`,
//! and the router's own limits.

use core::num::NonZeroUsize;
use illuvatar::{MarkupFormatter, Odin};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use thiserror::Error;
use valinor_loki::LokiConfig;
use valinor_thor::ThorConfig;

/// Why a `valhalla.json` couldn't be read.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The JSON doesn't parse, or an Odin value has the wrong type.
    #[error("parse the config: {0}")]
    Json(#[from] serde_json::Error),
    /// Loki's part is invalid.
    #[error("read Loki's config: {0}")]
    Loki(#[from] valinor_loki::ConfigError),
    /// Thor's part is invalid.
    #[error("read Thor's config: {0}")]
    Thor(#[from] valinor_thor::ConfigError),
}

/// What the router reads from `valhalla.json`, and its own limits.
#[derive(Debug, Clone)]
pub struct RouterConfig {
    /// Loki's part (actions, service defaults, and service limits).
    pub loki: LokiConfig,
    /// Thor's part (search parameters and hierarchy limits).
    pub thor: ThorConfig,
    /// Odin's part (`odin.markup_formatter`).
    pub odin: Odin,
    /// `mjolnir.tile_extract`, if the config names one (relative to the working directory,
    /// as mainline reads it). [`crate::Router::open`] doesn't read it: callers pass tile sets.
    pub tile_extract: Option<PathBuf>,
    /// How many path search workspaces to keep, and so how many routes run at once;
    /// requests beyond that wait for one.
    ///
    /// A workspace keeps its searches' storage between requests,
    /// which grows to the size of the largest search it has run.
    pub workspaces: NonZeroUsize,
}

impl Default for RouterConfig {
    /// Mainline's defaults, with a workspace per available core.
    fn default() -> Self {
        Self {
            loki: LokiConfig::default(),
            thor: ThorConfig::default(),
            odin: Odin::default(),
            tile_extract: None,
            workspaces: std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
        }
    }
}

#[derive(Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    odin: RawOdin,
    #[serde(default)]
    mjolnir: RawMjolnir,
}

#[derive(Deserialize, Default)]
struct RawOdin {
    #[serde(default)]
    markup_formatter: RawMarkup,
}

#[derive(Deserialize, Default)]
struct RawMarkup {
    #[serde(default)]
    markup_enabled: bool,
    #[serde(default)]
    phoneme_format: String,
}

#[derive(Deserialize, Default)]
struct RawMjolnir {
    tile_extract: Option<String>,
}

impl RouterConfig {
    /// Reads the stages' parts of a mainline `valhalla.json`;
    /// the router's own limits keep their defaults.
    ///
    /// # Errors
    ///
    /// Fails if the JSON doesn't parse or a stage's part is invalid.
    pub fn from_valhalla_json(json: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = serde_json::from_str(json)?;
        Ok(Self {
            loki: LokiConfig::from_valhalla_json(json)?,
            thor: ThorConfig::from_valhalla_json(json)?,
            odin: Odin::new(MarkupFormatter::new(
                raw.odin.markup_formatter.markup_enabled,
                raw.odin.markup_formatter.phoneme_format,
            )),
            tile_extract: raw
                .mjolnir
                .tile_extract
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            ..Self::default()
        })
    }

    /// `mjolnir.tile_extract`, resolved against `directory` if it is relative
    /// (for a config that names its extract relative to its own directory).
    pub fn tile_extract_in(&self, directory: &Path) -> Option<PathBuf> {
        self.tile_extract.as_ref().map(|path| directory.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_stage() {
        let config = RouterConfig::from_valhalla_json(
            r#"{
                "loki": {"actions": ["route", "status"]},
                "thor": {"extended_search": true},
                "odin": {"markup_formatter": {"markup_enabled": true, "phoneme_format": "<x>"}},
                "mjolnir": {"tile_extract": "tiles.tar", "tile_dir": ""}
            }"#,
        )
        .unwrap();
        assert_eq!(config.loki.actions.len(), 2);
        assert!(config.thor.extended_search);
        assert_eq!(config.tile_extract, Some(PathBuf::from("tiles.tar")));
        assert_eq!(
            config.tile_extract_in(Path::new("/releases/1/routing")),
            Some(PathBuf::from("/releases/1/routing/tiles.tar"))
        );
        assert_eq!(
            config
                .tile_extract_in(Path::new("/elsewhere"))
                .map(|p| p.is_absolute()),
            Some(true)
        );

        let config = RouterConfig::from_valhalla_json("{}").unwrap();
        assert_eq!(config.tile_extract, None);
        let config =
            RouterConfig::from_valhalla_json(r#"{"mjolnir": {"tile_extract": ""}}"#).unwrap();
        assert_eq!(config.tile_extract, None);
        // An absolute extract stays where it is.
        let config =
            RouterConfig::from_valhalla_json(r#"{"mjolnir": {"tile_extract": "/data/t.tar"}}"#)
                .unwrap();
        assert_eq!(
            config.tile_extract_in(Path::new("/releases")),
            Some(PathBuf::from("/data/t.tar"))
        );

        assert!(matches!(
            RouterConfig::from_valhalla_json(r#"{"loki": {"actions": []}}"#),
            Err(ConfigError::Loki(_))
        ));
        assert!(matches!(
            RouterConfig::from_valhalla_json("["),
            Err(ConfigError::Json(_))
        ));
    }
}
