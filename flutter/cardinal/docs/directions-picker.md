# Directions as an alternate picker

Status: design agreed 2026-11-20. Being built on `directions-picker` in the order at the
end of this document; steps 1 to 3 are done. Mockups (invented trip and numbers):
https://claude.ai/artifact/LbDfkfMsy2JBsvkoBvJTk3

## Goal

The directions screen stops being "one route and its steps" and becomes the place to see
what the options are and pick one. Steps move to their own screen. Transit no longer
holds the sheet at full height.

Every option, in every mode, is the same kind of row. The selected row stays on screen at
every sheet height and can be scrubbed to explore the route on the map.

Swiping sideways to change mode was considered and dropped: horizontal drags belong to
scrubbing. It may be revisited.

## The screen

Top to bottom, in the sheet:

1. **Trip card.** From and To as two 44 px rows in one filled card. To its right,
   outside the card so neither reads as part of a field: X above, swap below. No
   "Directions" title.
   - X leaves directions for the place's card, as the close button does today. It does
     not clear the From field.
   - Tapping From or To opens the existing search for that end: sheet full, keyboard up,
     the field's text selected. That search field's own clear button empties the end.
   - Swap exchanges the ends.
2. **Mode tabs.** Four equal tabs (driving, transit, walking or wheelchair, cycling),
   each an icon over that mode's best time. Tapping one shows its options.
3. **Options.** The mode's rows. Transit has a line above its rows: live status on the
   left ("Live times"), how you reach stops on the right ("Walking to stops", a menu of
   walking, cycling and wheelchair), then the time axis.
4. **Buttons**, pinned to the bottom of the window at every sheet height: Steps and Start
   for street modes, Trip details for transit. They act on the selected row.

## Option rows

One row shape for all modes, and one row height for all modes. The height matters: the
docked row sits under the tabs at the header anchor, so rows of different heights would
move the tabs when one is tapped there.

| Line | Street modes | Transit |
|---|---|---|
| Headline | Duration, large. Arrival time on the right. | "Go in 4 min" and the clock range. Duration on the right. |
| Name | "via I-5 South" | none: the timeline's route badges name it |
| Graph | 32 px tall and as wide as the route's share of the slowest option's time. Driving: a speed profile. Cycling and walking: an elevation profile over a bar in three tones | The journey's timeline on the list's shared time axis, with a lane above it for "or 4, 13" |
| Caption | Distance, then how it differs from the best option | none |

- **Shared axis.** Within a list, graph width is time. A slower option is visibly longer.
  Transit already works this way (`TimelineAxis.of`).
- **Tones**, dark to light, are the mode's best infrastructure to its worst.
  Driving: freeway, main roads, streets. Cycling: trail, bike lane, shared road. Walking:
  paths, sidewalks and streets, then stairs flagged in the caption. A legend follows the
  last row.
- **Driving's speed profile.** Height is the router's expected speed along the route,
  and the fill takes the road type's tone, so there is no separate bar. It's drawn as
  steps with rounded corners and fillets (3 px, less where a step is smaller), and no
  turn ticks. A tolled stretch has "Toll" written on it (from step 7 on; see "Data the
  rows need"); if the stretch is too narrow for the word, the caption says "Tolls"
  instead. These are expected speeds, not
  traffic, and nothing on the row should suggest they're live.
- **Cycling and walking** draw the elevation profile along the route, with the tone bar
  under it.
- **"or" lane.** Every transit row reserves it, with or without alternatives, and the
  label starts at its ride's left edge and may run as wide as it needs. It never goes
  inline in the timeline.
- **Caption.** One line, cut with an ellipsis, built in this order: distance; "Fastest"
  on the first row or "N min slower" on the others; "Shortest"; then mode facts.
  Driving: "Ferry", "No freeways" when another option has them, and "Tolls" only when
  the profile can't carry the label. Cycling and
  walking: climb, "Least climbing" on the row that has it, "Mostly trails" over half.
- **Selected row.** A tonal fill and the headline in the primary color. Nothing else
  changes, so selecting never moves anything.
- **Order.** As the router returns them, fastest first. A list never reorders while it
  shows.

### Naming a route

From Headway's `substantialRoadNames` (`services/frontend/www-app/src/models/Route.ts`),
with two changes.

1. Take each maneuver's first street name and its length. Sum the lengths of maneuvers
   that share a name.
2. Keep the names longer than a quarter of the route, at most three. If none qualify,
   keep the longest.
3. Changed: list them in the order they're driven, not by length.
4. Changed: while two routes in the list have the same name, add to each its longest
   road that the other doesn't use.
5. Join with the locale's list format: "via WA-99 South and WA-509".

A route with no named roads (a walk across a park) has no name line; the row keeps its
height.

### Large text

Text is never cut vertically. A row's height comes from the system text size, not a
fixed number of pixels: what the design needs is that every row, placeholder and message
box in every mode is the same height, and that still holds at any one text size. A
change of text size moves the anchors once, as a rotation does.

- Each line reserves one line at the current text size. The graph stays 32 px.
- At 1.5× and above every row takes a taller layout: the arrival time moves to its own
  line under the duration, and the name and the caption get two lines each. The switch
  depends only on the text size, never on what a row holds.
- Truncation is horizontal only. The duration and arrival never truncate. The name and
  caption end in an ellipsis only after their lines, and the caption's order is its
  priority, so the least important fact goes first.
- Whatever is cut is still reachable: the Steps page wraps without limit, and screen
  readers get the full text.
- Tab labels stop growing at 1.3×, as the platforms' own bars do. A long press shows
  the enlarged label on iOS and the tooltip on Android.
- The buttons and the trip card's rows grow with the text. Place names stay one line.

At the largest sizes the docked row is tall, so the lowest height covers more of the
map and on a small phone the header anchor may reach full height.

A test builds the page at 1×, 1.3×, 1.5×, 2× and 3.1× in every mode, on a regular and a
small window, and fails on any overflow or any text taller than its box.

## Mode tabs and loading

- All four modes are requested when the trip is set, in parallel. The screen never waits
  for any of them.
- Each tab reserves its time line. A time fades into its slot when its plan returns. A
  mode with no result shows a dash.
- A tab's time is the fastest route's duration, or for transit the time from now until
  the earliest arrival.
- The current mode shows placeholder rows of the real row height until its options
  arrive, then their content fades in. Nothing shifts.
- Each mode remembers its selection while the trip is unchanged. Changing an end, or
  swapping, plans again and selects each mode's first row.

## Selection

- Tapping a row selects it. Tapping a route's callout on the map selects it too.
- Street modes draw every option on the map, the selected one highlighted.
- Transit draws only the selected journey, which is why it takes a tap to select and
  then Trip details to open, rather than opening on the first tap.
- There is always a selection once a mode has options: its first row to begin with.

## The sheet

Not collapsible (as today). Lowest to highest:

| Rest | Shows |
|---|---|
| Lowest anchor | The handle directly on the selected row, the buttons under it |
| Header anchor | Handle, trip card and mode tabs, then the selected row directly under the tabs, then the buttons |
| Free range, header anchor to full | Rests wherever it's let go; at full the list scrolls |

Both anchors are measured to the top of the docked row. Between them the sheet springs to
the nearer one, so the trip card is never left half covered.

It opens in the free range showing the header and the first three rows, or all of them
if there are fewer, capped at about 60% of the window.

### Docking

One rule: **the selected row is never below the top of the buttons.**

- While its place in the list is in view, it's an ordinary row.
- When the sheet lowers past that, the row stops above the buttons. Everything else
  keeps travelling with the sheet and passes behind it: lower rows, upper rows, the
  tabs, the trip card.
- Raising the sheet reverses it: the row holds still until its place in the list rises
  to meet it, then travels with the list.
- At full height the same rule holds at the top: scrolled past, the row stops at the top
  of the content and the list passes behind it.

A docked row has a soft shadow on the edge the list passes behind. The shadow fades in
over the first 8 px of docking, so it never pops.

### When the dock changes without a drag

The dock animates whenever the person needs to know its state changed. A drag needs no
animation, since the row is under their control. All durations 200 ms, ease out.

| State | Event | What happens |
|---|---|---|
| Selection in view, nothing docked | A callout is tapped for a route whose row is below the buttons | The new row slides up from behind the buttons and docks. The old row loses its fill. The list doesn't move. |
| Row docked | A visible row, or its callout, is selected | The docked row slides down behind the buttons. The newly selected row takes its fill in place. |
| Row docked | A callout is tapped for another route that's also out of view | The docked row's content cross-fades in place. |
| Lowest or header anchor | A callout is tapped | As above: cross-fade in place. The row is always docked here. |
| Free range | A row cut off by the buttons is tapped | It takes its fill at once and, when the finger lifts, rises into the dock to sit whole above the buttons, covering part of the row above it. The list doesn't move and the tabs keep their height. |
| Header anchor | Another mode's tab is tapped | The docked row cross-fades to that mode's selected row. Same height, so the tabs don't move. |
| Free range | Another mode's tab is tapped | The list cross-fades. If the new selection is out of view it's docked when the fade ends, without a slide. |
| Any | The mode's options are loading | A placeholder row holds the dock. Content fades in when they arrive. |
| Any | No route, or the plan failed | The message takes the row's place, the same height. The buttons are disabled. |
| Any | An end changes or the trip is swapped | Rows cross-fade to placeholders, then to the new options, first row selected. |
| Any | A field is tapped | Editing replaces the content as it does today. The docked row and buttons fade out, and back in when editing ends. |
| Scrubbing a row cut off by the buttons | The scrub starts | The row stays where it is until the scrub ends, then rises as for a tap. |

## Scrubbing

- **Where.** The band and bar of any row, padded to at least 44 px tall. A drag that
  starts horizontally there scrubs. One that starts vertically moves the sheet or
  scrolls, as anywhere else.
- **Selects.** Scrubbing an unselected row selects it when the scrub starts.
- **Hairline.** A vertical line at the finger, across the band and bar. The graph's
  width is time, so the position is a time into the trip.
- **Readout.** While scrubbing it replaces the text on the right of the headline, which
  is above the finger. It fades back on release.
  - Driving: distance so far, the road and the expected speed,
    "Mile 6.2 · I-5 South · 58 mph".
  - Cycling and walking: distance, elevation and grade, "Mile 4.1 · 330 ft · 6% uphill".
  - Transit: the clock time and what's happening then, "3:41 · on 1 Line",
    "3:31 · walking", "3:27 · waiting at Westlake".
- **Map.** A marker moves along the selected route. The camera holds still unless the
  marker leaves the visible map, clear of the sheet, and then pans gently to bring it
  back inside. It doesn't return on release.
- **After.** The marker fades one second after release.
- **Without the gesture.** The graph is a slider to screen readers: it steps along the
  route and speaks the readout.

## Map

Unchanged except as said above: callouts sit at the visible point farthest from the
other routes' lines, route lines themselves aren't tappable yet, and the camera keeps
routes clear of the sheet.

## Steps and trip details

- **Steps** is a pushed sheet page: back arrow, the route's headline and name, distance
  and arrival, the maneuvers with their distances, and Start pinned at the bottom. Back
  returns to the picker at the height it was left.
- **Trip details** is today's journey view (itinerary, departures, connection
  alternatives) as a pushed sheet page like Steps, reached from the button instead of a
  tap on the row. It stops being a state of the directions page.

## Sheet library: recommendation for the anchors

The problem: both anchors rest with a row and the buttons showing below the content the
anchor names, and that row is one that otherwise lives in the list.

What the library does today (`lib/sheet/src/`):

- `SheetAnchor.section(id)` resolves through `SheetGeometry.extentShowing(y)`: the
  extent that shows the content down to `y` above `bottomInset`.
- `bottomInset` is the navigation bar plus the scaffold's bottom bar, or the keyboard.
  A page opts into the bottom bar with `showsBottomBar`, which is how nav-bar pages get
  anchors that clear the bar.
- The viewport is as tall as the content shows at the highest rest and moves with the
  sheet, so a sliver pinned to the viewport's bottom is off screen whenever the sheet is
  lowered. A pinned sliver can't do this.
- The first rest must be an anchor (`initialAnchor`), and no anchor may lie inside a
  free range.

Built (step 1), as general additions to the library:

- **`SheetContent.footer`.** A widget the library keeps at the bottom of the window (or
  of the wide-window column), inside the page so it comes and goes with it. Its height
  joins that page's bottom inset, so section anchors and the end of the scroll clear it
  with no other change. The buttons go here.
- **`SheetSlot(id, child)` and `SheetContent.held`.** Every row sits in a slot, and the
  page says which slot is held. A held slot's child is painted over the content at the
  slot's position, kept between the content's top and the footer's top; the slot keeps
  its place and size in the list. The child stays the slot's child, so its state and a
  scrub in progress carry on. Following a drag or a scroll builds, lays out and paints
  nothing.
- **Changing which slot is held** animates (200 ms, ease out). Each hold has a strength:
  the outgoing child's eases to nothing, so a docked row slides back to its place, and
  the incoming child's eases to full, so a row below the buttons rises from behind
  them. When the incoming child would come to rest where the outgoing one is painted,
  they cross-fade in place instead.
- **`SheetSlot.displacementOf(context)`** tells the child how far it's painted from its
  slot, for the docked shadow.
- **Anchor rule.** A section anchor adds the held slot's height when that slot starts at
  or below the line the anchor shows down to, because the child is then docked at that
  rest. Both of this page's anchors qualify. An anchor below the slot doesn't.
- **`SheetAnchor.footer()`** rests with only the handle, the held child and the footer
  showing: the lowest anchor.
- **`initialSection` and `initialMaxFraction`** open the sheet inside a free range,
  showing down to a section but no higher than a fraction of the window.

This page declares: anchors `footer()` and `section(modes)`; one free range from
`section(modes)` to `full()`; the buttons as footer; every row in a slot, the selected
one held.

What the page has to respect:

- A slot must be built to be held, so rows can't sit in a lazily built list.
- A held child whose slot is removed in the same frame just vanishes. Rows that
  cross-fade between modes need slots that stay, or a fade of the page's own.
- `held` going to or from nothing moves the anchors by the row's height, so a
  placeholder row always holds the dock.
- The held child is painted outside any clip or opacity its list ancestors apply.

Not verified: screen-reader semantics for a child held at the top while its slot is
scrolled far out of view.

Naming: `SheetPlacement.docked` already means the wide-window column, so the library
says "footer", "slot" and "held", not "dock".

### When an anchor moves

Today (`SheetController._setAnchors`): a sheet resting at an anchor follows it at once
when it moves, with no animation, and a sheet resting in a free range keeps its fraction
of the range when the range's ends move.

Both anchors here sit on the header's height, the held row's height and the footer's
height. The design keeps all three constant, so the anchors don't move in use:

- Rows are one height in every mode, and so are the placeholder row and the "no route"
  message that take the row's place.
- Steps and Start, and Trip details, make a footer of the same height.
- The trip card and tabs don't change height with mode, loading or errors.

What the page must do: keep those heights constant, and never change one in a single
step. A test resolves the two anchors in every mode, while loading and with no route,
and expects the same extents each time.

What the library must do: nothing new. When an anchor does move (the window is resized
or rotated, or the system text size changes) following at once is right, since the
whole layout changes in that frame. The same goes for keeping the fraction of the free
range.

### In the wide-window column

At 600 px wide and up the sheet is a column on the start edge. It still rests at its
anchors and still lowers, so the rules are the same:

- The footer sits at the bottom of the column, as wide as the column.
- The held row stays between the content's top and the footer's top, as wide as the
  content.
- The anchor rule and both anchors apply unchanged.
- Scrubbing's "visible map" is whatever `SheetView.obscured` leaves, which in the
  column is everything beside it, so the camera rarely needs to pan.

## Data the rows need

Surveyed 2026-11-20. The fact that shapes everything: the app never sees Valhalla's own
JSON. Ferrostar's adapter asks for `"format": "osrm"`, so on the server and on the
device the app parses Valinor's OSRM output (`valinor/valinor-tyr/src/tyr/osrm/`). Both
run the same `valinor_router::Router`.

| Row needs | Today | Where |
|---|---|---|
| Street names and lengths, for naming | Built (step 2): `DirectionsStep.roadName`, and `DirectionsRoute.roadNames` named in Rust across the routes returned together | `crates/routing_flutter/src/route_details.rs` |
| Tolls, ferry, freeway | Built (step 2): `hasToll`, `hasFerry`, `hasMotorway`, from the step intersections' `classes` | `route_details.rs` |
| Road type, bike infrastructure, surface per edge | Not in either route format. Only `trace_attributes` has them (`road_class`, `use`, `cycle_lane`, `bicycle_network`, `surface`) | `tyr/trace_attributes.rs`; on the device, `OfflineRouter::trace_attributes` |
| Elevation along the route | Valhalla JSON has `elevation_interval`. The OSRM output has none. `/height` isn't implemented | `tyr/valhalla.rs` |
| Elevation in the tiles | Only in tiles built with `--gedtm`. Whether the published regions have it isn't known | `data_pipeline/README.md` |
| Transit position at a time | Every stop of a ride has times and coordinates, and the leg has a shape, but no index from stops into the shape. Walks have a shape and a duration; Dart doesn't parse the duration yet | `valinor-transit-plan/src/response/mod.rs`, `transit_plan.dart` |

Where to add it: `DirectionsAdapter::parse` in `crates/routing_flutter` already makes
its own pass over the raw response beside Ferrostar's, for speed-limit signs. Names and
flags can be read there with no change to Valinor.

Decided: road types and elevation come on the route itself. Valinor's response schema
may change as needed, provided Ferrostar still parses it. In practice that means added
fields on the OSRM route that Ferrostar ignores, the way `speedLimitSign` already
works: a per-route list of runs (duration, road type, bike infrastructure) and elevation
samples, read by the app's own pass over the response. One request, and a row is
complete when it arrives. A second `trace_attributes` request per route was the
alternative and isn't needed.

Driving's speed profile is built (step 2): `DirectionsRoute.runs`, stretches with
duration, distance, speed and whether each is motorway, tolled or a ferry, merged to tens
per route (23 to 34 on real routes). That gives two tones, freeway and everything else.
The third tone needs the runs above. The runs' total time differs from the route's
duration by up to 0.2%, so the graph sizes itself from the runs' own total.

What the response gives for names, and its limits:

- A freeway is `I 5 North` with no street name; a state route is `SR 99` with no
  direction. In the United States an Interstate is written `I-5 North`. `WA-99` isn't
  possible: the response carries the country, not the state.
- A road with a name and a number is called by its name, as the people who drive it
  call it. Only a road with no name, usually a freeway, goes by its number. A named
  stretch of a numbered freeway therefore takes its name.
- Valinor marks a whole maneuver as tolled if any part of it is, so a tolled stretch
  would be drawn too long. The "Toll" label on the profile is therefore not drawn until
  the per-stretch runs exist (step 7); until then the caption says "Tolls".

Elevation in the published tiles: the routing release the app's releases URL points at
(2026-11-11) records build commit `65f8974`, the commit of the planet build that was run
with GEDTM elevation and checked against the DEM. So the published tiles should have
elevation. This is inferred from the matching commit; no published tile was inspected.

Rows must still work without elevation: when a tile has none, the band is left empty and
the caption omits climb.

For transit scrubbing, the app snaps each stop onto the leg's shape once, then places
the marker by time between consecutive stops. Walks interpolate along their shape by
time.

## iOS

Native per platform, as elsewhere in the app. iOS has edge-case mockups only (on the
canvas linked above), drawn to settle these points before it's built:

- **Trip card:** one grouped inset card, with X and swap as round buttons beside it.
- **Tabs:** a segmented control whose segments are two lines tall, the icon over the
  time, about 44 pt. Icon and time side by side don't fit four segments.
- **Rows:** separate cards with a gap between them, not one grouped list, so a row
  looks the same in place and docked.
- **Buttons:** capsules in the footer, over the home indicator.
- **Glass:** the sheet is translucent while it's low, which is where the row docks.
  The selected row, the cards and the Steps button are translucent tints there, and
  nothing paints an opaque backing; the library clips the list out from behind them.
- **Dark mode:** the profile's freeway tone is the lighter, more prominent one.

The docking and scrubbing rules are the same as Android's.

Not designed yet: transit and cycling rows on iOS, and right-to-left layout.

## Build order

1. Sheet library: the footer, slots and the held slot, the lowest anchor, opening inside
   a free range. Done.
2. Route data already in the response: road names, the route's name, toll, ferry and
   freeway, speed runs. Done.
3. The picker for street modes: trip card, tabs with parallel planning, rows, selection
   and docking, large text. Done. Transit keeps its old list under the new header.
4. Steps restyled, and Trip details as a pushed page.
5. Transit rows in the new shape: tap selects, a docked row, a Trip details button.
6. Scrubbing and the map marker.
7. Valinor's added route fields: road-type runs and elevation, for the cycling and
   walking bands, driving's third tone and the "Toll" label.

Until step 5, switching between a street mode and transit at the header height moves
the sheet by a row and the buttons, since transit has neither yet.

## Still open

1. **The third driving tone** waits on the route runs from Valinor; until then the
   profile has two.
