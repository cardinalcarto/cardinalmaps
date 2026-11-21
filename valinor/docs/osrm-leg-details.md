# Leg details in the OSRM format

An extension of Valinor's to the OSRM route format, which mainline Valhalla doesn't have:
on request, each leg says what the road is like along it, edge by edge,
and gives its elevation.
Cardinal's directions draw both along a route
(see `flutter/cardinal/docs/directions-picker.md` in the Cardinal repository).
The code is `valinor-tyr/src/tyr/osrm/details.rs`.

The OSRM format has neither.
A step's intersections have `classes`, but only `tunnel`, `toll`, `motorway`, `ferry`,
and `restricted`, and `toll` is on every intersection of a maneuver any part of which is tolled
(mainline's `portions_toll`; its own response for SR 99 through Seattle marks a 7.8 km step
whose tolled part is 3.8 km).
Elevation is only in the Valhalla format.

## Asking for it

| Request option | Type | Default | Meaning |
|---|---|---|---|
| `valinor_leg_details` | boolean | `false` | Each leg of an OSRM-format response gets `valinor_details`. |
| `elevation_interval` | number, 0 to 1000 | 0 | Mainline's option, in meters: when positive, the details include the leg's elevation sampled at it. |

`valinor_leg_details` is Valinor's own name.
Neither Valhalla nor OSRM has an option that starts with `valinor_`,
and mainline ignores keys it doesn't know, so the same request works against mainline,
without the details.
It is carried in `Options.valinor_leg_details`, a field of ours in mainline's message
(see `valhalla-proto/proto/README.md`).

`elevation_interval` is mainline's, with mainline's meaning and limits.
In the OSRM format mainline reads it and writes nothing for it, and so does Valinor
unless the details are asked for too.

The option applies wherever legs are written in the OSRM format:
`route`, `trace_route`, and `optimized_route`.
In the other formats it does nothing.
With it off, the response is mainline's;
with it on, taking `valinor_details` out of each leg leaves the same response
(`the_details_are_all_that_asking_for_them_changes`, over every captured case).

## The response

Each leg (`routes[].legs[]`, or `matchings[].legs[]`) gains one member:

```json
"valinor_details": {
  "runs": [
    {"duration": 28.724, "distance": 115.0, "speed": 4.00,
     "travel_mode": "drive", "road_class": "secondary", "use": "ramp", "toll": true},
    {"duration": 159.390, "distance": 3187.0, "speed": 19.99,
     "travel_mode": "drive", "road_class": "motorway", "use": "road",
     "toll": true, "tunnel": true}
  ],
  "elevation_interval": 30.0,
  "elevation": [21.4, 22.0, 24.9]
}
```

### `runs`

The leg from its start to its end, in travel order, with no gaps:
each run is one or more consecutive edges of the trip leg that are alike in everything below
but `duration`, `distance`, and `speed`, and that the costing gives the same speed
(the trip's `edge.speed`, exactly).
So edges whose speeds differ at all stay apart, and nothing is joined because speeds are close:
which speeds are near enough to draw as one is for the client to decide.
A drive across Seattle of 377 edges is 52 runs; a bicycle ride of 319 edges, whose speed
follows the grade, is 171.
A leg with no edges has an empty list.

| Member | Type | Meaning |
|---|---|---|
| `duration` | number, seconds, 3 decimal places | The time on the run's edges, with the time of the turn onto each. |
| `distance` | number, meters, 1 decimal place | The length of the run's edges, of the part of each the leg uses. |
| `speed` | number, meters per second, 2 decimal places | `distance / duration`, or 0 for a run that takes no time. With the turns, so less than the costing's speed for the edges. |
| `travel_mode` | string | `drive`, `pedestrian`, `bicycle`, or `transit`. |
| `road_class` | string | `motorway`, `trunk`, `primary`, `secondary`, `tertiary`, `unclassified`, `residential`, or `service_other`. |
| `use` | string | What the way is for, as below. |
| `cycle_lane` | string | Only when `travel_mode` is `bicycle`: `none`, `shared`, `dedicated`, or `separated`. |
| `bicycle_network` | boolean | Only when `travel_mode` is `bicycle`: whether the edge is on a bicycle network. |
| `toll` | `true` | Only on a tolled run. |
| `tunnel` | `true` | Only on a run in a tunnel. |
| `bridge` | `true` | Only on a run on a bridge. |

The strings are the ones `trace_attributes` gives an edge for the same attributes,
and a value mainline has no name for is `null` (the string), as there.
`use` is one of `road`, `ramp`, `turn_channel`, `track`, `driveway`, `alley`, `parking_aisle`,
`emergency_access`, `drive_through`, `culdesac`, `living_street`, `service_road`, `cycleway`,
`mountain_bike`, `sidewalk`, `footway`, `steps`, `path`, `pedestrian`, `bridleway`, `rest_area`,
`service_area`, `pedestrian_crossing`, `elevator`, `escalator`, `platform`, `other`, `ferry`,
`rail-ferry`, `construction`, `rail`, `bus`, `egress_connection`, `platform_connection`,
and `transit_connection`.
A ferry crossing is a run whose `use` is `ferry` or `rail-ferry`; there is no separate flag.

The values are read from the trip's edges, as `trace_attributes` reads them.
A request whose `filters` leave an edge attribute out of the trip
(`edge.road_class`, `edge.use`, `edge.toll`, `edge.speed`, `edge.length`, and so on,
all on by default) gets that attribute's default in every run,
and one that leaves out `node.elapsed_time` gets durations of 0.

**Sums.** An edge's time is the elapsed time at its end less that at its start,
which is how the leg's own `duration` is counted, so the turn onto an edge is in its run.
Each `duration` and `distance` is written as the difference of the rounded totals
at the run's two ends, so rounding doesn't add up along a leg:

- the runs' durations add up to the leg's `duration` to the millisecond it is written to;
- the runs' distances add up to the leg's `distance` to within a meter and a part in 100,000.
  They can't be exact: the leg's own distance is Odin's sum of its maneuvers' lengths
  in single precision, in the request's units.
  The worst over the captured routes is 0.07 m.

### `elevation` and `elevation_interval`

Present together, and only when the request's `elevation_interval` is positive
and the graph has the elevation of the whole leg.

- `elevation_interval`: the request's interval after mainline's clamping, in meters.
- `elevation`: heights in meters above sea level, to 1 decimal place:
  one at the leg's start, one every `elevation_interval` meters along it, and one at its end
  (which is less than an interval past the one before).
  They are the Valhalla format's (`tyr::get_elevation`), with runs of bridges and tunnels
  interpolated, except that they are always in meters, as the rest of the OSRM format is,
  and rounded rather than cut off at the decimal place.

A leg has no elevation, and the response is otherwise whole, when any of its edges
has no samples or a sample at −500 m, the height the graph gives a place it has none for
(a tile built without elevation).
The Valhalla format fails the request in the first case and returns −500 in the second.
