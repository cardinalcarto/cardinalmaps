//! Real streets: planning with `valinor-walk` over Valhalla tiles.
//!
//! [`StreetPlanner`] opens a tile set beside a [`TransitState`] and a [`TransitBuild`]'s walks,
//! keeps a pool of per-thread search
//! workers, and answers requests with real walking and cycling legs:
//! stop tables from [`valinor_walk::StopTables`], transfers from [`valinor_walk::TableTransfers`],
//! and shapes and instructions by converting through [`valinor_walk::paths`] to Thor's
//! [`Paths`](valinor_thor::Paths), [`Thor::build_trip`](valinor_thor::Thor::build_trip), and
//! Ilúvatar's narration.
//!
//! The planner, like [`TransitState`], has no runtime and reads no clock.

use core::fmt;
use core::num::NonZeroUsize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use illuvatar::Odin;
use jiff::Timestamp;
use self_cell::self_cell;
use thiserror::Error;
use valhalla_graphtile::GraphId;
use valhalla_proto::costing::Type as CostingType;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::location::{HasMinimumReachability, HasSearchCutoff};
use valhalla_proto::options::HasLanguage;
use valhalla_proto::{Api, Costing as CostingProto, LatLng, Location, Options};
use valhalla_response::ErrorCode;
use valinor_gtfs::{LatLon, TimetableSet};
use valinor_loki::{parse_location, search as loki_search};
use valinor_router::OpenError;
use valinor_router::tiles::TileSets;
use valinor_sif::costing_options::parse_costing;
use valinor_sif::json::JsonValue;
use valinor_sif::{AnyCosting, TravelMode as SifMode};
use valinor_thor::{Leg as ThorLeg, Retention, Thor, ThorConfig, Workspace};
use valinor_transit::{Router, Seconds, StopId, StopTable};
use valinor_walk::paths::{access_leg, route_paths};
use valinor_walk::{
    AccessSide, CYCLING_ACCESS_COST_FACTOR, DEFAULT_TRANSFER_SECONDS, SnapConfig, SnappedStop,
    StopAttachments, StopSnaps, StopTables, TableRequest, TableSeed, TableTransfers, TableWalk,
    TransferTable, WalkError, snap_stop,
};

#[cfg(any(test, feature = "test-support"))]
use valinor_walk::snap_stops;

use crate::build::{BuildLoadError, TransitBuild};
use crate::error::{PlanError, StateError};
use crate::plan::{
    AccessEgressRequest, AccessEgressSearch, AccessEgressTables, DirectSearch, PlanStage,
    TransitRequest, TravelMode, plan_pinned, validated_latest,
};
use crate::response::{PlanResponse, VehicleUpdate, WalkManeuver, WalkPath, lower};
use crate::state::{StateConfig, StopSnapper, TransitSnapshot, TransitState};

/// The farthest an origin or destination may be from a usable street, in meters.
const LOCATION_SEARCH_CUTOFF_M: u32 = 500;

/// How long a straight-line trip must take, at most, for a direct street search to be worth it.
///
/// Transit beats any walk that would take longer than this, so the search is skipped,
/// sparing a long pedestrian search for a trip across the whole region.
const DIRECT_SEARCH_MAX_SECONDS: f64 = 2.0 * 3600.0;

const WALKING_SPEED_MPS: f64 = 1.4;
const CYCLING_SPEED_MPS: f64 = 5.0;
/// Sif's default for a wheelchair is slower than walking, around 3.6 km/h.
const WHEELCHAIR_SPEED_MPS: f64 = 1.0;

/// Why a street-based query couldn't be served.
#[derive(Debug, Error)]
pub enum StreetError {
    /// A street search or stop snap failed, or tiles and snaps disagree on their build.
    #[error(transparent)]
    Walk(#[from] WalkError),
    /// A costing couldn't be parsed; only a bad constant does this.
    #[error("set up costing: {0}")]
    Costing(String),
    /// Thor or Ilúvatar couldn't turn a leg into a path.
    #[error("build a walking path: {0}")]
    Path(String),
    /// No street near a query's origin or destination.
    #[error("find a street near the {0}")]
    NoStreetNearby(&'static str),
    /// The snapshot has no stop snaps, so the planner can't reach its stops.
    #[error("use a snapshot without stop snaps for this travel mode")]
    NoSnaps,
}

/// The most a server or device will do for one query, as Valhalla's service limits are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceLimits {
    /// The longest access or egress leg a query may ask for, in seconds.
    ///
    /// Table work and the street graph retained after a query grow with the area it covers,
    /// so an unbounded value is an unbounded query.
    pub max_walk_seconds: u32,
    /// The most search storage one idle worker may keep for the next query, in bytes.
    ///
    /// A worker that retains more after a query gives back its searches' storage
    /// (street table labels and edge states, and transit labels),
    /// which the next query allocates again;
    /// if what is left is still more, the worker is dropped.
    pub max_retained_worker_bytes: usize,
    /// How many idle workers are kept. Concurrent queries beyond this make temporary workers,
    /// so the caller bounds them by how many queries it runs at once.
    pub max_idle_workers: usize,
}

impl Default for ServiceLimits {
    fn default() -> Self {
        Self {
            max_walk_seconds: 3600,
            max_retained_worker_bytes: 512 * 1024 * 1024,
            max_idle_workers: 32,
        }
    }
}

/// Choices for a [`StreetPlanner`].
#[derive(Debug, Clone, Copy)]
pub struct StreetConfig {
    /// Passed through to the state owner.
    pub state: StateConfig,
    /// How GTFS-RT new stops are snapped, and how a query's origin and destination are correlated.
    ///
    /// A build's stops come snapped by the offline build.
    pub snap: SnapConfig,
    /// How many tiles each tile directory keeps in memory.
    pub tile_cache_size: NonZeroUsize,
    /// The most one query may ask for and keep.
    pub limits: ServiceLimits,
    /// How much of a Thor workspace a worker keeps between queries.
    pub retention: Retention,
    /// The longest transfer between two stops on foot, in seconds.
    ///
    /// Only used when walks are built in memory; a build's manifest states its own bound.
    pub max_walking_transfer_seconds: u32,
    /// The longest transfer between two stops by bicycle, in seconds.
    pub max_cycling_transfer_seconds: u32,
    /// The longest transfer between two stops in a wheelchair, in seconds.
    pub max_wheelchair_transfer_seconds: u32,
}

impl Default for StreetConfig {
    fn default() -> Self {
        Self {
            state: StateConfig::default(),
            snap: SnapConfig::default(),
            tile_cache_size: valinor_router::DEFAULT_TILE_CACHE_SIZE,
            limits: ServiceLimits::default(),
            retention: Retention::default(),
            max_walking_transfer_seconds: DEFAULT_TRANSFER_SECONDS,
            max_cycling_transfer_seconds: DEFAULT_TRANSFER_SECONDS,
            max_wheelchair_transfer_seconds: DEFAULT_TRANSFER_SECONDS,
        }
    }
}

/// The pedestrian, bicycle, or wheelchair costing, as Thor's options and a Sif costing.
fn costing_parts(mode: TravelMode) -> Result<(CostingProto, i32), StreetError> {
    let (costing_type, options, key): (_, &[u8], _) = match mode {
        TravelMode::Walking => (CostingType::Pedestrian, b"{}", "/none"),
        TravelMode::Cycling => (CostingType::Bicycle, b"{}", "/none"),
        TravelMode::Wheelchair => (
            CostingType::Pedestrian,
            br#"{"o":{"type":"wheelchair"}}"#,
            "/o",
        ),
    };
    let doc = JsonValue::parse(options).map_err(|e| StreetError::Costing(format!("{e:?}")))?;
    let mut proto = CostingProto::default();
    parse_costing(&doc, key, &mut proto, &mut Vec::new(), Some(costing_type))
        .map_err(|e| StreetError::Costing(format!("{e:?}")))?;
    Ok((proto, costing_type as i32))
}

fn any_costing(mode: TravelMode) -> Result<AnyCosting, StreetError> {
    let (proto, _) = costing_parts(mode)?;
    AnyCosting::new(&proto).map_err(|e| StreetError::Costing(e.to_string()))
}

/// Trip options for building one mode's walking legs.
fn trip_options(mode: TravelMode, language: &str) -> Result<Options, StreetError> {
    let (proto, costing_type) = costing_parts(mode)?;
    let mut options = Options {
        costing_type,
        has_language: Some(HasLanguage::Language(language.to_owned())),
        ..Options::default()
    };
    options.costings.insert(costing_type, proto);
    Ok(options)
}

fn sif_mode(mode: TravelMode) -> SifMode {
    match mode {
        TravelMode::Walking | TravelMode::Wheelchair => SifMode::Pedestrian,
        TravelMode::Cycling => SifMode::Bicycle,
    }
}

/// A location at a point, correlated to the streets by Loki.
///
/// Its edges must reach as far as a stop's (see [`SnapConfig::min_reachability`]):
/// a journey leaves its origin and enters its destination as it does a stop, and an edge into a
/// pocket the mode can't leave, such as a garage with no exit for a bicycle, would strand it.
fn correlated(
    tiles: &TileSets,
    costing: &AnyCosting,
    snap: SnapConfig,
    point: LatLon,
) -> Result<Location, StreetError> {
    let loki = snap.loki_config();
    let mut location = Location {
        ll: Some(LatLng {
            has_lat: Some(HasLat::Lat(point.lat)),
            has_lng: Some(HasLng::Lng(point.lon)),
        }),
        has_minimum_reachability: Some(HasMinimumReachability::MinimumReachability(
            snap.min_reachability,
        )),
        has_search_cutoff: Some(HasSearchCutoff::SearchCutoff(LOCATION_SEARCH_CUTOFF_M)),
        ..Location::default()
    };
    parse_location(&mut location, &loki);
    let mut locations = [location];
    loki_search(&mut locations, &tiles, costing)
        .map_err(|e| StreetError::Walk(WalkError::Loki(e)))?;
    let [location] = locations;
    Ok(location)
}

#[expect(clippy::cast_possible_truncation, reason = "a fraction along one edge")]
fn seeds_of(location: &Location) -> Vec<TableSeed> {
    location
        .correlation
        .as_ref()
        .map_or(&[][..], |c| c.edges.as_slice())
        .iter()
        .filter_map(|edge| {
            Some(TableSeed {
                edge: GraphId::try_from_id(edge.graph_id).ok()?,
                percent_along: edge.percent_along as f32,
                initial: 0.0,
            })
        })
        .collect()
}

fn lon_lat(point: LatLon) -> (f64, f64) {
    (point.lon, point.lat)
}

/// Meters between two points, for the direct-search bound.
fn approximate_meters(a: LatLon, b: LatLon) -> f64 {
    const EARTH_RADIUS_METERS: f64 = 6_371_008.8;
    let lat = f64::midpoint(a.lat, b.lat).to_radians();
    let dx = (b.lon - a.lon).to_radians() * lat.cos();
    let dy = (b.lat - a.lat).to_radians();
    (dx * dx + dy * dy).sqrt() * EARTH_RADIUS_METERS
}

/// Binds `table` to `snaps`, sharing `bound`'s attachments if it is bound to the same snaps.
///
/// A mode's access, egress, and transfer tables reach the same stops, and attachments are keyed
/// by graph ID rather than by a table's own numbering, so one copy serves all three.
fn bind_like(
    table: &mut StopTables<&TileSets, AnyCosting>,
    bound: &StopTables<&TileSets, AnyCosting>,
    snaps: &Arc<StopSnaps>,
) -> Result<(), WalkError> {
    match bound.attachments() {
        Some(attachments) if Arc::ptr_eq(attachments.snaps(), snaps) => {
            table.bind_shared(Arc::clone(attachments));
            Ok(())
        }
        _ => table.bind(Arc::clone(snaps)),
    }
}

/// One travel mode's search storage in one worker.
struct ModeWorker<'a> {
    router: Router<TableTransfers>,
    /// Separate, because a table only reconstructs legs from its most recent search.
    access: StopTables<&'a TileSets, AnyCosting>,
    egress: StopTables<&'a TileSets, AnyCosting>,
    /// Routes the transfers of a result.
    transfer: StopTables<&'a TileSets, AnyCosting>,
    costing: AnyCosting,
}

impl<'a> ModeWorker<'a> {
    fn new(tiles: &'a TileSets, mode: TravelMode) -> Result<Self, StreetError> {
        let mut access = StopTables::new(tiles, any_costing(mode)?);
        let mut egress = StopTables::new(tiles, any_costing(mode)?);
        // Access and egress run on every plan; transfers and their draws keep the table's factor.
        if mode == TravelMode::Cycling {
            access.set_cost_factor_override(Some(CYCLING_ACCESS_COST_FACTOR));
            egress.set_cost_factor_override(Some(CYCLING_ACCESS_COST_FACTOR));
        }
        Ok(Self {
            router: Router::new(TableTransfers::empty()),
            access,
            egress,
            transfer: StopTables::new(tiles, any_costing(mode)?),
            costing: any_costing(mode)?,
        })
    }
}

impl ModeWorker<'_> {
    /// What this mode's searches keep for the next query, in bytes,
    /// counting stop attachments that its tables share once.
    fn retained_bytes(&self) -> usize {
        let tables = [&self.access, &self.egress, &self.transfer];
        let mut attachments: Vec<&Arc<StopAttachments>> = Vec::new();
        for table in tables {
            if let Some(a) = table.attachments()
                && !attachments.iter().any(|seen| Arc::ptr_eq(seen, a))
            {
                attachments.push(a);
            }
        }
        tables
            .iter()
            .map(|table| table.bytes_without_attachments())
            .chain(attachments.iter().map(|a| a.bytes()))
            .fold(
                self.router
                    .transfers()
                    .working_memory_bytes()
                    .saturating_add(self.router.label_bytes()),
                usize::saturating_add,
            )
    }

    /// Gives back what this mode's searches keep from their last query,
    /// keeping the bound stops and the graph's numbering.
    fn release_search(&mut self) {
        self.router.release_labels();
        self.access.release_search();
        self.egress.release_search();
        self.transfer.release_search();
    }
}

/// One thread's search storage: per-mode searches and a Thor workspace.
struct Worker<'a> {
    walking: ModeWorker<'a>,
    cycling: ModeWorker<'a>,
    wheelchair: ModeWorker<'a>,
    thor: Thor<&'a TileSets>,
    workspace: Workspace,
}

impl<'a> Worker<'a> {
    fn new(tiles: &'a TileSets, retention: Retention) -> Result<Self, StreetError> {
        Ok(Self {
            walking: ModeWorker::new(tiles, TravelMode::Walking)?,
            cycling: ModeWorker::new(tiles, TravelMode::Cycling)?,
            wheelchair: ModeWorker::new(tiles, TravelMode::Wheelchair)?,
            thor: Thor::new(ThorConfig::default(), tiles),
            workspace: Workspace::new(retention),
        })
    }

    /// What this worker's searches keep for the next query, in bytes.
    fn retained_bytes(&self) -> usize {
        [&self.walking, &self.cycling, &self.wheelchair]
            .into_iter()
            .map(ModeWorker::retained_bytes)
            .fold(self.workspace.retained_bytes(), usize::saturating_add)
    }

    /// Gives back what every mode's searches keep from their last query.
    fn release_search(&mut self) {
        self.walking.release_search();
        self.cycling.release_search();
        self.wheelchair.release_search();
    }
}

/// The per-query [`AccessEgressSearch`] over a worker's storage.
struct StreetSearch<'w, 'a> {
    tiles: &'a TileSets,
    mode: TravelMode,
    mode_worker_access: &'w mut StopTables<&'a TileSets, AnyCosting>,
    mode_worker_egress: &'w mut StopTables<&'a TileSets, AnyCosting>,
    mode_worker_transfer: &'w mut StopTables<&'a TileSets, AnyCosting>,
    costing: &'w AnyCosting,
    thor: &'w Thor<&'a TileSets>,
    workspace: &'w mut Workspace,
    odin: &'w Odin,
    options: Options,
    now: Timestamp,
    interrupt: &'w dyn Fn() -> bool,
    probe: &'w dyn Fn(PlanStage),
    max_walk_seconds: u32,
    /// How the origin and destination are correlated, as stops are snapped.
    snap: SnapConfig,
    /// Transfers drawn so far: journeys of a window share them, and a draw is a street search.
    /// Keyed by everything a draw depends on within one query.
    transfer_paths: HashMap<(AccessSide, StopId, StopId), Option<WalkPath>>,
}

impl StreetSearch<'_, '_> {
    /// A snapped stop's point on the street network.
    fn stop_point(snaps: &StopSnaps, stop: StopId) -> Option<(f64, f64)> {
        snaps.point(stop).map(|snap| snap.point)
    }

    fn snaps<'s>(&self, snapshot: &'s TransitSnapshot) -> Result<&'s StopSnaps, StreetError> {
        snapshot
            .snaps()
            .map(|snaps| snaps.for_mode(self.mode).as_ref())
            .ok_or(StreetError::NoSnaps)
    }

    /// Thor builds the leg's shape and Ilúvatar narrates it.
    fn build_path(&mut self, leg: ThorLeg) -> Result<WalkPath, StreetError> {
        (self.probe)(PlanStage::ThorTrip);
        let mut api = Api {
            options: Some(self.options.clone()),
            ..Api::default()
        };
        self.thor
            .build_trip_interruptible(&mut api, &route_paths(vec![leg]), self.now, self.interrupt)
            .map_err(|e| StreetError::Path(e.to_string()))?;
        (self.probe)(PlanStage::Narrate);
        self.odin
            .narrate(&mut api)
            .map_err(|e| StreetError::Path(e.to_string()))?;
        let directions = api
            .directions
            .ok_or_else(|| StreetError::Path("narration produced no directions".to_owned()))?;
        (self.probe)(PlanStage::ConvertWalkPath);
        let route = directions.routes.into_iter().next();
        let leg = route
            .and_then(|route| route.legs.into_iter().next())
            .ok_or_else(|| StreetError::Path("narration produced no legs".to_owned()))?;
        Ok(walk_path(leg))
    }

    /// Routes a transfer again by the search that found it, and builds its path.
    fn draw_transfer(
        &mut self,
        request: &AccessEgressRequest<'_>,
        walk: &TableWalk,
        from: StopId,
        to: StopId,
    ) -> Result<Option<WalkPath>, StreetError> {
        (self.probe)(PlanStage::DrawTransfer);
        let snaps = self.snaps(request.snapshot)?;
        let (Some(start), Some(end)) = (Self::stop_point(snaps, from), Self::stop_point(snaps, to))
        else {
            return Ok(None);
        };
        // The search that found this walk, so the drawn route matches its seconds: a walk from a
        // base stop to a GTFS-RT new stop was found by a backward search from the new stop.
        // It runs at the table's own limit, which sets the search's cost bound.
        bind_like(
            self.mode_worker_transfer,
            self.mode_worker_access,
            request
                .snapshot
                .snaps()
                .ok_or(StreetError::NoSnaps)?
                .for_mode(request.mode),
        )?;
        let (seed_stop, target_stop) = match walk.search {
            AccessSide::Access => (walk.from, walk.to),
            AccessSide::Egress => (walk.to, walk.from),
        };
        let max_seconds = request
            .snapshot
            .snaps()
            .ok_or(StreetError::NoSnaps)?
            .transfers_for_mode(request.mode)
            .max_seconds();
        let seeds = self.mode_worker_transfer.stop_seeds(seed_stop)?;
        if !self.mode_worker_transfer.search_to_stop(
            &TableRequest {
                side: walk.search,
                seeds: &seeds,
                max_seconds,
                interrupt: self.interrupt,
            },
            target_stop,
        )? {
            return Ok(None);
        }
        let Some(leg) = self.mode_worker_transfer.leg(target_stop) else {
            return Ok(None);
        };
        let Some(thor_leg) = access_leg(&leg, sif_mode(self.mode), 0, 1, start, end) else {
            return Ok(None);
        };
        self.build_path(thor_leg).map(Some)
    }

    /// How a direct search for the whole trip fares, when it is worth running.
    fn direct(
        &mut self,
        origin: &Location,
        destination: &Location,
        from: LatLon,
        to: LatLon,
    ) -> DirectSearch {
        let speed = match self.mode {
            TravelMode::Walking => WALKING_SPEED_MPS,
            TravelMode::Cycling => CYCLING_SPEED_MPS,
            TravelMode::Wheelchair => WHEELCHAIR_SPEED_MPS,
        };
        if approximate_meters(from, to) / speed > DIRECT_SEARCH_MAX_SECONDS {
            return DirectSearch::NotSearched;
        }
        let mut options = self.options.clone();
        options.locations = vec![origin.clone(), destination.clone()];
        let mut api = Api {
            options: Some(options),
            ..Api::default()
        };
        let paths =
            match self
                .thor
                .route_interruptible(self.workspace, &mut api, self.now, self.interrupt)
            {
                Ok(paths) => paths,
                Err(error) if error.code() == ErrorCode::NoPathFound => {
                    return DirectSearch::NoRoute;
                }
                Err(error) => return DirectSearch::Failed(error.to_string()),
            };
        let Some(edge) = paths
            .routes
            .first()
            .and_then(|route| route.legs.first())
            .and_then(|leg| leg.edges.last())
        else {
            return DirectSearch::NoRoute;
        };
        #[expect(clippy::cast_possible_truncation, reason = "a search's seconds")]
        #[expect(clippy::cast_sign_loss, reason = "elapsed seconds are positive")]
        let seconds = edge.elapsed_cost.secs.ceil() as u32;
        Seconds::try_new(seconds).map_or(DirectSearch::NotSearched, DirectSearch::Found)
    }
}

/// A route's first leg of directions, as this crate's walk path.
fn walk_path(leg: valhalla_proto::DirectionsLeg) -> WalkPath {
    let summary = leg.summary.unwrap_or_default();
    let maneuvers = leg
        .maneuver
        .into_iter()
        .map(|maneuver| WalkManeuver {
            kind: maneuver_name(&maneuver),
            instruction: maneuver.text_instruction,
            verbal_pre_transition_instruction: non_empty(
                maneuver.verbal_pre_transition_instruction,
            ),
            verbal_post_transition_instruction: non_empty(
                maneuver.verbal_post_transition_instruction,
            ),
            street_names: maneuver
                .street_name
                .into_iter()
                .map(|name| name.value)
                .collect(),
            distance_meters: rounded(f64::from(maneuver.length) * 1000.0),
            duration_seconds: rounded(maneuver.time),
            begin_shape_index: maneuver.begin_shape_index,
            end_shape_index: maneuver.end_shape_index,
        })
        .collect();
    WalkPath {
        distance_meters: rounded(f64::from(summary.length) * 1000.0),
        duration_seconds: rounded(summary.time),
        shape: leg.shape,
        maneuvers,
    }
}

/// A tenth of a meter or second: anything finer is float noise, and churns snapshots.
fn rounded(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn maneuver_name(maneuver: &valhalla_proto::directions_leg::Maneuver) -> String {
    let name = maneuver.r#type().as_str_name();
    let trimmed = name.strip_prefix('k').unwrap_or(name);
    // `kSlightRight` to `slight_right`.
    let mut out = String::with_capacity(trimmed.len() + 4);
    for (i, c) in trimmed.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    lower(&out)
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

impl AccessEgressSearch for StreetSearch<'_, '_> {
    type Error = StreetError;
    type Walk = TableWalk;

    fn enter_stage(&self, stage: PlanStage) {
        (self.probe)(stage);
    }

    fn transfer_seconds(&self, walk: &TableWalk) -> Option<u32> {
        Some(walk.seconds)
    }

    fn tables(
        &mut self,
        request: &AccessEgressRequest<'_>,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<AccessEgressTables, StreetError> {
        let snaps = request
            .snapshot
            .snaps()
            .ok_or(StreetError::NoSnaps)?
            .for_mode(request.mode);
        let stop_count = request.snapshot.snapshot().stop_count();
        self.mode_worker_access.bind(Arc::clone(snaps))?;
        bind_like(self.mode_worker_egress, self.mode_worker_access, snaps)?;

        (self.probe)(PlanStage::Locate);
        let origin = correlated(self.tiles, self.costing, self.snap, request.origin)?;
        let destination = correlated(self.tiles, self.costing, self.snap, request.destination)?;
        let origin_seeds = seeds_of(&origin);
        let destination_seeds = seeds_of(&destination);
        if origin_seeds.is_empty() {
            return Err(StreetError::NoStreetNearby("origin"));
        }
        if destination_seeds.is_empty() {
            return Err(StreetError::NoStreetNearby("destination"));
        }

        (self.probe)(PlanStage::AccessTable);
        let access = self.mode_worker_access.compute(
            stop_count,
            &TableRequest {
                side: AccessSide::Access,
                seeds: &origin_seeds,
                max_seconds: self.max_walk_seconds,
                interrupt,
            },
        )?;
        (self.probe)(PlanStage::EgressTable);
        let egress = self.mode_worker_egress.compute(
            stop_count,
            &TableRequest {
                side: AccessSide::Egress,
                seeds: &destination_seeds,
                max_seconds: self.max_walk_seconds,
                interrupt,
            },
        )?;
        let (access, egress) = match (access, egress) {
            (Some(access), Some(egress)) => (access, egress),
            // Interrupted: empty tables, and the profile reports the interrupt.
            _ => (StopTable::new(stop_count), StopTable::new(stop_count)),
        };
        (self.probe)(PlanStage::DirectRoute);
        let direct = self.direct(&origin, &destination, request.origin, request.destination);
        Ok(AccessEgressTables {
            access,
            egress,
            direct,
        })
    }

    fn access_path(
        &mut self,
        request: &AccessEgressRequest<'_>,
        stop: StopId,
    ) -> Result<Option<WalkPath>, StreetError> {
        (self.probe)(PlanStage::DrawAccessEgress);
        let snaps = self.snaps(request.snapshot)?;
        let Some(end) = Self::stop_point(snaps, stop) else {
            return Ok(None);
        };
        let Some(leg) = self.mode_worker_access.leg(stop) else {
            return Ok(None);
        };
        let Some(thor_leg) = access_leg(
            &leg,
            sif_mode(self.mode),
            0,
            1,
            lon_lat(request.origin),
            end,
        ) else {
            return Ok(None);
        };
        self.build_path(thor_leg).map(Some)
    }

    fn egress_path(
        &mut self,
        request: &AccessEgressRequest<'_>,
        stop: StopId,
    ) -> Result<Option<WalkPath>, StreetError> {
        (self.probe)(PlanStage::DrawAccessEgress);
        let snaps = self.snaps(request.snapshot)?;
        let Some(start) = Self::stop_point(snaps, stop) else {
            return Ok(None);
        };
        let Some(leg) = self.mode_worker_egress.leg(stop) else {
            return Ok(None);
        };
        let Some(thor_leg) = access_leg(
            &leg,
            sif_mode(self.mode),
            0,
            1,
            start,
            lon_lat(request.destination),
        ) else {
            return Ok(None);
        };
        self.build_path(thor_leg).map(Some)
    }

    fn transfer_path(
        &mut self,
        request: &AccessEgressRequest<'_>,
        walk: &TableWalk,
        from: StopId,
        to: StopId,
    ) -> Result<Option<WalkPath>, StreetError> {
        let key = (walk.search, from, to);
        if let Some(path) = self.transfer_paths.get(&key) {
            return Ok(path.clone());
        }
        let path = self.draw_transfer(request, walk, from, to)?;
        self.transfer_paths.insert(key, path.clone());
        Ok(path)
    }
}

/// The search workers borrow the tile sets.
struct WorkerPool<'a> {
    tiles: &'a TileSets,
    idle: Mutex<Vec<Worker<'a>>>,
    odin: Odin,
    limits: ServiceLimits,
    retention: Retention,
    snap: SnapConfig,
}

impl<'a> WorkerPool<'a> {
    fn take(&self) -> Result<Worker<'a>, StreetError> {
        let idle = self
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        match idle {
            Some(worker) => Ok(worker),
            None => Worker::new(self.tiles, self.retention),
        }
    }

    /// Keeps a worker for the next query, unless it holds too much or enough are idle.
    fn put(&self, mut worker: Worker<'a>) {
        // Thor's searches keep their storage until their next start; give it back now.
        worker.workspace.trim();
        if worker.retained_bytes() > self.limits.max_retained_worker_bytes {
            // The searches' storage is what grows from query to query (and from mode to mode);
            // the next query allocates what it needs again.
            worker.release_search();
            if worker.retained_bytes() > self.limits.max_retained_worker_bytes {
                // What is left (bound stops, the graph's numbering, Thor's retention)
                // is still too much: let the worker go.
                return;
            }
        }
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        if idle.len() < self.limits.max_idle_workers {
            idle.push(worker);
        }
    }
}

self_cell!(
    struct Cell {
        owner: Arc<TileSets>,
        #[not_covariant]
        dependent: WorkerPool,
    }
);

/// A snapper over an owned tile set, for the state owner.
struct TileSnapper {
    tiles: Arc<TileSets>,
    config: SnapConfig,
    transfers: BaseTransfers,
}

/// Where a planner's walks between stops come from.
enum BaseTransfers {
    /// Assembled from a build's transfer tiles, which is the only path in production.
    Loaded {
        walking: Arc<TransferTable>,
        cycling: Arc<TransferTable>,
        wheelchair: Arc<TransferTable>,
        walking_snaps: Arc<StopSnaps>,
        cycling_snaps: Arc<StopSnaps>,
        wheelchair_snaps: Arc<StopSnaps>,
    },
    /// Built in memory from the street network, for tests.
    #[cfg(any(test, feature = "test-support"))]
    Build {
        walking_seconds: u32,
        cycling_seconds: u32,
        wheelchair_seconds: u32,
    },
}

impl fmt::Debug for TileSnapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TileSnapper").finish_non_exhaustive()
    }
}

impl StopSnapper for TileSnapper {
    fn snap_all(
        &self,
        mode: TravelMode,
        stops: &[(f64, f64)],
    ) -> Result<Arc<StopSnaps>, WalkError> {
        match &self.transfers {
            BaseTransfers::Loaded {
                walking_snaps,
                cycling_snaps,
                wheelchair_snaps,
                ..
            } => {
                let _ = stops;
                Ok(Arc::clone(match mode {
                    TravelMode::Walking => walking_snaps,
                    TravelMode::Cycling => cycling_snaps,
                    TravelMode::Wheelchair => wheelchair_snaps,
                }))
            }
            #[cfg(any(test, feature = "test-support"))]
            BaseTransfers::Build { .. } => {
                let tiles: &TileSets = &self.tiles;
                let costing = any_costing(mode).map_err(snap_setup_error)?;
                snap_stops(&tiles, &costing, &self.config, tiles.build_id(), stops).map(Arc::new)
            }
        }
    }

    fn snap_one(&self, mode: TravelMode, ll: (f64, f64)) -> Result<SnappedStop, WalkError> {
        let tiles: &TileSets = &self.tiles;
        let costing = any_costing(mode).map_err(snap_setup_error)?;
        snap_stop(&tiles, &costing, &self.config, ll)
    }

    fn base_transfers(
        &self,
        mode: TravelMode,
        snaps: &Arc<StopSnaps>,
    ) -> Result<Arc<TransferTable>, WalkError> {
        // Only the in-memory build needs the snaps.
        let _ = snaps;
        match &self.transfers {
            BaseTransfers::Loaded {
                walking,
                cycling,
                wheelchair,
                ..
            } => Ok(Arc::clone(match mode {
                TravelMode::Walking => walking,
                TravelMode::Cycling => cycling,
                TravelMode::Wheelchair => wheelchair,
            })),
            #[cfg(any(test, feature = "test-support"))]
            BaseTransfers::Build {
                walking_seconds,
                cycling_seconds,
                wheelchair_seconds,
            } => {
                let tiles: &TileSets = &self.tiles;
                let costing = any_costing(mode).map_err(snap_setup_error)?;
                let threads = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
                let seconds = match mode {
                    TravelMode::Walking => *walking_seconds,
                    TravelMode::Cycling => *cycling_seconds,
                    TravelMode::Wheelchair => *wheelchair_seconds,
                };
                TransferTable::build(&tiles, &costing, snaps, seconds, threads, &|| false)?
                    .map(Arc::new)
                    .ok_or(WalkError::NoCandidates)
            }
        }
    }

    fn add_transfers(
        &self,
        mode: TravelMode,
        base: &TransferTable,
        snaps: &Arc<StopSnaps>,
        new_stops: &[StopId],
    ) -> Result<TransferTable, WalkError> {
        let tiles: &TileSets = &self.tiles;
        let costing = any_costing(mode).map_err(snap_setup_error)?;
        base.with_new_stops(&tiles, &costing, snaps, new_stops, &|| false)?
            .ok_or(WalkError::NoCandidates)
    }
}

/// Costing setup only fails on a bad constant, which a snap can report as nothing nearby.
fn snap_setup_error(_: StreetError) -> WalkError {
    WalkError::NoCandidates
}

/// Plans over a compiled timetable, real-time overlays, and real streets.
///
/// One planner serves every thread; each [`Self::plan`] takes a search worker from a pool.
/// The pool keeps [`ServiceLimits::max_idle_workers`], and a query with none idle makes one,
/// so the number of workers alive is the number of queries the caller runs at once.
pub struct StreetPlanner {
    cell: Cell,
    state: Arc<TransitState>,
    limits: ServiceLimits,
    /// Whether the transit build has wheelchair tables; a build may be made without them.
    has_wheelchair: bool,
}

impl fmt::Debug for StreetPlanner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreetPlanner")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// A mode the build has no tables for is a bad request, not a search that finds nothing.
pub(crate) fn require_mode_built<T, A>(
    mode: TravelMode,
    has_wheelchair: bool,
) -> Result<(), PlanError<T, A>>
where
    T: core::error::Error + 'static,
    A: core::error::Error + 'static,
{
    if mode == TravelMode::Wheelchair && !has_wheelchair {
        return Err(PlanError::InvalidRequest(
            "wheelchair routing, which this build has no tables for",
        ));
    }
    Ok(())
}

/// Why a [`StreetPlanner`] couldn't open.
#[derive(Debug, Error)]
pub enum OpenPlannerError {
    /// The tile sets couldn't be opened.
    #[error(transparent)]
    Tiles(#[from] OpenError),
    /// The timetable or stop snaps couldn't be set up.
    #[error(transparent)]
    State(#[from] StateError),
    /// The build can't be loaded or isn't consistent.
    #[error(transparent)]
    Build(#[from] BuildLoadError),
}

/// The error of [`StreetPlanner::plan`].
pub type StreetPlanError = PlanError<WalkError, StreetError>;

impl StreetPlanner {
    /// Opens a loaded transit build over tile sets.
    ///
    /// # Performance
    ///
    /// Stop snaps and tables of walks come from the build, so this only materializes state.
    ///
    /// # Errors
    ///
    /// If the tiles can't be opened, are of a different street build from the manifest,
    /// or the timetables can't be set up.
    pub fn from_build<P: AsRef<Path>>(
        build: TransitBuild,
        tile_sets: &[P],
        config: StreetConfig,
    ) -> Result<Self, OpenPlannerError> {
        let tiles = Arc::new(TileSets::open(tile_sets, config.tile_cache_size)?);
        if u32::from(tiles.build_id().value()) != build.valhalla_build_id {
            return Err(OpenPlannerError::Build(
                BuildLoadError::StreetBuildMismatch {
                    manifest: build.valhalla_build_id,
                    tiles: u32::from(tiles.build_id().value()),
                },
            ));
        }
        let has_wheelchair = build.has_wheelchair;
        let transfers = BaseTransfers::Loaded {
            walking: Arc::new(build.walking),
            cycling: Arc::new(build.cycling),
            wheelchair: Arc::new(build.wheelchair),
            walking_snaps: Arc::new(build.walking_snaps),
            cycling_snaps: Arc::new(build.cycling_snaps),
            wheelchair_snaps: Arc::new(build.wheelchair_snaps),
        };
        Self::open_tiles(build.set, tiles, transfers, config, has_wheelchair)
    }

    /// Opens a compiled timetable over tile sets, building its walks in memory.
    ///
    /// For tests and tools only: production builds its tables offline, and loads them as tiles.
    ///
    /// # Errors
    ///
    /// If the tiles can't be opened or the timetable can't be built.
    #[cfg(any(test, feature = "test-support"))]
    pub fn open_with_built_transfers<P: AsRef<Path>>(
        set: TimetableSet,
        tile_sets: &[P],
        config: StreetConfig,
    ) -> Result<Self, OpenPlannerError> {
        let tiles = Arc::new(TileSets::open(tile_sets, config.tile_cache_size)?);
        let transfers = BaseTransfers::Build {
            walking_seconds: config.max_walking_transfer_seconds,
            cycling_seconds: config.max_cycling_transfer_seconds,
            wheelchair_seconds: config.max_wheelchair_transfer_seconds,
        };
        Self::open_tiles(set, tiles, transfers, config, true)
    }

    fn open_tiles(
        set: TimetableSet,
        tiles: Arc<TileSets>,
        transfers: BaseTransfers,
        config: StreetConfig,
        has_wheelchair: bool,
    ) -> Result<Self, OpenPlannerError> {
        let snapper = TileSnapper {
            tiles: Arc::clone(&tiles),
            config: config.snap,
            transfers,
        };
        let state = TransitState::with_snapper(set, config.state, Box::new(snapper))?;
        let cell = Cell::new(tiles, |tiles| WorkerPool {
            tiles,
            idle: Mutex::new(Vec::new()),
            odin: Odin::default(),
            limits: config.limits,
            retention: config.retention,
            snap: config.snap,
        });
        Ok(Self {
            cell,
            state: Arc::new(state),
            limits: config.limits,
            has_wheelchair,
        })
    }

    /// The timetable state, for ingesting GTFS-RT bytes and refreshing.
    pub fn state(&self) -> &Arc<TransitState> {
        &self.state
    }

    /// Plans one request with real streets, narrating maneuvers in `language`.
    ///
    /// # Performance
    ///
    /// Walking searches are not optimized yet.
    ///
    /// # Errors
    ///
    /// If the request is invalid, `interrupt` asks to stop, or a search or leg expansion fails.
    pub fn plan(
        &self,
        request: &TransitRequest,
        language: &str,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<PlanResponse, StreetPlanError> {
        self.plan_probed(request, language, interrupt, &|_| {})
    }

    /// [`Self::plan`], telling `probe` as each stage of the work begins,
    /// so that a tool can see where a plan's time goes.
    ///
    /// # Errors
    ///
    /// As [`Self::plan`].
    pub fn plan_probed(
        &self,
        request: &TransitRequest,
        language: &str,
        interrupt: &dyn Fn() -> bool,
        probe: &dyn Fn(PlanStage),
    ) -> Result<PlanResponse, StreetPlanError> {
        probe(PlanStage::Setup);
        let latest_unix = validated_latest::<WalkError, StreetError>(request)?;
        if request.options.max_walk_seconds > self.limits.max_walk_seconds {
            return Err(PlanError::InvalidRequest(
                "max_walk_seconds exceeds the service limit",
            ));
        }
        require_mode_built(request.mode, self.has_wheelchair)?;
        let snapshot = self
            .state
            .pin_for_departure(request.now, request.departure, latest_unix)?;
        let now = Timestamp::from_second(request.now)
            .map_err(|_| PlanError::InvalidRequest("now is out of range"))?;
        self.cell.with_dependent(|_, pool| {
            let mut worker = pool.take().map_err(PlanError::AccessEgress)?;
            let result = Self::plan_with(
                pool,
                &mut worker,
                &snapshot,
                request,
                language,
                now,
                interrupt,
                probe,
            );
            probe(PlanStage::Release);
            pool.put(worker);
            result
        })
    }

    /// The latest real-time state of a vehicle.
    ///
    /// # Errors
    ///
    /// If `now` is outside the timetable's coverage.
    pub fn vehicle(&self, now: i64, feed: &str, id: &str) -> Result<VehicleUpdate, StateError> {
        self.state.vehicle(now, feed, id)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one query's context, for one call"
    )]
    fn plan_with<'a>(
        pool: &WorkerPool<'a>,
        worker: &mut Worker<'a>,
        snapshot: &Arc<TransitSnapshot>,
        request: &TransitRequest,
        language: &str,
        now: Timestamp,
        interrupt: &dyn Fn() -> bool,
        probe: &dyn Fn(PlanStage),
    ) -> Result<PlanResponse, StreetPlanError> {
        let Worker {
            walking,
            cycling,
            wheelchair,
            thor,
            workspace,
        } = worker;
        let mode_worker = match request.mode {
            TravelMode::Walking => walking,
            TravelMode::Cycling => cycling,
            TravelMode::Wheelchair => wheelchair,
        };
        let snaps = snapshot
            .snaps()
            .ok_or(StreetError::NoSnaps)
            .map_err(PlanError::AccessEgress)?;
        mode_worker
            .router
            .transfers_mut()
            .bind(Arc::clone(snaps.transfers_for_mode(request.mode)));

        let ModeWorker {
            router,
            access,
            egress,
            transfer,
            costing,
        } = mode_worker;
        let mut search = StreetSearch {
            tiles: pool.tiles,
            mode: request.mode,
            mode_worker_access: access,
            mode_worker_egress: egress,
            mode_worker_transfer: transfer,
            costing,
            thor,
            workspace,
            odin: &pool.odin,
            options: trip_options(request.mode, language).map_err(PlanError::AccessEgress)?,
            now,
            interrupt,
            probe,
            max_walk_seconds: request.options.max_walk_seconds,
            snap: pool.snap,
            transfer_paths: HashMap::new(),
        };
        plan_pinned(snapshot, router, &mut search, request, interrupt)
    }
}

/// What an idle worker keeps, for tests of the pool.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct IdleWorker {
    /// [`Worker::retained_bytes`].
    pub(crate) retained_bytes: usize,
    /// Per mode: how many of its tables are bound, and how many distinct stop attachments
    /// they hold between them.
    pub(crate) attachments: [(usize, usize); 3],
}

#[cfg(test)]
impl StreetPlanner {
    /// The pool's idle workers, most recently returned last.
    pub(crate) fn idle_workers(&self) -> Vec<IdleWorker> {
        fn attachments(mode: &ModeWorker<'_>) -> (usize, usize) {
            let bound: Vec<&Arc<StopAttachments>> = [&mode.access, &mode.egress, &mode.transfer]
                .into_iter()
                .filter_map(|table| table.attachments())
                .collect();
            let distinct = bound
                .iter()
                .enumerate()
                .filter(|(i, a)| !bound[..*i].iter().any(|b| Arc::ptr_eq(a, b)))
                .count();
            (bound.len(), distinct)
        }
        self.cell.with_dependent(|_, pool| {
            pool.idle
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .map(|worker| IdleWorker {
                    retained_bytes: worker.retained_bytes(),
                    attachments: [
                        attachments(&worker.walking),
                        attachments(&worker.cycling),
                        attachments(&worker.wheelchair),
                    ],
                })
                .collect()
        })
    }
}
