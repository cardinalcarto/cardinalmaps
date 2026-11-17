//! Keeping only what's in a box, exactly, while a search collects.
//!
//! Coverings are coarse (see `geometry::MAX_COVER_LEVEL`), so a box's
//! covering query also matches places near it. Checking them only after the
//! best few are taken would let those places crowd out what's in the box, so
//! the check is made as each match is collected: by its `center` fast field
//! when its shape is its center, and otherwise by its stored shape, read only
//! when its center is outside the box.

use std::sync::Arc;

use geo::{Geometry, Point, Rect};
use tantivy::collector::{Collector, SegmentCollector};
use tantivy::columnar::Column;
use tantivy::schema::{Field, Value};
use tantivy::store::StoreReader;
use tantivy::{DocId, Score, SegmentReader, TantivyDocument};

use crate::geometry;
use crate::index::{FIELD_CENTER, FIELD_GEOMETRY, FIELD_IS_POINT};

/// Store blocks kept decompressed per segment while shapes are read.
const STORE_CACHE_BLOCKS: usize = 8;

/// Collects with `inner` only the matches whose shapes intersect `rects`.
pub(crate) struct Within<C> {
    rects: Arc<Vec<Rect<f64>>>,
    inner: C,
}

impl<C> Within<C> {
    pub(crate) fn new(rects: Arc<Vec<Rect<f64>>>, inner: C) -> Self {
        Self { rects, inner }
    }
}

impl<C: Collector> Collector for Within<C> {
    type Fruit = C::Fruit;
    type Child = WithinSegment<C::Child>;

    fn for_segment(&self, segment_local_id: u32, segment: &SegmentReader) -> tantivy::Result<Self::Child> {
        Ok(WithinSegment {
            inner: self.inner.for_segment(segment_local_id, segment)?,
            rects: self.rects.clone(),
            center: segment.fast_fields().u64(FIELD_CENTER).ok(),
            is_point: segment.fast_fields().u64(FIELD_IS_POINT).ok(),
            geometry: segment.schema().get_field(FIELD_GEOMETRY)?,
            segment: segment.clone(),
            store: None,
            accepted: Vec::new(),
        })
    }

    fn requires_scoring(&self) -> bool {
        self.inner.requires_scoring()
    }

    fn merge_fruits(&self, fruits: Vec<<C::Child as SegmentCollector>::Fruit>) -> tantivy::Result<Self::Fruit> {
        self.inner.merge_fruits(fruits)
    }
}

pub(crate) struct WithinSegment<S> {
    inner: S,
    rects: Arc<Vec<Rect<f64>>>,
    center: Option<Column<u64>>,
    is_point: Option<Column<u64>>,
    geometry: Field,
    segment: SegmentReader,
    /// Opened when a shape is first needed.
    store: Option<StoreReader>,
    accepted: Vec<DocId>,
}

impl<S> WithinSegment<S> {
    fn accepts(&mut self, doc: DocId) -> bool {
        if let Some(center) = self.center.as_ref().and_then(|column| column.first(doc)) {
            let center = s2::latlng::LatLng::from(s2::cellid::CellID(center));
            let center = Geometry::Point(Point::new(center.lng.deg(), center.lat.deg()));
            // A shape's center is in it, so it's in the box if its center is.
            if geometry::intersects_any(&center, &self.rects) {
                return true;
            }
            if self.is_point.as_ref().and_then(|column| column.first(doc)) == Some(1) {
                return false;
            }
        }
        self.stored_shape_intersects(doc)
    }

    /// Whether the stored shape intersects the box. A document that can't be
    /// read is kept, for ranking to fail on as it would have.
    fn stored_shape_intersects(&mut self, doc: DocId) -> bool {
        if self.store.is_none() {
            match self.segment.get_store_reader(STORE_CACHE_BLOCKS) {
                Ok(store) => self.store = Some(store),
                Err(_) => return true,
            }
        }
        let Some(Ok(stored)) = self.store.as_ref().map(|store| store.get::<TantivyDocument>(doc)) else {
            return true;
        };
        match stored.get_first(self.geometry).and_then(|v| v.as_bytes()).map(geometry::from_wkb) {
            Some(Ok(shape)) => geometry::intersects_any(&shape, &self.rects),
            _ => true,
        }
    }
}

impl<S: SegmentCollector> SegmentCollector for WithinSegment<S> {
    type Fruit = S::Fruit;

    fn collect(&mut self, doc: DocId, score: Score) {
        if self.accepts(doc) {
            self.inner.collect(doc, score);
        }
    }

    fn collect_block(&mut self, docs: &[DocId]) {
        let mut accepted = std::mem::take(&mut self.accepted);
        accepted.clear();
        accepted.extend(docs.iter().copied().filter(|&doc| self.accepts(doc)));
        if !accepted.is_empty() {
            self.inner.collect_block(&accepted);
        }
        self.accepted = accepted;
    }

    fn harvest(self) -> Self::Fruit {
        self.inner.harvest()
    }
}
