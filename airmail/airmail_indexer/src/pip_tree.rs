use std::{path::Path, sync::Arc};

use anyhow::Result;
use geo::{Intersects, Polygon};
use geo_types::{Geometry, Point};
use log::{debug, info};
use rstar::{AABB, RTree, primitives::GeomWithData};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::task::spawn_blocking;

use crate::wof::{ConcisePipResponse, PipWithGeometry, WhosOnFirst};

/// A spatial index to hold hold and efficiently query polygons
#[derive(Serialize, Deserialize, Clone)]
pub struct PipTree<T> {
    tree: Arc<RTree<GeomWithData<geo_types::Polygon, T>>>,
}

impl PipTree<ConcisePipResponse> {
    /// Either load a `PipTree` from disk, or create a new one
    /// from a `WhosOnFirst` database, and write it to disk.
    ///
    /// Either load a constructed `WhoIsOnFirst` `PipTree` from disk,
    /// or assemble a new one from a `WhosOnFirst` database.
    pub async fn new_or_load(wof_db: &WhosOnFirst, path: &Path) -> Result<Self> {
        if path.exists() {
            Self::new_from_disk(path).await
        } else {
            let pip_tree = Self::new_from_wof_db(wof_db).await?;
            pip_tree.write_to_disk(path).await?;
            Ok(pip_tree)
        }
    }

    /// Create a new `PipTree` from a `WhosOnFirst` database.
    pub async fn new_from_wof_db(wof_db: &WhosOnFirst) -> Result<Self> {
        info!("Creating PipTree from WhosOnFirst database");
        let features: Vec<PipWithGeometry> = wof_db.all_polygons().await?;
        Ok(Self::new(features))
    }

    /// Load a `PipTree` from disk.
    pub async fn new_from_disk(path: &Path) -> Result<Self> {
        let path = path.to_path_buf();

        let handle = spawn_blocking(move || {
            info!("Loading PipTree from disk: {:?}", path);

            let file = std::fs::File::open(path)?;
            let reader = std::io::BufReader::new(file);
            let tree = postcard::from_io((reader, &mut [0u8; 64 * 1024]))?.0;

            Ok(tree)
        });

        handle.await?
    }
}

/// A semi-generic spatial index to hold and efficiently query polygons
impl<T> PipTree<T>
where
    T: Clone + DeserializeOwned + Serialize + Send + Sync + 'static,
{
    /// Create a new `PipTree` from a list of features.
    /// The features ordinarily contain both geometry and properties,
    /// so they need to be split into their component parts for storage.
    /// E.g. `impl From<S> for (Option<geo_types::Geometry<f64>>, T)`
    #[must_use]
    pub fn new<S>(features: Vec<S>) -> Self
    where
        S: Into<(Option<geo_types::Geometry<f64>>, T)>,
    {
        let features: Vec<GeomWithData<Polygon, T>> = features
            .into_iter()
            .filter_map(|feature| {
                let (geom, t) = feature.into();
                if let Some(Geometry::Polygon(polygon)) = geom {
                    Some(GeomWithData::new(polygon, t))
                } else {
                    None
                }
            })
            .collect();

        info!("Creating PipTree with {} polygons", features.len());
        let tree = RTree::bulk_load(features);
        debug!("PipTree created");

        Self {
            tree: Arc::new(tree),
        }
    }

    /// Write the `PipTree` to disk.
    pub async fn write_to_disk(&self, destination: &Path) -> Result<()> {
        let destination = destination.to_path_buf();
        let tree = self.clone();

        let handle = spawn_blocking(move || {
            let size = tree.tree.size();
            debug!(
                "Writing PipTree to disk: {:?}, tree size: {}",
                destination, size
            );

            let file = std::fs::File::create(destination)?;
            let writer = std::io::BufWriter::new(file);
            postcard::to_io(&tree, writer)?;

            Ok(())
        });

        handle.await?
    }

    /// Find all polygons containing a given point.
    pub async fn point_in_polygon(&self, lng: f64, lat: f64) -> Result<Vec<T>> {
        let self_c = self.clone();
        let polygons = self_c
            .geo_point_in_polygon(Point::new(lng, lat))
            .unwrap_or_default();

        Ok(polygons)
    }

    /// Find all polygons containing a given point. Points on a boundary count
    /// as inside, matching SpatiaLite's `INTERSECTS`.
    fn geo_point_in_polygon(&self, point: Point<f64>) -> Option<Vec<T>> {
        let found_ids = self
            .tree
            .locate_in_envelope_intersecting(&AABB::from_point(point))
            .filter(|f| f.geom().intersects(&point))
            .map(|f| f.data.clone())
            .collect::<Vec<_>>();

        if found_ids.is_empty() {
            None
        } else {
            Some(found_ids)
        }
    }

    /// Size of the `PipTree`.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.tree.size()
    }

    /// Is the `PipTree` empty?
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tree.size() == 0
    }
}
