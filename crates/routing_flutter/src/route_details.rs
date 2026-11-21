//! What the directions sheet says about a route beyond its line and steps: what to call
//! it, and how fast and on what kind of road it goes along the way.
//!
//! All of it is worked out from the router's OSRM answer, which
//! [`crate::api::directions`] reads and hands over as the plain values here.

use crate::api::directions::DirectionsRun;

/// The kinds of road the app tells apart, of an intersection's `classes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RoadClasses {
    pub(crate) motorway: bool,
    pub(crate) toll: bool,
    pub(crate) ferry: bool,
}

impl RoadClasses {
    /// The classes among an intersection's `classes`; the others (`tunnel`, `restricted`)
    /// are left out.
    pub(crate) fn of(classes: &[String]) -> Self {
        let has = |class: &str| classes.iter().any(|c| c == class);
        Self {
            motorway: has("motorway"),
            toll: has("toll"),
            ferry: has("ferry"),
        }
    }

    /// These and `other`'s together.
    pub(crate) fn or(self, other: Self) -> Self {
        Self {
            motorway: self.motorway || other.motorway,
            toll: self.toll || other.toll,
            ferry: self.ferry || other.ferry,
        }
    }
}

/// What a driver calls the road a step is on, from the step's `name` and `ref`: the
/// street's name if it has one, else its route number, else nothing.
///
/// The street's name comes first because it's what people who drive a numbered street
/// call it, and what its signs say; a motorway usually has only a number. Each of the two
/// can list several, `; `-separated, and the first is taken.
///
/// Valhalla writes route numbers as OpenStreetMap tags them, so an Interstate is
/// `I 5 North`, with the direction where the data has one. In the United States
/// (`in_united_states`) that is written as it's signed and said, `I-5 North`. Other
/// numbers are left as they are: a state route is `SR 99` whatever the state.
pub(crate) fn road_name(name: &str, refs: Option<&str>, in_united_states: bool) -> Option<String> {
    let first = |list: &str| {
        list.split(';')
            .map(str::trim)
            .find(|item| !item.is_empty())
            .map(str::to_owned)
    };
    if let Some(name) = first(name) {
        return Some(name);
    }
    let number = refs.and_then(first)?;
    Some(match interstate_number(&number) {
        Some(rest) if in_united_states => format!("I-{rest}"),
        _ => number,
    })
}

/// What follows `I ` in an Interstate's route number as OpenStreetMap tags it (`5 North`
/// of `I 5 North`); none for any other number.
fn interstate_number(number: &str) -> Option<&str> {
    number
        .strip_prefix("I ")
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// A step's road, for naming its route: what the road is called, if anything, and how far
/// the step goes along it, in meters.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StepRoad {
    pub(crate) name: Option<String>,
    pub(crate) length: f64,
}

/// The share of a route's length a road must exceed for the route to be named after it.
const SUBSTANTIAL_SHARE: f64 = 0.25;

/// The most roads a route is named after, before any are added to tell it from another.
const MOST_NAMES: usize = 3;

/// A route's roads in the order it first takes them, each with the length of all its
/// steps on that road, and which of them it's named after.
struct NamedRoute<'a> {
    roads: Vec<(&'a str, f64)>,
    /// Indices into `roads`, ascending: the order the roads are travelled in.
    chosen: Vec<usize>,
}

impl<'a> NamedRoute<'a> {
    /// The route of `steps`, named after each road that makes up more than a quarter of
    /// it, at most three; or after its longest road if none does; or after nothing if no
    /// step has a name.
    fn new(steps: &'a [StepRoad]) -> Self {
        let mut roads: Vec<(&str, f64)> = Vec::new();
        let mut total = 0.0;
        for step in steps {
            total += step.length;
            let Some(name) = step.name.as_deref() else {
                continue;
            };
            match roads.iter_mut().find(|(road, _)| *road == name) {
                Some((_, length)) => *length += step.length,
                None => roads.push((name, step.length)),
            }
        }
        // Longest first, and of equal lengths the one taken first.
        let mut by_length: Vec<usize> = (0..roads.len()).collect();
        by_length.sort_by(|&a, &b| roads[b].1.total_cmp(&roads[a].1));
        let mut chosen: Vec<usize> = by_length
            .iter()
            .copied()
            .filter(|&road| roads[road].1 > total * SUBSTANTIAL_SHARE)
            .take(MOST_NAMES)
            .collect();
        if chosen.is_empty() {
            chosen.extend(by_length.first());
        }
        chosen.sort_unstable();
        Self { roads, chosen }
    }

    /// The names of the roads it's named after, in the order they're travelled.
    fn names(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.chosen.iter().map(|&road| self.roads[road].0)
    }

    /// Whether any of its steps is on the road called `name`.
    fn uses(&self, name: &str) -> bool {
        self.roads.iter().any(|(road, _)| *road == name)
    }

    /// The longest of its roads that it isn't yet named after and `other` doesn't use at
    /// all, the first of them if several are as long; none if there's no such road.
    fn longest_apart_from(&self, other: &Self) -> Option<usize> {
        let mut longest: Option<usize> = None;
        for (road, &(name, length)) in self.roads.iter().enumerate() {
            if self.chosen.contains(&road) || other.uses(name) {
                continue;
            }
            if longest.is_none_or(|longest| length > self.roads[longest].1) {
                longest = Some(road);
            }
        }
        longest
    }

    /// Names it after `road` as well, keeping the names in the order they're travelled.
    fn add(&mut self, road: usize) {
        let at = self.chosen.partition_point(|&chosen| chosen < road);
        self.chosen.insert(at, road);
    }
}

/// What to call each of the routes an answer gives together, as the roads it's named
/// after in the order they're travelled: for each route, in `routes`' order, the names of
/// its roads, which the app joins into "via I-5 South and Mercer Street". A route none of
/// whose steps has a name gets no names.
///
/// From Headway's `substantialRoadNames`: the lengths of a route's steps are added up by
/// road, and the route is named after each road that makes up more than a quarter of its
/// length, at most three, or failing any after its longest. Unlike Headway's, the names
/// are in the order the roads are travelled rather than longest first, and routes that
/// would be named alike are told apart: while two routes have the same names, each of the
/// two also takes the name of its longest road that the other doesn't use anywhere, until
/// they differ or neither has such a road.
pub(crate) fn route_names(routes: &[Vec<StepRoad>]) -> Vec<Vec<String>> {
    let mut named: Vec<NamedRoute> = routes.iter().map(|steps| NamedRoute::new(steps)).collect();
    // Every pass that doesn't end this names some route after one more of its roads, so
    // it ends.
    loop {
        let mut added = false;
        for b in 1..named.len() {
            let (before, rest) = named.split_at_mut(b);
            let second = &mut rest[0];
            for first in before {
                if !first.names().eq(second.names()) {
                    continue;
                }
                let for_first = first.longest_apart_from(second);
                let for_second = second.longest_apart_from(first);
                if let Some(road) = for_first {
                    first.add(road);
                    added = true;
                }
                if let Some(road) = for_second {
                    second.add(road);
                    added = true;
                }
            }
        }
        if !added {
            break;
        }
    }
    named
        .iter()
        .map(|route| route.names().map(str::to_owned).collect())
        .collect()
}

/// An intersection as a leg's speed profile needs it: where along the leg's line it is,
/// and what leaving it is like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProfileIntersection {
    /// The index of its point in the leg's line (`geometry_index`).
    pub(crate) geometry_index: usize,
    /// The classes of the road leaving it.
    pub(crate) classes: RoadClasses,
    /// How long the turn onto that road takes, in seconds (`turn_duration`).
    pub(crate) turn_duration: f64,
}

/// A leg as its speed profile needs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ProfileLeg {
    /// How long each segment of the leg's line takes, in seconds (the annotation's
    /// `duration`).
    pub(crate) durations: Vec<f64>,
    /// How long each segment of the leg's line is, in meters (the annotation's
    /// `distance`).
    pub(crate) distances: Vec<f64>,
    /// Every step's intersections, in order.
    pub(crate) intersections: Vec<ProfileIntersection>,
}

/// How far apart, in meters per second, a stretch of road's speed and a run's can be for
/// the stretch to join the run: 3 m/s is about 11 km/h or 7 mph.
const RUN_SPEED_TOLERANCE: f64 = 3.0;

/// The share of a route's duration below which a run is too short to stand by itself: a
/// hundredth, which is a few pixels of a row's graph.
const SHORTEST_RUN_SHARE: f64 = 0.01;

/// A run being put together.
#[derive(Clone, Copy, Debug)]
struct Run {
    duration: f64,
    distance: f64,
    classes: RoadClasses,
}

impl Run {
    /// Its speed in meters per second; zero if it takes no time.
    fn speed(&self) -> f64 {
        if self.duration > 0.0 {
            self.distance / self.duration
        } else {
            0.0
        }
    }

    /// Takes `other` in.
    fn absorb(&mut self, other: &Run) {
        self.duration += other.duration;
        self.distance += other.distance;
    }
}

/// The stretches of road between one intersection of `leg` and the next, each with the
/// classes of the road leaving its intersection; none if the leg has no annotation.
///
/// The join is by position along the leg's line. The annotation has a duration and a
/// distance for each segment of the line, segment `k` running from point `k` to point
/// `k + 1`. An intersection's `geometry_index` is the point where the road leaving it
/// begins, and that road runs to the next intersection's point, whichever step that one
/// is in. So an intersection at point `g`, followed by one at point `h`, has segments `g`
/// to `h - 1`. The last intersection of a leg is its arrival, at the line's last point,
/// which has no segments.
///
/// The annotation's durations are of the roads alone. The time a turn takes is the
/// intersection's `turn_duration`, which is added to the stretch the turn leads onto, so
/// that the stretches' durations add up to the leg's. They do so nearly, not exactly:
/// the annotation times and measures each segment along the line, while the leg's own
/// duration and distance are of its roads, whose lengths the graph keeps to the whole
/// meter. Over a route the two differ by a fraction of a percent.
fn stretches(leg: &ProfileLeg) -> Option<Vec<Run>> {
    let segments = leg.durations.len().min(leg.distances.len());
    if segments == 0 {
        return None;
    }
    let between = |from: usize, to: usize, classes: RoadClasses, turn_duration: f64| Run {
        duration: leg.durations[from..to].iter().sum::<f64>() + turn_duration,
        distance: leg.distances[from..to].iter().sum(),
        classes,
    };
    let mut stretches = Vec::with_capacity(leg.intersections.len());
    // The point the stretches so far reach.
    let mut reached = 0;
    let mut leaving: Option<&ProfileIntersection> = None;
    let starts = leg
        .intersections
        .iter()
        .map(|intersection| (intersection.geometry_index, Some(intersection)))
        .chain([(segments, None)]);
    for (index, intersection) in starts {
        // A point before the last one or past the line's end is taken as the nearest
        // that keeps the stretches in order and on the line.
        let start = index.clamp(reached, segments);
        match leaving {
            Some(leaving) => stretches.push(between(
                reached,
                start,
                leaving.classes,
                leaving.turn_duration,
            )),
            // Line before the first intersection has no classes to go by.
            None if start > reached => {
                stretches.push(between(reached, start, RoadClasses::default(), 0.0));
            }
            None => {}
        }
        reached = start;
        leaving = intersection.or(leaving);
    }
    Some(stretches)
}

/// A route's speed profile: runs that follow one another from its start to its end, each
/// with how long it takes, how far it goes and what kind of road it's on. Empty if any of
/// the route's `legs` has no annotation, since the runs wouldn't cover the route.
///
/// The runs are put together from the stretches of road between intersections
/// ([`stretches`]), which a route has hundreds or thousands of, in two steps that leave
/// it some tens:
///
/// 1. A stretch joins the run before it when the two have the same classes and the
///    stretch's speed is within 3 m/s of the run's so far. A stretch with no length or
///    that takes no time has no speed, and joins the run before it whatever its classes
///    (or, at the route's start, the run after it).
/// 2. A run that takes less than a hundredth of the route's duration is too short to
///    draw, so it joins whichever neighbor of the same classes is nearer its speed (the
///    one before it if they're equally near). The shortest such run goes first, and this
///    repeats until every run left is long enough or has no neighbor of its classes.
///
/// Classes never mix, except that a stretch with no speed has none that matter: a run is
/// all motorway or not, all tolled or not, all ferry or not. Joining adds durations and
/// distances, so the runs' add up to the stretches'. A run's speed is its distance over
/// its duration.
pub(crate) fn speed_profile(legs: &[ProfileLeg]) -> Vec<DirectionsRun> {
    let mut runs: Vec<Run> = Vec::new();
    // Stretches without a speed that came before any run.
    let mut carried: Option<Run> = None;
    for leg in legs {
        let Some(stretches) = stretches(leg) else {
            return Vec::new();
        };
        for mut stretch in stretches {
            let has_speed = stretch.duration > 0.0 && stretch.distance > 0.0;
            match runs.last_mut() {
                Some(run) if !has_speed => run.absorb(&stretch),
                Some(run)
                    if run.classes == stretch.classes
                        && (run.speed() - stretch.speed()).abs() <= RUN_SPEED_TOLERANCE =>
                {
                    run.absorb(&stretch);
                }
                None if !has_speed => match carried.as_mut() {
                    Some(carried) => carried.absorb(&stretch),
                    None => carried = Some(stretch),
                },
                _ => {
                    if let Some(carried) = carried.take() {
                        stretch.absorb(&carried);
                    }
                    runs.push(stretch);
                }
            }
        }
    }
    // A route with no stretch that has a speed is one run all the same.
    if let Some(carried) = carried {
        runs.push(carried);
    }

    let shortest = runs.iter().map(|run| run.duration).sum::<f64>() * SHORTEST_RUN_SHARE;
    while let Some((run, neighbor)) = shortest_joinable(&runs, shortest) {
        let joined = runs.remove(run);
        // Removing the run moved the neighbor after it into its place.
        runs[neighbor.min(run)].absorb(&joined);
    }

    runs.iter()
        .map(|run| DirectionsRun {
            duration: run.duration,
            distance: run.distance,
            speed: run.speed(),
            motorway: run.classes.motorway,
            toll: run.classes.toll,
            ferry: run.classes.ferry,
        })
        .collect()
}

/// The shortest of `runs` that takes less than `shortest` seconds and has a neighbor of
/// its classes, with the neighbor it should join: the one nearer its speed, or the one
/// before it if they're equally near.
fn shortest_joinable(runs: &[Run], shortest: f64) -> Option<(usize, usize)> {
    let neighbor_of = |index: usize| {
        let run = &runs[index];
        let apart = |neighbor: &usize| (runs[*neighbor].speed() - run.speed()).abs();
        let before = index.checked_sub(1);
        let after = Some(index + 1).filter(|&after| after < runs.len());
        [before, after]
            .into_iter()
            .flatten()
            .filter(|&neighbor| runs[neighbor].classes == run.classes)
            .min_by(|a, b| apart(a).total_cmp(&apart(b)))
    };
    (0..runs.len())
        .filter(|&index| runs[index].duration < shortest)
        .filter_map(|index| Some((index, neighbor_of(index)?)))
        .min_by(|(a, _), (b, _)| runs[*a].duration.total_cmp(&runs[*b].duration))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_road_is_called_by_its_name_before_its_number() {
        // A motorway has only a number.
        assert_eq!(road_name("", Some("SR 99"), true).as_deref(), Some("SR 99"));
        // A street with a number is still the street.
        assert_eq!(
            road_name("Avinguda de Tarragona", Some("CG-1"), false).as_deref(),
            Some("Avinguda de Tarragona")
        );
        assert_eq!(
            road_name("Rainier Avenue South", None, true).as_deref(),
            Some("Rainier Avenue South")
        );
        // The first of several.
        assert_eq!(
            road_name("Main Street; High Street", Some("US 2"), true).as_deref(),
            Some("Main Street")
        );
        assert_eq!(
            road_name("", Some("US 2; SR 9"), true).as_deref(),
            Some("US 2")
        );
        // An empty name is no name.
        assert_eq!(
            road_name(" ; ", Some("SR 513"), true).as_deref(),
            Some("SR 513")
        );
        assert_eq!(road_name("", None, true), None);
        assert_eq!(road_name("", Some(" ; "), false), None);
    }

    #[test]
    fn an_interstate_is_written_as_signed_in_the_united_states() {
        let named =
            |number: &str, in_united_states| road_name("", Some(number), in_united_states).unwrap();
        assert_eq!(named("I 5 North", true), "I-5 North");
        assert_eq!(named("I 405", true), "I-405");
        assert_eq!(named("I 35E South; US 77", true), "I-35E South");
        // Elsewhere it's some other road.
        assert_eq!(named("I 5 North", false), "I 5 North");
        // Other numbers stay as tagged.
        assert_eq!(named("SR 520", true), "SR 520");
        assert_eq!(named("US 2", true), "US 2");
        assert_eq!(named("I Street", true), "I Street");
        assert_eq!(named("I-90", true), "I-90");
    }

    fn step(name: &str, length: f64) -> StepRoad {
        StepRoad {
            name: (!name.is_empty()).then(|| name.to_owned()),
            length,
        }
    }

    fn names(routes: &[&[(&str, f64)]]) -> Vec<Vec<String>> {
        let routes: Vec<Vec<StepRoad>> = routes
            .iter()
            .map(|steps| {
                steps
                    .iter()
                    .map(|&(name, length)| step(name, length))
                    .collect()
            })
            .collect();
        route_names(&routes)
    }

    #[test]
    fn a_route_is_named_after_roads_over_a_quarter_of_it_in_travel_order() {
        // The shorter of the two comes first because it's travelled first, and a road's
        // steps count together wherever they are.
        assert_eq!(
            names(&[&[
                ("Pine Street", 100.0),
                ("Mercer Street", 150.0),
                ("I-5 North", 500.0),
                ("", 50.0),
                ("Mercer Street", 150.0),
                ("Market Street", 50.0),
            ]]),
            [["Mercer Street", "I-5 North"]]
        );
    }

    #[test]
    fn a_road_of_exactly_a_quarter_is_not_enough() {
        assert_eq!(
            names(&[&[("A", 250.0), ("B", 251.0), ("C", 499.0)]]),
            [["B", "C"]]
        );
    }

    #[test]
    fn the_quarter_is_of_the_whole_route_with_its_unnamed_steps() {
        // "A" is over a quarter of the named length, but not of the route.
        assert_eq!(
            names(&[&[("A", 300.0), ("", 500.0), ("B", 700.0)]]),
            [["B"]]
        );
    }

    #[test]
    fn at_most_three_roads_name_a_route() {
        // Three is all that can each be over a quarter.
        assert_eq!(
            names(&[&[
                ("A", 260.0),
                ("B", 30.0),
                ("C", 270.0),
                ("D", 280.0),
                ("E", 160.0)
            ]]),
            [["A", "C", "D"]]
        );
    }

    #[test]
    fn a_route_with_no_substantial_road_is_named_after_its_longest() {
        assert_eq!(
            names(&[&[
                ("A", 100.0),
                ("B", 200.0),
                ("C", 150.0),
                ("D", 200.0),
                ("E", 150.0),
                ("F", 100.0),
            ]]),
            // Of the two longest, the one taken first.
            [["B"]]
        );
    }

    #[test]
    fn a_route_with_no_named_roads_has_no_names() {
        assert_eq!(names(&[&[("", 400.0), ("", 20.0)]]), [Vec::<String>::new()]);
        assert_eq!(names(&[&[]]), [Vec::<String>::new()]);
        assert!(names(&[]).is_empty());
    }

    #[test]
    fn routes_named_alike_each_add_their_longest_road_the_other_lacks() {
        let first: &[(&str, f64)] = &[
            ("Pine Street", 150.0),
            ("Boren Avenue", 100.0),
            ("I-5 North", 700.0),
            ("Mercer Street", 50.0),
        ];
        let second: &[(&str, f64)] = &[
            ("Pine Street", 150.0),
            ("I-5 North", 600.0),
            ("Roy Street", 60.0),
            ("Mercer Street", 50.0),
            ("Dexter Avenue", 140.0),
        ];
        // Both are "I-5 North" alone. Pine Street is the longest other road of each, but
        // both use it, so each adds the longest of its own, where it's travelled.
        assert_eq!(
            names(&[first, second]),
            [
                vec!["Boren Avenue", "I-5 North"],
                vec!["I-5 North", "Dexter Avenue"],
            ]
        );
        // Routes that already differ are left alone.
        let third: &[(&str, f64)] = &[("Aurora Avenue", 900.0), ("Pine Street", 100.0)];
        assert_eq!(
            names(&[first, third]),
            [vec!["I-5 North"], vec!["Aurora Avenue"]]
        );
    }

    #[test]
    fn only_the_route_with_a_road_of_its_own_adds_one() {
        let short: &[(&str, f64)] = &[("Pine Street", 100.0), ("I-5 North", 900.0)];
        let long: &[(&str, f64)] = &[
            ("Pine Street", 100.0),
            ("I-5 North", 900.0),
            ("Mercer Street", 200.0),
        ];
        assert_eq!(
            names(&[short, long]),
            [vec!["I-5 North"], vec!["I-5 North", "Mercer Street"]]
        );
    }

    #[test]
    fn routes_with_the_same_roads_keep_the_same_names() {
        let route: &[(&str, f64)] = &[("Pine Street", 100.0), ("I-5 North", 900.0)];
        assert_eq!(
            names(&[route, route]),
            [vec!["I-5 North"], vec!["I-5 North"]]
        );
        // Nor can routes with no names be told apart.
        let unnamed: &[(&str, f64)] = &[("", 100.0)];
        assert_eq!(
            names(&[unnamed, unnamed]),
            [Vec::<String>::new(), Vec::new()]
        );
    }

    #[test]
    fn a_road_added_to_tell_two_routes_apart_can_set_off_another() {
        // The first two are both "A". The first has "B" to add and the second nothing, which
        // leaves the first named as the third is; those two then each add their own.
        let first: &[(&str, f64)] = &[("A", 600.0), ("B", 200.0), ("C", 50.0)];
        let second: &[(&str, f64)] = &[("A", 600.0)];
        let third: &[(&str, f64)] = &[("A", 500.0), ("B", 400.0), ("D", 50.0)];
        assert_eq!(
            names(&[first, second, third]),
            [vec!["A", "B", "C"], vec!["A"], vec!["A", "B", "D"]]
        );
    }

    fn classes(motorway: bool, toll: bool, ferry: bool) -> RoadClasses {
        RoadClasses {
            motorway,
            toll,
            ferry,
        }
    }

    const STREET: RoadClasses = RoadClasses {
        motorway: false,
        toll: false,
        ferry: false,
    };
    const MOTORWAY: RoadClasses = RoadClasses {
        motorway: true,
        toll: false,
        ferry: false,
    };

    #[test]
    fn classes_are_read_from_an_intersections_list() {
        let list = |classes: &[&str]| -> Vec<String> {
            classes.iter().map(|&class| class.to_owned()).collect()
        };
        assert_eq!(RoadClasses::of(&[]), STREET);
        assert_eq!(RoadClasses::of(&list(&["tunnel", "restricted"])), STREET);
        assert_eq!(
            RoadClasses::of(&list(&["tunnel", "toll", "motorway"])),
            classes(true, true, false)
        );
        assert_eq!(
            RoadClasses::of(&list(&["ferry"])),
            classes(false, false, true)
        );
        assert_eq!(
            MOTORWAY.or(classes(false, false, true)),
            classes(true, false, true)
        );
    }

    fn at(geometry_index: usize, classes: RoadClasses, turn_duration: f64) -> ProfileIntersection {
        ProfileIntersection {
            geometry_index,
            classes,
            turn_duration,
        }
    }

    /// A run as `(duration, distance, classes)`.
    fn summary(runs: &[DirectionsRun]) -> Vec<(f64, f64, RoadClasses)> {
        runs.iter()
            .map(|run| {
                (
                    run.duration,
                    run.distance,
                    classes(run.motorway, run.toll, run.ferry),
                )
            })
            .collect()
    }

    #[test]
    fn segments_take_the_classes_of_the_intersection_before_them() {
        // Six segments at 10 m/s. The street's intersections are at points 0 and 2, the
        // motorway begins at point 3 and has another at 4, and the arrival is at point 6.
        let leg = ProfileLeg {
            durations: vec![10.0; 6],
            distances: vec![100.0; 6],
            intersections: vec![
                at(0, STREET, 0.0),
                at(2, STREET, 0.0),
                at(3, MOTORWAY, 0.0),
                at(4, MOTORWAY, 0.0),
                at(6, STREET, 0.0),
            ],
        };
        let runs = speed_profile(&[leg]);
        assert_eq!(
            summary(&runs),
            [(30.0, 300.0, STREET), (30.0, 300.0, MOTORWAY)]
        );
        assert_eq!(runs[0].speed, 10.0);
    }

    #[test]
    fn a_turns_time_goes_to_the_stretch_it_leads_onto() {
        let leg = ProfileLeg {
            durations: vec![10.0, 10.0, 5.0, 5.0],
            distances: vec![100.0; 4],
            intersections: vec![
                at(0, STREET, 0.0),
                // Ten seconds waiting to turn onto the faster road.
                at(2, STREET, 10.0),
                at(4, STREET, 0.0),
            ],
        };
        // 10 m/s, then 200 m in 20 s with the turn: 10 m/s too.
        let runs = speed_profile(&[leg]);
        assert_eq!(summary(&runs), [(40.0, 400.0, STREET)]);
    }

    #[test]
    fn stretches_join_when_their_speeds_are_within_three_meters_a_second() {
        let stretch = |speed: f64| (100.0 / speed, 100.0);
        let profile = |speeds: &[f64]| {
            let (durations, distances): (Vec<f64>, Vec<f64>) =
                speeds.iter().map(|&speed| stretch(speed)).unzip();
            let intersections = (0..=speeds.len())
                .map(|index| at(index, STREET, 0.0))
                .collect();
            speed_profile(&[ProfileLeg {
                durations,
                distances,
                intersections,
            }])
        };
        // 10 and 12.5 m/s join; 20 m/s is too far from the two together.
        let runs = profile(&[10.0, 12.5, 20.0]);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].distance, 200.0);
        assert_eq!(runs[0].duration, 18.0);
        assert!((runs[0].speed - 200.0 / 18.0).abs() < 1e-9);
        assert_eq!(runs[1].speed, 20.0);
        // It's the run's speed so far that counts: each of these is within 3 m/s of the
        // one before, but the last is not within 3 m/s of the run.
        let runs = profile(&[10.0, 12.0, 13.5, 16.0]);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].distance, 300.0);
    }

    #[test]
    fn stretches_of_different_classes_never_join() {
        let leg = ProfileLeg {
            durations: vec![10.0; 4],
            distances: vec![100.0; 4],
            intersections: vec![
                at(0, STREET, 0.0),
                at(1, classes(false, true, false), 0.0),
                at(2, classes(true, true, false), 0.0),
                at(3, classes(false, false, true), 0.0),
                at(4, STREET, 0.0),
            ],
        };
        assert_eq!(
            summary(&speed_profile(&[leg])),
            [
                (10.0, 100.0, STREET),
                (10.0, 100.0, classes(false, true, false)),
                (10.0, 100.0, classes(true, true, false)),
                (10.0, 100.0, classes(false, false, true)),
            ]
        );
    }

    #[test]
    fn a_run_under_a_hundredth_of_the_route_joins_the_neighbor_nearer_its_speed() {
        // 1000 s in all, so a run under 10 s is too short. The 5 s at 2 m/s sits between
        // 10 m/s and 20 m/s, and joins the nearer.
        let leg = ProfileLeg {
            durations: vec![495.0, 5.0, 500.0],
            distances: vec![4950.0, 10.0, 10000.0],
            intersections: vec![
                at(0, STREET, 0.0),
                at(1, STREET, 0.0),
                at(2, STREET, 0.0),
                at(3, STREET, 0.0),
            ],
        };
        assert_eq!(
            summary(&speed_profile(std::slice::from_ref(&leg))),
            [(500.0, 4960.0, STREET), (500.0, 10000.0, STREET)]
        );
        // A neighbor of other classes won't do, however near.
        let mut tolled = leg.clone();
        tolled.intersections[0].classes = classes(false, true, false);
        assert_eq!(
            summary(&speed_profile(&[tolled])),
            [
                (495.0, 4950.0, classes(false, true, false)),
                (505.0, 10010.0, STREET)
            ]
        );
        // With neither, it stays, short as it is.
        let mut alone = leg;
        alone.intersections[1].classes = classes(false, false, true);
        assert_eq!(speed_profile(&[alone]).len(), 3);
    }

    #[test]
    fn short_runs_join_shortest_first() {
        // 1000 s in all. The 2 s run joins the 4 s run beside it, nearer its speed than
        // the long run on its other side, and the two together (6 s) are still too short,
        // so they join the long run nearer theirs.
        let leg = ProfileLeg {
            durations: vec![494.0, 2.0, 4.0, 500.0],
            distances: vec![4940.0, 60.0, 100.0, 15000.0],
            intersections: (0..=4).map(|index| at(index, STREET, 0.0)).collect(),
        };
        assert_eq!(
            summary(&speed_profile(&[leg])),
            [(494.0, 4940.0, STREET), (506.0, 15160.0, STREET)]
        );
    }

    #[test]
    fn a_stretch_with_no_speed_joins_the_run_beside_it() {
        // Two intersections at one point: the second's turn takes time over no distance.
        // It joins the run before it, though that one is a motorway and it is not. So
        // does a stretch whose segment's time rounded to nothing.
        let leg = ProfileLeg {
            durations: vec![10.0, 0.0, 10.0],
            distances: vec![300.0, 0.5, 100.0],
            intersections: vec![
                at(0, MOTORWAY, 0.0),
                at(1, STREET, 2.0),
                at(1, STREET, 0.0),
                at(2, STREET, 0.0),
                at(3, STREET, 0.0),
            ],
        };
        assert_eq!(
            summary(&speed_profile(&[leg])),
            [(12.0, 300.5, MOTORWAY), (10.0, 100.0, STREET)]
        );
        // At the route's start there's none before it, so it joins the first after.
        let leg = ProfileLeg {
            durations: vec![10.0],
            distances: vec![100.0],
            intersections: vec![at(0, MOTORWAY, 3.0), at(0, STREET, 0.0), at(1, STREET, 0.0)],
        };
        assert_eq!(summary(&speed_profile(&[leg])), [(13.0, 100.0, STREET)]);
        // A route that never moves is one run without a speed.
        let leg = ProfileLeg {
            durations: vec![4.0],
            distances: vec![0.0],
            intersections: vec![at(0, STREET, 1.0), at(1, STREET, 0.0)],
        };
        let runs = speed_profile(&[leg]);
        assert_eq!(summary(&runs), [(5.0, 0.0, STREET)]);
        assert_eq!(runs[0].speed, 0.0);
    }

    #[test]
    fn the_whole_line_is_covered_whatever_the_intersections_say() {
        let leg = |intersections: Vec<ProfileIntersection>| ProfileLeg {
            durations: vec![10.0; 4],
            distances: vec![100.0; 4],
            intersections,
        };
        // No intersections at all, or none at the start: the line before has no classes.
        assert_eq!(
            summary(&speed_profile(&[leg(Vec::new())])),
            [(40.0, 400.0, STREET)]
        );
        assert_eq!(
            summary(&speed_profile(&[leg(vec![at(2, MOTORWAY, 0.0)])])),
            [(20.0, 200.0, STREET), (20.0, 200.0, MOTORWAY)]
        );
        // One past the end of the line, and one out of order, are brought into line.
        assert_eq!(
            summary(&speed_profile(&[leg(vec![
                at(0, STREET, 0.0),
                at(3, MOTORWAY, 0.0),
                at(1, classes(false, true, false), 0.0),
                at(9, STREET, 0.0),
            ])])),
            [
                (30.0, 300.0, STREET),
                (10.0, 100.0, classes(false, true, false))
            ]
        );
    }

    #[test]
    fn legs_follow_one_another_and_a_leg_without_an_annotation_leaves_no_runs() {
        let leg = |classes: RoadClasses| ProfileLeg {
            durations: vec![10.0; 2],
            distances: vec![100.0; 2],
            intersections: vec![at(0, classes, 0.0), at(2, STREET, 0.0)],
        };
        assert_eq!(
            summary(&speed_profile(&[leg(STREET), leg(STREET), leg(MOTORWAY)])),
            [(40.0, 400.0, STREET), (20.0, 200.0, MOTORWAY)]
        );
        let bare = ProfileLeg {
            intersections: vec![at(0, STREET, 0.0), at(2, STREET, 0.0)],
            ..ProfileLeg::default()
        };
        assert!(speed_profile(&[leg(STREET), bare]).is_empty());
        assert!(speed_profile(&[]).is_empty());
    }
}
