//! Who's On First places in a bounding box, from a WOF SpatiaLite database
//! read with plain SQLite.
//!
//! A place is in the box if the airmail indexer would index it
//! (`airmail_indexer/src/load_wof.rs`) and the shape it would index meets the
//! box: its `boundary` if it has one, else its label point. Boundaries are
//! tested against the `shard` table, which holds them cut into small
//! polygons with an R-tree over them (`idx_shard_geom`), so a country that
//! merely spans the box costs a few small shards rather than its whole
//! outline. Places without a boundary are found through the R-tree over the
//! `geometry` table (`idx_geometry_geom`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use geo::{Geometry, Intersects, Point, Rect, coord};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use thiserror::Error;

use crate::geojson::GeoJson;
use crate::simplify::{self, Simplification};
use crate::spatialite;

/// Placetypes the indexer skips (`WhosOnFirst::searchable_places`).
pub const EXCLUDED_PLACETYPES: [&str; 4] = ["planet", "timezone", "marketarea", "campus"];

/// The roles of the points that stand for a place, in the order the indexer
/// prefers them.
const LABEL_ROLES: [&str; 3] = ["label_position", "reversegeo_position", "centroid"];

/// Who's On First's country IDs, as `airmail_indexer/src/lib.rs` has them: a
/// place's country is the first of itself and its ancestors in this list.
pub const COUNTRIES: [u64; 214] = [
    85632343, 85632573, 85632229, 85632529, 85632405, 85632773, 85632281, 85632715, 85632505, 85632785, 85632793,
    85632295, 85632717, 85632609, 85632491, 85632475, 85632997, 85632213, 85633001, 85632167, 85632285, 85632247,
    85632749, 85632623, 85633009, 85632339, 85632171, 85632235, 85632395, 85632571, 85633041, 85632643, 85632391,
    85632541, 85633051, 85632449, 85633057, 85632245, 85632695, 85632519, 85632487, 85632675, 85632721, 85632441,
    85632437, 85633105, 85633111, 85632319, 85633121, 85632503, 85632713, 85632451, 85632261, 85633135, 85632581,
    85632217, 85632781, 85633129, 85632257, 85633143, 85632755, 85632431, 85633147, 85632407, 85633159, 85632335,
    85633163, 85632547, 85632189, 85633217, 85632603, 85632691, 85632287, 85633171, 85632385, 85632757, 85632397,
    85632483, 85632323, 85633229, 85632433, 85633237, 85632203, 85633241, 85632315, 85632461, 85632469, 85632191,
    85632361, 85633249, 85633253, 85632593, 85632215, 85632425, 85632429, 85632329, 85632761, 85632359, 85632709,
    85632259, 85632551, 85632639, 85632231, 85632401, 85632307, 85632241, 85632533, 85632369, 85633267, 85632313,
    85632249, 85632173, 85633269, 85633275, 85633279, 85632627, 85632693, 85633285, 85633287, 85632667, 85632223,
    85632663, 85632373, 85632553, 85632181, 85632439, 85632161, 85632679, 85633331, 85632357, 85632305, 85632383,
    85633293, 85632739, 85632729, 85632535, 85632269, 85632735, 85632599, 85633337, 85633341, 85632465, 85632747,
    85633345, 85632207, 85632179, 85632521, 85632347, 85632509, 85632659, 85633723, 85633739, 85633735, 85632331,
    85632355, 85632299, 85633745, 85633755, 85632685, 85632303, 85632253, 85632591, 85632661, 85632751, 85633789,
    85632605, 85633779, 85633769, 85632467, 85633763, 85632365, 85632379, 85632443, 85632657, 85632765, 85632545,
    85632185, 85632413, 85632635, 85632325, 85632647, 85632293, 85632513, 85632583, 85632671, 85632703, 85632455,
    85632393, 85632271, 85632607, 85632403, 85632227, 85633805, 85632625, 102312305, 85633793, 85632511, 85632645,
    85632187, 85632569, 85632317, 85632763, 85632263, 85632681, 85633259, 1, 85632733, 1729945891, 1729945893,
    1729989201, 85632499, 85633813, 85632559, 85632243,
];

#[derive(Debug, Error)]
pub enum Error {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("more than {0} places; ask for a smaller bbox")]
    TooManyPlaces(usize),
    #[error("the query took longer than its limit; ask for a smaller bbox")]
    TooSlow,
}

impl From<rusqlite::types::FromSqlError> for Error {
    fn from(e: rusqlite::types::FromSqlError) -> Self {
        Error::Sqlite(e.into())
    }
}

/// The longitude/latitude box a request asks for. A box whose west is east
/// of its east crosses the antimeridian.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bbox {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl Bbox {
    /// The box as rectangles that don't cross the antimeridian.
    fn rects(&self) -> Vec<Rect<f64>> {
        let rect = |w: f64, e: f64| Rect::new(coord! { x: w, y: self.south }, coord! { x: e, y: self.north });
        if self.west <= self.east {
            vec![rect(self.west, self.east)]
        } else {
            vec![rect(self.west, 180.0), rect(-180.0, self.east)]
        }
    }

    /// Its area in square degrees.
    #[must_use]
    pub fn area(&self) -> f64 {
        self.rects().iter().map(|r| r.width() * r.height()).sum()
    }
}

/// What a query may cost.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The most places (including ancestors) a response holds.
    pub max_places: usize,
    /// When to give up.
    pub deadline: Instant,
}

/// One of a place's names, as `[lang, tag, abbr, name]`: `lang` is ISO
/// 639-3 or `und`, `tag` is `default`, `preferred`, `variant` or
/// `colloquial`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Name(pub String, pub String, pub bool, pub String);

/// A place, with everything the indexer reads of it.
#[derive(Debug, Clone, Serialize)]
pub struct Place {
    pub id: u64,
    pub placetype: String,
    pub names: Vec<Name>,
    /// The places containing it in every branch of its hierarchy, not itself,
    /// in the database's order.
    pub ancestors: Vec<u64>,
    /// Its country: the first of itself and its ancestors that's a country
    /// (`COUNTRIES`), whose `lang_x_spoken` are the languages spoken there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_id: Option<u64>,
    /// `alpha2`, else `wof:country`, as given (the indexer normalizes it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub population: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub population_rank: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang_x_spoken: Option<Vec<String>>,
    /// The first of its label position, reverse geocoding position and
    /// centroid, `[lon, lat]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<[f64; 2]>,
    /// The bounds of its unsimplified shape, `[west, south, east, north]`.
    pub bbox: [f64; 4],
    /// Its boundary, simplified, as a GeoJSON geometry; `null` for a place
    /// that's only a point (its `label`).
    pub geometry: Option<GeoJson>,
    /// Set on places that aren't in the box themselves, but contain a place
    /// that is. They're there for their names and properties; the indexer
    /// wouldn't index them for this box.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ancestor_only: bool,
}

/// An open, read-only database.
pub struct Database {
    connection: Connection,
}

impl Database {
    /// Opens the database at `path` read-only and immutable: nothing else
    /// may write to it while it's open.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let uri = format!("file:{}?immutable=1", path.display());
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.set_prepared_statement_cache_capacity(32);
        let db = Self { connection };
        db.check()?;
        Ok(db)
    }

    /// Fails unless this is a WOF SpatiaLite database with its R-trees.
    pub fn check(&self) -> rusqlite::Result<()> {
        self.connection.query_row("SELECT 1 FROM idx_shard_geom LIMIT 1", [], |_| Ok(())).optional()?;
        self.connection.query_row("SELECT 1 FROM idx_geometry_geom LIMIT 1", [], |_| Ok(())).optional()?;
        self.connection.query_row("SELECT 1 FROM place LIMIT 1", [], |_| Ok(())).optional()?;
        Ok(())
    }

    /// Every place in `bbox` and every ancestor of one, by ID, their shapes
    /// simplified.
    pub fn places(&self, bbox: Bbox, simplification: Simplification, limits: Limits) -> Result<Vec<Place>, Error> {
        let rects = bbox.rects();
        let check_time = || if Instant::now() > limits.deadline { Err(Error::TooSlow) } else { Ok(()) };

        // Places whose boundary meets the box, and those that may be in it by
        // their label point.
        let mut boundaries: BTreeSet<u64> = BTreeSet::new();
        let mut maybe_boundaries: BTreeSet<u64> = BTreeSet::new();
        let mut maybe_points: BTreeSet<u64> = BTreeSet::new();
        for rect in &rects {
            let mut shards = self.connection.prepare_cached(
                "SELECT s.id, s.geom FROM idx_shard_geom r
                 CROSS JOIN shard s ON s.rowid = r.pkid
                 WHERE r.xmin <= ?2 AND r.xmax >= ?1 AND r.ymin <= ?4 AND r.ymax >= ?3
                 AND s.source = 'wof' AND s.role = 'boundary'",
            )?;
            let mut rows = shards.query(params![rect.min().x, rect.max().x, rect.min().y, rect.max().y])?;
            let mut n = 0_u32;
            while let Some(row) = rows.next()? {
                n += 1;
                if n.is_multiple_of(4096) {
                    check_time()?;
                }
                let Some(id) = parse_id(&row.get::<_, Text>(0)?.0) else { continue };
                if boundaries.contains(&id) {
                    continue;
                }
                let Ok(blob) = row.get_ref(1)?.as_blob() else { continue };
                if spatialite::parse(blob).is_ok_and(|shard| rects.iter().any(|r| shard.intersects(r))) {
                    boundaries.insert(id);
                }
            }

            let mut geometries = self.connection.prepare_cached(
                "SELECT g.id, g.role FROM idx_geometry_geom r
                 CROSS JOIN geometry g ON g.rowid = r.pkid
                 WHERE r.xmin <= ?2 AND r.xmax >= ?1 AND r.ymin <= ?4 AND r.ymax >= ?3
                 AND g.source = 'wof'
                 AND g.role IN ('boundary', 'label_position', 'reversegeo_position', 'centroid')",
            )?;
            let mut rows = geometries.query(params![rect.min().x, rect.max().x, rect.min().y, rect.max().y])?;
            while let Some(row) = rows.next()? {
                n += 1;
                if n.is_multiple_of(4096) {
                    check_time()?;
                }
                let Some(id) = parse_id(&row.get::<_, Text>(0)?.0) else { continue };
                if row.get::<_, Text>(1)?.0 == "boundary" {
                    maybe_boundaries.insert(id);
                } else {
                    maybe_points.insert(id);
                }
            }
        }

        let mut places: BTreeMap<u64, Place> = BTreeMap::new();
        let candidates: BTreeSet<u64> =
            boundaries.iter().chain(&maybe_boundaries).chain(&maybe_points).copied().collect();
        for id in candidates {
            check_time()?;
            // A boundary with shards is in the box only if a shard is, and a
            // place with a boundary is indexed by it, not its points.
            if !boundaries.contains(&id) && self.has_shards(id)? {
                continue;
            }
            let Some(placetype) = self.placetype(id)? else { continue };
            if EXCLUDED_PLACETYPES.contains(&placetype.as_str()) {
                continue;
            }
            let names = self.names(id)?;
            if !names.iter().any(|n| !n.2) {
                // The indexer needs a full name to show.
                continue;
            }
            let shape = self.shape(id, true)?;
            let in_box = match &shape.boundary {
                Some(_) if boundaries.contains(&id) => true,
                // A boundary without shards is tested whole.
                Some(boundary) => rects.iter().any(|r| boundary.intersects(r)),
                None => shape.label.is_some_and(|p| rects.iter().any(|r| r.intersects(&p))),
            };
            if !in_box {
                continue;
            }
            places.insert(id, self.place(id, placetype, names, shape, simplification, false)?);
            if places.len() > limits.max_places {
                return Err(Error::TooManyPlaces(limits.max_places));
            }
        }

        // Ancestors not in the box themselves.
        let missing: BTreeSet<u64> =
            places.values().flat_map(|p| p.ancestors.iter().copied()).filter(|id| !places.contains_key(id)).collect();
        for id in missing {
            check_time()?;
            let Some(placetype) = self.placetype(id)? else { continue };
            let names = self.names(id)?;
            // Its outline isn't needed, only its bounds.
            let shape = self.shape(id, false)?;
            places.insert(id, self.place(id, placetype, names, shape, simplification, true)?);
            if places.len() > limits.max_places {
                return Err(Error::TooManyPlaces(limits.max_places));
            }
        }
        Ok(places.into_values().collect())
    }

    fn place(
        &self,
        id: u64,
        placetype: String,
        names: Vec<Name>,
        shape: Shape,
        simplification: Simplification,
        ancestor_only: bool,
    ) -> Result<Place, Error> {
        let ancestors = self.ancestors(id)?;
        let mut properties = self.properties(id)?;
        let country_id = std::iter::once(&id).chain(&ancestors).find(|a| COUNTRIES.contains(a)).copied();
        let country = properties.remove("alpha2").or_else(|| properties.remove("wof:country"));
        let population = properties.get("wof:population").and_then(|p| p.parse::<f64>().ok()).map(|p| p as u64);
        let population_rank = properties.get("wof:population_rank").and_then(|r| r.parse().ok());
        let lang_x_spoken = properties
            .get("wof:lang_x_spoken")
            .map(|langs| langs.split(',').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect());
        let label = shape.label.map(|p| [p.x(), p.y()]);
        let bbox = shape.bounds.or(label.map(|[x, y]| [x, y, x, y])).unwrap_or_default();
        let geometry = shape.boundary.map(|b| GeoJson(simplify::simplify(&b, simplification)));
        Ok(Place {
            id,
            placetype,
            names,
            ancestors,
            country_id,
            country,
            population,
            population_rank,
            lang_x_spoken,
            label,
            bbox,
            geometry,
            ancestor_only,
        })
    }

    fn placetype(&self, id: u64) -> rusqlite::Result<Option<String>> {
        self.connection
            .prepare_cached("SELECT type FROM place WHERE source = 'wof' AND id = ?1")?
            .query_row([id.to_string()], |row| row.get::<_, Text>(0))
            .optional()
            .map(|t| t.map(|t| t.0))
    }

    fn names(&self, id: u64) -> rusqlite::Result<Vec<Name>> {
        let mut query = self
            .connection
            .prepare_cached("SELECT lang, tag, abbr, name FROM name WHERE source = 'wof' AND id = ?1")?;
        let names = query
            .query_map([id.to_string()], |row| {
                let abbr = row.get::<_, Text>(2)?.0;
                Ok(Name(
                    row.get::<_, Text>(0)?.0,
                    row.get::<_, Text>(1)?.0,
                    abbr != "0" && !abbr.is_empty(),
                    row.get::<_, Text>(3)?.0,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(names.into_iter().filter(|n| !n.3.is_empty()).collect())
    }

    fn ancestors(&self, id: u64) -> rusqlite::Result<Vec<u64>> {
        let mut query = self.connection.prepare_cached(
            "SELECT DISTINCT parent_id FROM hierarchy
             WHERE child_source = 'wof' AND child_id = ?1 AND parent_id != child_id",
        )?;
        let ids = query
            .query_map([id.to_string()], |row| row.get::<_, Text>(0).map(|t| t.0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids.iter().filter_map(|id| parse_id(id)).collect())
    }

    fn properties(&self, id: u64) -> rusqlite::Result<BTreeMap<String, String>> {
        let mut query = self.connection.prepare_cached(
            "SELECT key, value FROM property WHERE source = 'wof' AND id = ?1
             AND key IN ('wof:population', 'wof:population_rank', 'alpha2', 'wof:country', 'wof:lang_x_spoken')",
        )?;
        query.query_map([id.to_string()], |row| Ok((row.get::<_, Text>(0)?.0, row.get::<_, Text>(1)?.0)))?.collect()
    }

    fn has_shards(&self, id: u64) -> rusqlite::Result<bool> {
        self.connection
            .prepare_cached("SELECT 1 FROM shard WHERE source = 'wof' AND id = ?1 AND role = 'boundary' LIMIT 1")?
            .query_row([id.to_string()], |_| Ok(()))
            .optional()
            .map(|row| row.is_some())
    }

    /// A place's boundary (if `with_boundary`), its bounds, and its label
    /// point. A boundary that can't be read is left out, as the indexer
    /// leaves it out.
    fn shape(&self, id: u64, with_boundary: bool) -> rusqlite::Result<Shape> {
        let mut query = self.connection.prepare_cached(
            "SELECT role, geom FROM geometry WHERE source = 'wof' AND id = ?1
             AND role IN ('boundary', 'label_position', 'reversegeo_position', 'centroid')",
        )?;
        let mut rows = query.query([id.to_string()])?;
        let mut shape = Shape::default();
        let mut labels: [Option<Point<f64>>; 3] = [None; 3];
        while let Some(row) = rows.next()? {
            let role = row.get::<_, Text>(0)?.0;
            let Ok(blob) = row.get_ref(1)?.as_blob() else { continue };
            if role == "boundary" {
                if !with_boundary {
                    shape.bounds = spatialite::bounds(blob).ok();
                } else if let Ok(geometry) = spatialite::parse(blob) {
                    shape.bounds = spatialite::bounds(blob).ok();
                    shape.boundary = Some(geometry);
                }
            } else if let Some(i) = LABEL_ROLES.iter().position(|r| *r == role)
                && let Ok(Geometry::Point(point)) = spatialite::parse(blob)
            {
                labels[i] = Some(point);
            }
        }
        shape.label = labels.into_iter().flatten().next();
        Ok(shape)
    }
}

#[derive(Default)]
struct Shape {
    boundary: Option<Geometry<f64>>,
    bounds: Option<[f64; 4]>,
    label: Option<Point<f64>>,
}

/// A value as text, whatever SQLite stored it as: the WOF database's text
/// columns hold numbers as integers or reals here and there.
struct Text(String);

impl rusqlite::types::FromSql for Text {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        use rusqlite::types::{FromSqlError, ValueRef};
        Ok(Text(match value {
            ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
            ValueRef::Integer(i) => i.to_string(),
            ValueRef::Real(r) => r.to_string(),
            ValueRef::Null => String::new(),
            ValueRef::Blob(_) => return Err(FromSqlError::InvalidType),
        }))
    }
}

fn parse_id(id: &str) -> Option<u64> {
    id.parse().ok()
}
