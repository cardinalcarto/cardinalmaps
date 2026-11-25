//! Assembling OSM multipolygon relations from their member ways.

use std::collections::HashMap;

use geo::{Area, Contains, Coord, LineString, MultiPolygon, Point, Polygon};

/// Whether a member way bounds the area from outside or a hole in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Outer,
    Inner,
}

impl Role {
    /// Members without a role are outer, as most consumers treat them.
    #[must_use]
    pub fn parse(role: &str) -> Option<Self> {
        match role {
            "outer" | "" => Some(Role::Outer),
            "inner" => Some(Role::Inner),
            _ => None,
        }
    }
}

/// Builds the relation's shape from its member ways, `(lon, lat)` points
/// each, or `None` if no outer ring closes. Ways that don't join into a
/// closed ring are dropped.
#[must_use]
pub fn assemble(members: Vec<(Role, Vec<(f64, f64)>)>) -> Option<MultiPolygon<f64>> {
    let (outer, inner): (Vec<_>, Vec<_>) = members
        .into_iter()
        .partition(|(role, _)| *role == Role::Outer);
    let to_lines = |members: Vec<(Role, Vec<(f64, f64)>)>| -> Vec<Vec<Coord<f64>>> {
        members
            .into_iter()
            .map(|(_, points)| {
                points
                    .into_iter()
                    .map(|(x, y)| Coord { x, y })
                    .collect::<Vec<_>>()
            })
            .filter(|line| line.len() >= 2)
            .collect()
    };
    let mut polygons: Vec<Polygon<f64>> = rings(to_lines(outer))
        .into_iter()
        .map(|ring| Polygon::new(ring, vec![]))
        .collect();
    if polygons.is_empty() {
        return None;
    }
    // Put each hole in the smallest outer ring containing it.
    polygons.sort_by(|a, b| a.unsigned_area().total_cmp(&b.unsigned_area()));
    let mut holes: Vec<Vec<LineString<f64>>> = vec![Vec::new(); polygons.len()];
    for ring in rings(to_lines(inner)) {
        let first = Point(ring.0[0]);
        if let Some(i) = polygons
            .iter()
            .position(|p| p.contains(&first) || p.exterior().contains(&first))
        {
            holes[i].push(ring);
        }
    }
    let polygons = polygons
        .into_iter()
        .zip(holes)
        .map(|(polygon, holes)| Polygon::new(polygon.exterior().clone(), holes))
        .collect();
    Some(MultiPolygon(polygons))
}

fn key(c: Coord<f64>) -> (u64, u64) {
    (c.x.to_bits(), c.y.to_bits())
}

/// Joins lines end to end into closed rings.
fn rings(lines: Vec<Vec<Coord<f64>>>) -> Vec<LineString<f64>> {
    let mut lines: Vec<Option<Vec<Coord<f64>>>> = lines.into_iter().map(Some).collect();
    // Unused lines by each of their endpoints.
    let mut by_end: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        let line = line.as_ref().unwrap();
        by_end.entry(key(line[0])).or_default().push(i);
        by_end
            .entry(key(*line.last().unwrap()))
            .or_default()
            .push(i);
    }

    let mut rings = Vec::new();
    for start in 0..lines.len() {
        let Some(mut ring) = lines[start].take() else {
            continue;
        };
        while ring.first() != ring.last() {
            let end = key(*ring.last().unwrap());
            let next = by_end
                .get(&end)
                .and_then(|candidates| candidates.iter().copied().find(|&i| lines[i].is_some()));
            let Some(next) = next else {
                break;
            };
            let mut line = lines[next].take().unwrap();
            if key(line[0]) != end {
                line.reverse();
            }
            ring.extend(line.into_iter().skip(1));
        }
        if ring.first() == ring.last() && ring.len() >= 4 {
            rings.push(LineString(ring));
        }
    }
    rings
}

#[cfg(test)]
mod test {
    use super::*;

    fn square(x: f64, y: f64, size: f64) -> Vec<(f64, f64)> {
        vec![
            (x, y),
            (x + size, y),
            (x + size, y + size),
            (x, y + size),
            (x, y),
        ]
    }

    #[test]
    fn joins_ways_into_rings_in_any_direction() {
        let members = vec![
            (Role::Outer, vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]),
            // Reversed relative to the first.
            (Role::Outer, vec![(0.0, 0.0), (0.0, 10.0)]),
            (Role::Outer, vec![(10.0, 10.0), (0.0, 10.0)]),
        ];
        let shape = assemble(members).unwrap();
        assert_eq!(shape.0.len(), 1);
        assert_eq!(shape.0[0].unsigned_area(), 100.0);
    }

    #[test]
    fn puts_holes_in_their_outer_ring() {
        let members = vec![
            (Role::Outer, square(0.0, 0.0, 10.0)),
            (Role::Outer, square(20.0, 0.0, 10.0)),
            (Role::Inner, square(22.0, 2.0, 2.0)),
        ];
        let shape = assemble(members).unwrap();
        assert_eq!(shape.0.len(), 2);
        let with_hole = shape.0.iter().find(|p| !p.interiors().is_empty()).unwrap();
        assert!(with_hole.contains(&Point::new(25.0, 5.0)));
        assert_eq!(with_hole.unsigned_area(), 96.0);
    }

    #[test]
    fn drops_rings_that_dont_close() {
        let members = vec![(Role::Outer, vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)])];
        assert!(assemble(members).is_none());
    }
}
