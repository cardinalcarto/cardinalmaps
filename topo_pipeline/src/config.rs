//! The pipeline's configuration file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Base lattice points per degree: 1 arc second.
pub const BASE_PER_DEGREE: i64 = 3600;

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub contours: ContourConfig,
    /// In ascending priority: later sources override earlier ones where they have data.
    #[serde(rename = "source")]
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct ContourConfig {
    pub interval_m: u32,
    pub interval_ft: u32,
    /// Douglas-Peucker tolerance.
    pub simplify_m: f64,
    /// Rounds simplified corners, doubling the nodes of what simplifying left.
    pub round_corners: bool,
    /// Closed contours with a shorter perimeter are DEM noise, and dropped.
    pub min_loop_m: f64,
    /// The most memory given to decoded DEM files.
    pub cache_gb: f64,
}

impl Default for ContourConfig {
    fn default() -> Self {
        Self { interval_m: 10, interval_ft: 40, simplify_m: 8.0, round_corners: true, min_loop_m: 250.0, cache_gb: 16.0 }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub name: String,
    /// Searched recursively. Either this or `s3`.
    #[serde(default)]
    pub dir: PathBuf,
    /// Read zip archives of rasters from a bucket, instead of a directory; see `remote`.
    #[serde(default)]
    pub s3: Option<S3Config>,
    /// Which files in `dir` are this source's elevation rasters.
    pub suffix: String,
    /// The source's lattice spacing. Must be 1 arc second divided by an odd integer
    /// (1, 1/3, 1/9...), so its points nest with the 1 arc second lattice.
    pub arcsec: f64,
    /// A 3x3 median first, for speckle.
    #[serde(default)]
    pub median: bool,
    /// Gaussian smoothing, in lattice cells of this source.
    #[serde(default)]
    pub sigma: f64,
    /// Width of the band, inside this source's edge, in which it blends with the sources
    /// below it, in lattice cells of this source. Ignored for the lowest source.
    #[serde(default)]
    pub feather: u32,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct S3Config {
    /// The S3 endpoint's host.
    pub endpoint: String,
    pub bucket: String,
    /// Where the archives are.
    #[serde(default)]
    pub prefix: String,
    /// The signing region; by default, the endpoint's second label.
    #[serde(default)]
    pub region: Option<String>,
}

impl SourceConfig {
    /// The factor by which the source's lattice refines the 1 arc second one.
    pub fn refine(&self) -> Result<i64> {
        let r = (1.0 / self.arcsec).round() as i64;
        if r < 1 || r % 2 == 0 || (r as f64 * self.arcsec - 1.0).abs() > 1e-6 {
            bail!(
                "source {}: arcsec must be 1 divided by an odd integer so lattices nest, not {}",
                self.name,
                self.arcsec
            );
        }
        Ok(r)
    }

    /// How far outside a region the source must be read to get its smoothed, feathered
    /// values there exactly, in the source's lattice cells.
    pub fn margin(&self) -> i64 {
        let kernel = if self.sigma > 0.0 { (self.sigma * 3.0).ceil() as i64 } else { 0 };
        kernel + i64::from(self.median) + i64::from(self.feather) + 2
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if config.sources.is_empty() {
            bail!("{}: no [[source]]", path.display());
        }
        for source in &config.sources {
            source.refine()?;
            if source.s3.is_some() == !source.dir.as_os_str().is_empty() {
                bail!("source {}: needs either dir or s3", source.name);
            }
        }
        if config.contours.interval_m == 0 || config.contours.interval_ft == 0 {
            bail!("contour intervals must be positive");
        }
        Ok(config)
    }
}
