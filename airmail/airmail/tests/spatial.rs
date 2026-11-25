//! Searching in a box: coverings are coarse, so places near a small box match
//! its covering query, and mustn't crowd out the places in it.

use airmail::geometry::{self, MAX_COVER_LEVEL};
use airmail::index::{AirmailIndex, SearchRequest};
use airmail::poi::{Kind, SchemafiedPoi, ToIndexPoi};
use geo::{Geometry, LineString, Point, Polygon};
use s2::{cell::Cell, cellid::CellID, latlng::LatLng};

fn cafe(shape: Geometry<f64>, importance: f64) -> ToIndexPoi {
    let tags = vec![
        ("amenity".to_string(), "cafe".to_string()),
        ("name".to_string(), "Corner Cafe".to_string()),
    ];
    let mut doc = ToIndexPoi::new(Kind::Poi, vec!["Corner Cafe".to_string()], shape, tags).unwrap();
    doc.importance = importance;
    doc
}

fn square(x: f64, y: f64, half: f64) -> Geometry<f64> {
    let ring = LineString::from(vec![
        (x - half, y - half),
        (x + half, y - half),
        (x + half, y + half),
        (x - half, y + half),
        (x - half, y - half),
    ]);
    Geometry::Polygon(Polygon::new(ring, vec![]))
}

/// A box of about 30 m inside one covering cell, a few places in it, and many
/// more important places in that cell but outside the box.
#[test]
fn places_near_a_small_box_dont_crowd_out_those_in_it() {
    let cell =
        Cell::from(CellID::from(LatLng::from_degrees(47.61, -122.34)).parent(MAX_COVER_LEVEL));
    let bound = cell.rect_bound();
    let (west, south, east, north) = (
        bound.lo().lng.deg(),
        bound.lo().lat.deg(),
        bound.hi().lng.deg(),
        bound.hi().lat.deg(),
    );
    let center = LatLng::from(cell.center());
    let (cx, cy) = (center.lng.deg(), center.lat.deg());
    let half = 0.0002;
    let bbox = [cx - half, cy - half, cx + half, cy + half];

    let mut docs = Vec::new();
    for i in 0..3 {
        docs.push(cafe(
            Geometry::Point(Point::new(cx - half / 2.0 + f64::from(i) * half / 2.0, cy)),
            0.01,
        ));
    }
    // A square whose center is outside the box but which overlaps it, found
    // by its stored shape.
    docs.push(cafe(square(cx + half * 2.0, cy, half * 1.5), 0.01));
    // A square near the box.
    docs.push(cafe(square(cx, cy + half * 2.5, half / 2.0), 0.9));
    // Places on a grid over the cell, outside the box.
    let mut outside = 0;
    for i in 0..20 {
        for j in 0..20 {
            let x = west + (east - west) * f64::from(i) / 20.0;
            let y = south + (north - south) * f64::from(j) / 20.0;
            let in_box = (x - cx).abs() <= half * 1.5 && (y - cy).abs() <= half * 1.5;
            if !in_box
                && outside < 60
                && cell.contains_point(&s2::point::Point::from(LatLng::from_degrees(y, x)))
            {
                docs.push(cafe(Geometry::Point(Point::new(x, y)), 0.9));
                outside += 1;
            }
        }
    }
    assert_eq!(outside, 60);

    let dir = tempfile::tempdir().unwrap();
    let mut index = AirmailIndex::create(dir.path()).unwrap();
    let mut writer = index.writer().unwrap();
    for doc in docs {
        writer.add_poi(SchemafiedPoi::from(doc), "test").unwrap();
    }
    writer.commit().unwrap();

    let search = |bbox: Option<[f64; 4]>, limit: usize| {
        index
            .search(&SearchRequest {
                text: "corner cafe",
                bbox,
                limit,
                ..SearchRequest::default()
            })
            .unwrap()
            .hits
    };
    // Without a box, the important places outside it come first.
    let all = search(None, 100);
    assert_eq!(all.len(), 65);
    assert!(all[..30].iter().all(|hit| hit.importance > 0.5));

    let rects = geometry::bbox_rects(bbox);
    assert_eq!(search(Some(bbox), 3).len(), 3);
    let hits = search(Some(bbox), 10);
    assert_eq!(
        hits.len(),
        4,
        "{:?}",
        hits.iter().map(|h| h.center).collect::<Vec<_>>()
    );
    assert!(
        hits.iter()
            .all(|hit| geometry::intersects_any(&hit.geometry, &rects))
    );
    assert!(
        hits.iter()
            .any(|hit| matches!(hit.geometry, Geometry::Polygon(_))),
        "the overlapping square"
    );
}
