//! Point clustering, as [supercluster](https://github.com/mapbox/supercluster) does it for
//! MapLibre's GeoJSON sources.
//!
//! From the deepest level up, each point (or cluster from the level below) absorbs its
//! unvisited neighbours within the cluster radius into a cluster at their weighted center,
//! if together they have at least `clusterMinPoints` points. Each level is a [`KdBush`] for
//! cutting tiles. `clusterProperties` are mapped from each point and reduced into clusters
//! with expressions, MapLibre's way.

use std::sync::Arc;

use maplibre_expr::{EvaluationContext, Expr, Value};
use mlt_core::geo_types::{Coord, Geometry, Point};
use serde_json::Value as Json;

use super::kdbush::KdBush;
use super::{Properties, TileFeature};
use crate::Error;
use crate::TileId;

/// The property name `["accumulated"]` is read from while reducing; MapLibre's evaluation
/// context has a slot for it, `maplibre_expr`'s doesn't, so reduce expressions read it as a
/// feature property instead.
const ACCUMULATED: &str = "\u{0}accumulated";

/// Tile coordinates the cluster radius is measured in, as MapLibre's tile size.
const RADIUS_EXTENT: f64 = 512.0;

#[derive(Debug, Clone)]
pub(crate) struct ClusterOptions {
    /// The deepest level points are clustered at; deeper tiles show every point.
    pub max_zoom: u8,
    pub min_points: u32,
    /// In pixels of a 512 pixel tile.
    pub radius: f64,
    /// Tile coordinates across an output tile.
    pub extent: u32,
    pub properties: Option<ClusterProperties>,
}

/// `clusterProperties`: per property, an expression mapping a point's properties to a
/// value and one reducing two values into one.
#[derive(Debug, Clone)]
pub(crate) struct ClusterProperties {
    names: Vec<String>,
    map: Vec<Expr>,
    reduce: Vec<Expr>,
}

impl ClusterProperties {
    /// Parse `{"name": [operator, map expression], ...}`, where the operator is a name
    /// such as `"+"` (reducing with `[operator, ["accumulated"], ["get", name]]`) or a
    /// whole expression reading `["accumulated"]`.
    pub fn parse(json: &Json) -> Result<Self, Error> {
        let invalid = |name: &str, message: String| {
            Error::GeoJson(format!("clusterProperties.{name}: {message}"))
        };
        let object = json
            .as_object()
            .ok_or_else(|| Error::GeoJson("clusterProperties must be an object".into()))?;
        let mut out = Self {
            names: Vec::new(),
            map: Vec::new(),
            reduce: Vec::new(),
        };
        for (name, value) in object {
            let Some([operator, map]) = value
                .as_array()
                .and_then(|a| <&[Json; 2]>::try_from(a.as_slice()).ok())
            else {
                return Err(invalid(name, "must be [operator, map expression]".into()));
            };
            let reduce = match operator {
                Json::String(op) => {
                    serde_json::json!([op, ["accumulated"], ["get", name]])
                }
                expr => expr.clone(),
            };
            let parse = |json: &Json| {
                maplibre_expr::parse(&replace_accumulated(json))
                    .map_err(|e| e.to_string())
                    .and_then(|e| {
                        maplibre_expr::typecheck(&e, None, false).map_err(|e| e.to_string())
                    })
                    .map_err(|e| invalid(name, e))
            };
            out.map.push(parse(map)?);
            out.reduce.push(parse(&reduce)?);
            out.names.push(name.clone());
        }
        Ok(out)
    }

    fn map(&self, ctx: &mut EvaluationContext, point: &Properties) -> Properties {
        ctx.feature.properties.clone_from(point);
        self.names
            .iter()
            .zip(&self.map)
            .map(|(name, expr)| {
                let value = maplibre_expr::evaluate(expr, ctx).unwrap_or(Value::Null);
                (name.clone(), value)
            })
            .collect()
    }

    fn reduce(
        &self,
        ctx: &mut EvaluationContext,
        accumulated: &mut Properties,
        other: &Properties,
    ) {
        ctx.feature.properties.clone_from(other);
        for (name, expr) in self.names.iter().zip(&self.reduce) {
            let current = accumulated.get(name).cloned().unwrap_or(Value::Null);
            ctx.feature.properties.insert(ACCUMULATED.into(), current);
            let value = maplibre_expr::evaluate(expr, ctx).unwrap_or(Value::Null);
            accumulated.insert(name.clone(), value);
        }
    }
}

/// `["accumulated"]` as a read of the [`ACCUMULATED`] property.
fn replace_accumulated(json: &Json) -> Json {
    match json {
        Json::Array(items) if items.len() == 1 && items[0] == "accumulated" => {
            serde_json::json!(["get", ACCUMULATED])
        }
        // Literals are left alone.
        Json::Array(items) if items.first().is_some_and(|op| op == "literal") => json.clone(),
        Json::Array(items) => Json::Array(items.iter().map(replace_accumulated).collect()),
        other => other.clone(),
    }
}

/// A point or cluster at one zoom level.
#[derive(Debug, Clone)]
struct Node {
    /// World coordinates.
    position: [f64; 2],
    /// The zoom level this was last visited at while clustering the level above.
    visited: Option<u8>,
    /// The point's index in the input, or the cluster's id.
    id: u64,
    /// The cluster that absorbed this one at the level above.
    parent: Option<u64>,
    num_points: u32,
    /// Index into `Supercluster::cluster_properties` for clusters with mapped properties.
    properties: Option<usize>,
    is_cluster: bool,
}

struct Level {
    index: KdBush,
    nodes: Vec<Node>,
}

/// A cluster or point that a cluster splits into one zoom level down.
#[derive(Debug, Clone)]
pub struct ClusterChild {
    pub id: Option<u64>,
    pub is_cluster: bool,
    pub properties: Arc<Properties>,
    /// `[lng, lat]`
    pub position: [f64; 2],
}

pub(crate) struct Supercluster {
    options: ClusterOptions,
    /// `(id, properties)` of each input point.
    points: Vec<(Option<u64>, Arc<Properties>)>,
    /// One per zoom level `0..=max_zoom + 1`; the last holds the unclustered points.
    levels: Vec<Level>,
    cluster_properties: Vec<Properties>,
}

impl Supercluster {
    /// Cluster `points`: `(id, properties, world position)`.
    pub fn new(
        points: Vec<(Option<u64>, Arc<Properties>, [f64; 2])>,
        options: ClusterOptions,
    ) -> Self {
        let nodes: Vec<Node> = points
            .iter()
            .enumerate()
            .map(|(i, (_, _, position))| Node {
                position: *position,
                visited: None,
                id: i as u64,
                parent: None,
                num_points: 1,
                properties: None,
                is_cluster: false,
            })
            .collect();
        let mut this = Self {
            points: points.into_iter().map(|(id, p, _)| (id, p)).collect(),
            levels: Vec::new(),
            cluster_properties: Vec::new(),
            options,
        };
        let deepest = Level {
            index: KdBush::new(nodes.iter().map(|n| n.position)),
            nodes,
        };
        let mut levels = vec![deepest];
        let mut ctx = EvaluationContext::new();
        for z in (0..=this.options.max_zoom).rev() {
            let above = levels.last_mut().unwrap();
            let nodes = this.cluster(above, z, &mut ctx);
            levels.push(Level {
                index: KdBush::new(nodes.iter().map(|n| n.position)),
                nodes,
            });
        }
        levels.reverse();
        this.levels = levels;
        this
    }

    /// Cluster the level above `zoom` into the nodes of `zoom`.
    fn cluster(&mut self, above: &mut Level, zoom: u8, ctx: &mut EvaluationContext) -> Vec<Node> {
        let r = self.options.radius / (RADIUS_EXTENT * f64::from(zoom).exp2());
        let min_points = self.options.min_points;
        let mut next = Vec::new();
        let mut neighbours = Vec::new();
        let unvisited = |n: &Node| n.visited.is_none_or(|v| v > zoom);
        for i in 0..above.nodes.len() {
            if !unvisited(&above.nodes[i]) {
                continue;
            }
            above.nodes[i].visited = Some(zoom);
            let origin = above.nodes[i].clone();
            neighbours.clear();
            above.index.within(origin.position, r, &mut neighbours);
            let num_points = origin.num_points
                + neighbours
                    .iter()
                    .map(|&n| &above.nodes[n as usize])
                    .filter(|n| unvisited(n))
                    .map(|n| n.num_points)
                    .sum::<u32>();

            if num_points > origin.num_points && num_points >= min_points {
                let weight = f64::from(origin.num_points);
                let mut center = origin.position.map(|c| c * weight);
                // Unique across levels: the node's index, then the level.
                let id = ((i as u64) << 5) + u64::from(zoom) + 1 + self.points.len() as u64;
                let mut properties: Option<Properties> = None;
                for &n in &neighbours {
                    let node = &mut above.nodes[n as usize];
                    if !unvisited(node) {
                        continue;
                    }
                    node.visited = Some(zoom);
                    node.parent = Some(id);
                    let weight = f64::from(node.num_points);
                    center[0] += node.position[0] * weight;
                    center[1] += node.position[1] * weight;
                    if let Some(cp) = &self.options.properties {
                        let node = node.clone();
                        let accumulated =
                            properties.get_or_insert_with(|| self.mapped(cp, &origin, ctx));
                        let other = self.mapped(cp, &node, ctx);
                        cp.reduce(ctx, accumulated, &other);
                    }
                }
                above.nodes[i].parent = Some(id);
                let properties = properties.map(|p| {
                    self.cluster_properties.push(p);
                    self.cluster_properties.len() - 1
                });
                next.push(Node {
                    position: center.map(|c| c / f64::from(num_points)),
                    visited: None,
                    id,
                    parent: None,
                    num_points,
                    properties,
                    is_cluster: true,
                });
            } else {
                next.push(Node {
                    visited: None,
                    ..origin
                });
                if num_points > 1 {
                    for &n in &neighbours {
                        let node = &mut above.nodes[n as usize];
                        if unvisited(node) {
                            node.visited = Some(zoom);
                            next.push(Node {
                                visited: None,
                                ..node.clone()
                            });
                        }
                    }
                }
            }
        }
        next
    }

    /// The mapped cluster properties of a node: a cluster's accumulated ones, or a point's
    /// mapped from its properties.
    fn mapped(
        &self,
        cp: &ClusterProperties,
        node: &Node,
        ctx: &mut EvaluationContext,
    ) -> Properties {
        match node.properties {
            Some(i) => self.cluster_properties[i].clone(),
            None if node.is_cluster => Properties::new(),
            None => cp.map(ctx, &self.points[node.id as usize].1),
        }
    }

    /// The points and clusters of a tile, with a buffer of the cluster radius around it.
    pub fn tile(&self, id: TileId, out: &mut Vec<TileFeature>) {
        let level = &self.levels[usize::from(id.z.min(self.options.max_zoom + 1))];
        let scale = f64::from(id.z).exp2();
        let p = self.options.radius / RADIUS_EXTENT;
        let (x, y) = (f64::from(id.x), f64::from(id.y));
        let (top, bottom) = ((y - p) / scale, (y + 1.0 + p) / scale);
        let mut found = Vec::new();
        level.index.range(
            [(x - p) / scale, top],
            [(x + 1.0 + p) / scale, bottom],
            &mut found,
        );
        self.add_features(level, &found, x, y, scale, out);
        // Points across the antimeridian, for the tiles at the world's edges.
        if id.x == 0 {
            found.clear();
            level
                .index
                .range([1.0 - p / scale, top], [1.0, bottom], &mut found);
            self.add_features(level, &found, scale, y, scale, out);
        }
        if f64::from(id.x) == scale - 1.0 {
            found.clear();
            level
                .index
                .range([0.0, top], [p / scale, bottom], &mut found);
            self.add_features(level, &found, -1.0, y, scale, out);
        }
    }

    fn add_features(
        &self,
        level: &Level,
        found: &[u32],
        x: f64,
        y: f64,
        scale: f64,
        out: &mut Vec<TileFeature>,
    ) {
        let extent = f64::from(self.options.extent);
        for &i in found {
            let node = &level.nodes[i as usize];
            let coord = Coord {
                x: (extent * (node.position[0] * scale - x)).round() as i32,
                y: (extent * (node.position[1] * scale - y)).round() as i32,
            };
            let (id, properties) = self.node_properties(node);
            out.push(TileFeature {
                id,
                geometry: Geometry::Point(Point(coord)),
                properties,
                line_clip: None,
            });
        }
    }

    fn node_properties(&self, node: &Node) -> (Option<u64>, Arc<Properties>) {
        if !node.is_cluster {
            let (id, properties) = &self.points[node.id as usize];
            return (*id, properties.clone());
        }
        let mut properties = node
            .properties
            .map(|i| self.cluster_properties[i].clone())
            .unwrap_or_default();
        let count = node.num_points;
        properties.insert("cluster".into(), Value::Bool(true));
        properties.insert("cluster_id".into(), Value::Number(node.id as f64));
        properties.insert("point_count".into(), Value::Number(count.into()));
        properties.insert(
            "point_count_abbreviated".into(),
            Value::String(abbreviate(count)),
        );
        (Some(node.id), Arc::new(properties))
    }

    /// The level a cluster was made at, and its node's index in the level above.
    fn origin(&self, cluster_id: u64) -> Option<(usize, usize)> {
        let offset = cluster_id.checked_sub(self.points.len() as u64)?;
        let zoom = (offset % 32) as usize;
        let index = (offset >> 5) as usize;
        (zoom >= 1 && zoom < self.levels.len() && index < self.levels[zoom].nodes.len())
            .then_some((zoom, index))
    }

    /// What a cluster splits into at the next zoom level, or `None` for an unknown id.
    pub fn children(&self, cluster_id: u64) -> Option<Vec<ClusterChild>> {
        let (origin_zoom, index) = self.origin(cluster_id)?;
        let level = &self.levels[origin_zoom];
        let r = self.options.radius / (RADIUS_EXTENT * ((origin_zoom - 1) as f64).exp2());
        let mut found = Vec::new();
        level
            .index
            .within(level.nodes[index].position, r, &mut found);
        let children = found
            .into_iter()
            .map(|i| &level.nodes[i as usize])
            .filter(|n| n.parent == Some(cluster_id))
            .map(|n| {
                let (id, properties) = self.node_properties(n);
                ClusterChild {
                    id,
                    is_cluster: n.is_cluster,
                    properties,
                    position: world_to_lng_lat(n.position),
                }
            })
            .collect();
        Some(children)
    }

    /// The zoom level at which a cluster splits into more than one child.
    pub fn expansion_zoom(&self, mut cluster_id: u64) -> Option<u8> {
        let mut zoom = self.origin(cluster_id)?.0 as u8 - 1;
        while zoom <= self.options.max_zoom {
            let children = self.children(cluster_id)?;
            zoom += 1;
            match children.as_slice() {
                [only] if only.is_cluster => cluster_id = only.id?,
                _ => break,
            }
        }
        Some(zoom)
    }
}

/// `point_count_abbreviated`: `"1.2k"` from 1000 on, `"12k"` from 10000 on.
fn abbreviate(count: u32) -> String {
    if count >= 10_000 {
        format!("{}k", (f64::from(count) / 1000.0).round())
    } else if count >= 1000 {
        let tenths = (f64::from(count) / 100.0).round() / 10.0;
        format!("{}k", maplibre_expr::format_number(tenths))
    } else {
        count.to_string()
    }
}

pub(crate) fn world_to_lng_lat([x, y]: [f64; 2]) -> [f64; 2] {
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y))
        .sinh()
        .atan()
        .to_degrees();
    [x * 360.0 - 180.0, lat]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn options(properties: Option<ClusterProperties>) -> ClusterOptions {
        ClusterOptions {
            max_zoom: 16,
            min_points: 2,
            radius: 50.0,
            extent: 8192,
            properties,
        }
    }

    /// `n` points in a small cluster near the world's center, each with `value = i`.
    fn points(n: usize) -> Vec<(Option<u64>, Arc<Properties>, [f64; 2])> {
        (0..n)
            .map(|i| {
                let properties = Properties::from([("value".into(), Value::Number(i as f64))]);
                let position = [0.5 + i as f64 * 1e-6, 0.5 + (i % 3) as f64 * 1e-6];
                (Some(i as u64 + 100), Arc::new(properties), position)
            })
            .collect()
    }

    /// Every feature of the tiles around the world's center, where the test points are.
    fn all(cluster: &Supercluster, z: u8) -> Vec<TileFeature> {
        let n = 1u32 << z;
        let near = (n / 2).saturating_sub(2)..(n / 2 + 2).min(n);
        let mut out = Vec::new();
        for x in near.clone() {
            for y in near.clone() {
                let mut tile = Vec::new();
                cluster.tile(TileId::new(z, x, y), &mut tile);
                // Only count each feature in the tile that contains it.
                out.extend(tile.into_iter().filter(|f| {
                    let Geometry::Point(p) = f.geometry else {
                        return false;
                    };
                    (0..8192).contains(&p.0.x) && (0..8192).contains(&p.0.y)
                }));
            }
        }
        out
    }

    #[test]
    fn clusters_at_low_zoom_and_splits_at_high_zoom() {
        let cp = ClusterProperties::parse(&json!({
            "sum": ["+", ["get", "value"]],
            "max": [["max", ["accumulated"], ["get", "max"]], ["get", "value"]],
        }))
        .unwrap();
        let cluster = Supercluster::new(points(10), options(Some(cp)));
        let low = all(&cluster, 2);
        assert_eq!(low.len(), 1, "{low:?}");
        let props = &low[0].properties;
        assert_eq!(props["cluster"], Value::Bool(true));
        assert_eq!(props["point_count"], Value::Number(10.0));
        assert_eq!(props["point_count_abbreviated"], Value::String("10".into()));
        assert_eq!(props["sum"], Value::Number(45.0));
        assert_eq!(props["max"], Value::Number(9.0));
        let id = low[0].id.unwrap();
        assert_eq!(props["cluster_id"], Value::Number(id as f64));

        // Past the max zoom, every point is itself with its own id and properties.
        let deep = all(&cluster, 17);
        assert_eq!(deep.len(), 10);
        assert!(deep.iter().all(|f| f.id.unwrap() >= 100));
        assert!(!deep[0].properties.contains_key("cluster"));

        let zoom = cluster.expansion_zoom(id).unwrap();
        assert!(zoom > 2 && zoom <= 17, "{zoom}");
        let children = cluster.children(id).unwrap();
        let count: f64 = children
            .iter()
            .map(|c| match c.properties.get("point_count") {
                Some(Value::Number(n)) => *n,
                _ => 1.0,
            })
            .sum();
        assert_eq!(count, 10.0);
    }

    #[test]
    fn min_points() {
        let cluster = Supercluster::new(
            points(3),
            ClusterOptions {
                min_points: 4,
                ..options(None)
            },
        );
        assert_eq!(all(&cluster, 0).len(), 3);
    }

    #[test]
    fn far_points_stay_apart() {
        let mut pts = points(2);
        pts[1].2 = [0.25, 0.25];
        let cluster = Supercluster::new(pts, options(None));
        assert_eq!(all(&cluster, 3).len(), 2);
    }

    #[test]
    fn abbreviations() {
        assert_eq!(abbreviate(999), "999");
        assert_eq!(abbreviate(1234), "1.2k");
        assert_eq!(abbreviate(15_678), "16k");
    }
}
