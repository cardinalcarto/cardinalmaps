//! Journeys: what the searches found, with enough identity for a response layer to expand.

use crate::ids::{Direction, LaneRef, RouteId, Seconds, StopId, Time, TripKey};
use crate::raptor::{LabelKind, Labels, TransitQuery};

/// A journey: one or more rides, joined by walks.
///
/// `W` is the transfer search's [`TransferSearch::Walk`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Journey<W> {
    /// When the rider leaves the origin.
    pub departure: Time,
    /// When the rider reaches the target.
    pub arrival: Time,
    /// How many trips it uses.
    pub trips: u8,
    /// The legs in travel order.
    pub legs: Vec<Leg<W>>,
}

/// One leg of a journey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leg<W> {
    /// From the origin to the first stop.
    Access {
        /// The stop reached.
        stop: StopId,
        /// How long it took, from the access table.
        duration: Seconds,
    },
    /// A ride on one trip.
    Ride(Ride),
    /// A walk between two different stops.
    Transfer {
        /// The stop the rider left.
        from: StopId,
        /// The stop the rider reached.
        to: StopId,
        /// How long the walk took.
        duration: Seconds,
        /// The walk, resolved by the transfer search.
        walk: W,
    },
    /// From the last stop to the target.
    Egress {
        /// The stop left.
        stop: StopId,
        /// How long it took, from the egress table.
        duration: Seconds,
    },
}

/// A ride on one trip, identified well enough to find every detail again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ride {
    /// The route.
    pub route: RouteId,
    /// The lane, in the query's pinned snapshot.
    pub lane: LaneRef,
    /// The trip's row within the lane.
    pub row: u32,
    /// The trip and its service date.
    pub trip: TripKey,
    /// Where the rider boarded.
    pub board_stop: StopId,
    /// The position of the boarding on the route.
    pub board_position: u32,
    /// When the trip departed from the boarding stop.
    pub board_time: Time,
    /// Where the rider alighted.
    pub alight_stop: StopId,
    /// The position of the alighting on the route.
    pub alight_position: u32,
    /// When the trip arrived at the alighting stop.
    pub alight_time: Time,
}

/// A step along the label chain, in chain order.
enum ChainStep<P> {
    Ride {
        round: u8,
        stop: usize,
    },
    Walk {
        reached: usize,
        source: StopId,
        duration: i32,
        parent: P,
    },
}

impl<P: Copy> Journey<P> {
    /// Turns each walk's handle into what `resolve` makes of it.
    ///
    /// Resolving a walk is the expensive part of reading a journey (on real tiles it rebuilds
    /// the street path), so a caller decides whether a journey is useful before it does this.
    pub(crate) fn resolve_walks<W>(self, resolve: impl Fn(P) -> W) -> Journey<W> {
        Journey {
            departure: self.departure,
            arrival: self.arrival,
            trips: self.trips,
            legs: self
                .legs
                .into_iter()
                .map(|leg| match leg {
                    Leg::Access { stop, duration } => Leg::Access { stop, duration },
                    Leg::Ride(ride) => Leg::Ride(ride),
                    Leg::Transfer {
                        from,
                        to,
                        duration,
                        walk,
                    } => Leg::Transfer {
                        from,
                        to,
                        duration,
                        walk: resolve(walk),
                    },
                    Leg::Egress { stop, duration } => Leg::Egress { stop, duration },
                })
                .collect(),
        }
    }
}

/// Reads the journey the labels hold for `trips`, if the search found one.
///
/// Its walks are still the transfer search's handles; see [`Journey::resolve_walks`].
pub(crate) fn trace_journey<P: Copy>(
    query: &TransitQuery<'_>,
    labels: &Labels<P>,
    direction: Direction,
    trips: u8,
) -> Option<Journey<P>> {
    let target = labels.target(trips)?;
    if !target.improved {
        return None;
    }

    // Walk from the search's target back to its seeding table.
    let mut chain = Vec::new();
    let mut stop = target.stop.index();
    let mut round = trips;
    let mut kind = LabelKind::Trip;
    loop {
        match kind {
            LabelKind::Trip => {
                let cell = labels.trip_cell(round, stop)?;
                chain.push(ChainStep::Ride { round, stop });
                let lane = query.snapshot.lane(cell.lane)?;
                let arena = query.snapshot.arena(cell.lane.arena());
                stop = arena.lane_stops(lane).get(cell.board_pos as usize)?.index();
                round = cell.origin_round;
                kind = cell.origin_kind;
            }
            LabelKind::Foot => {
                let cell = labels.foot_cell(round, stop)?;
                let Some(parent) = cell.parent else {
                    // An access entry of round zero: the chain is complete.
                    break;
                };
                let trip_time = labels.trip_time(round, cell.source.index())?;
                chain.push(ChainStep::Walk {
                    reached: stop,
                    source: cell.source,
                    duration: cell.time - trip_time,
                    parent,
                });
                stop = cell.source.index();
                kind = LabelKind::Trip;
            }
        }
    }
    let seed_stop = StopId::new(u32::try_from(stop).ok()?);

    // The chain runs from the search's target to its seed: travel order when searching backward,
    // reversed when searching forward.
    if direction == Direction::Forward {
        chain.reverse();
    }

    let (access_stop, egress_stop) = match direction {
        Direction::Forward => (seed_stop, target.stop),
        Direction::Backward => (target.stop, seed_stop),
    };
    let access = query.access.cost(access_stop)?;
    let egress = query.egress.cost(egress_stop)?;

    let mut legs = Vec::with_capacity(chain.len() + 2);
    legs.push(Leg::Access {
        stop: access_stop,
        duration: access,
    });
    let mut first_board = None;
    let mut last_alight = None;
    let mut ride_count = 0_u8;
    for step in chain {
        match step {
            ChainStep::Ride { round, stop } => {
                let ride = ride_from_cell(query, labels, direction, round, stop)?;
                first_board.get_or_insert(ride.board_time);
                last_alight = Some(ride.alight_time);
                legs.push(Leg::Ride(ride));
                ride_count += 1;
            }
            ChainStep::Walk {
                reached,
                source,
                duration,
                parent,
            } => {
                let reached = StopId::new(u32::try_from(reached).ok()?);
                let (from, to) = match direction {
                    Direction::Forward => (source, reached),
                    Direction::Backward => (reached, source),
                };
                legs.push(Leg::Transfer {
                    from,
                    to,
                    duration: Seconds::try_new(duration.try_into().ok()?).ok()?,
                    walk: parent,
                });
            }
        }
    }
    legs.push(Leg::Egress {
        stop: egress_stop,
        duration: egress,
    });

    let departure = first_board?.into_inner() - access.as_i32();
    let arrival = last_alight?.into_inner() + egress.as_i32();
    Some(Journey {
        departure: Time::try_new(departure).ok()?,
        arrival: Time::try_new(arrival).ok()?,
        trips: ride_count,
        legs,
    })
}

/// Reads the real-time facts of a ride from its label.
fn ride_from_cell<P: Copy>(
    query: &TransitQuery<'_>,
    labels: &Labels<P>,
    direction: Direction,
    round: u8,
    stop: usize,
) -> Option<Ride> {
    let cell = labels.trip_cell(round, stop)?;
    let lane = query.snapshot.lane(cell.lane)?;
    let arena = query.snapshot.arena(cell.lane.arena());
    let (board_position, alight_position) = match direction {
        Direction::Forward => (cell.board_pos, cell.alight_pos),
        Direction::Backward => (cell.alight_pos, cell.board_pos),
    };
    let stride = lane.stop_count as usize * 2;
    let grid = arena.lane_times(lane);
    let row = cell.row as usize;
    let row_times = grid.get(row * stride..(row + 1) * stride)?;
    let stops = arena.lane_stops(lane);
    let board_time = Time::try_new(*row_times.get(2 * board_position as usize + 1)?).ok()?;
    let alight_time = Time::try_new(*row_times.get(2 * alight_position as usize)?).ok()?;
    let real_row = cell.row.checked_sub(1)?;
    Some(Ride {
        route: lane.route,
        lane: cell.lane,
        row: real_row,
        trip: query.snapshot.trip_key(cell.lane, real_row)?,
        board_stop: *stops.get(board_position as usize)?,
        board_position,
        board_time,
        alight_stop: *stops.get(alight_position as usize)?,
        alight_position,
        alight_time,
    })
}
