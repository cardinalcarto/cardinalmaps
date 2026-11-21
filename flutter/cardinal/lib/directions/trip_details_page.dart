/// A transit journey in detail, as a page pushed over the directions page, and what the
/// two pages share of it.
library;

import 'dart:async';

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../sheet/sheet.dart';
import '../units.dart';
import 'connections.dart';
import 'departure_legs.dart';
import 'departure_lists.dart';
import 'directions.dart';
import 'route_source.dart';
import 'transit_details.dart';
import 'transit_plan.dart';

/// The transit journey open in Trip details, kept by the directions page under it,
/// which draws it on the map: the departures chosen of the journey's rides while it's
/// open, the legs fetched of the departures chosen other than the plan's, and, while
/// no plan has the journey, each ride's departures looked up between its two stops.
///
/// | In | When | Then |
/// |---|---|---|
/// | closed | a journey is opened | open: each ride's departure the plan's own, the first ride's made as the clock has it |
/// | open | a departure is chosen | open: the rest of the journey re-timed by it |
/// | open | the clock passes the last moment one of the first ride's departures can be made | open: that one missed, and the journey re-timed as for a choice that can't be made; with none left, every departure gone |
/// | open | transit is planned again, and the new plan has a journey whose rides are the departures taken (the same trips, between the same stops, in order), with every ride connected | open, on that journey, which is the one open from then on: the choices kept, as far as they can be, now that journey's own |
/// | open | transit is planned again, and the new plan has the journey (and none as above) | open, on the journey as that plan has it: the choices kept, as far as they can be |
/// | open | transit is planned again, and the new plan hasn't the journey | open, on the journey as it was: each ride's departures as last looked up, if they have been, and otherwise as the new plan lists them between its two stops, or as they were where it lists none; the choices kept, as far as they can be. Each ride's departures are asked for between its two stops ([DepartureLists]) |
/// | open, on a journey the plan hasn't | a ride's departures looked up arrive | open: that ride's departures are those, the choices kept, as far as they can be; a first ride whose departures were all gone has those still to come, and the journey is made again if one can be |
/// | open, on a journey the plan hasn't | a lookup answers with no departures, fails, or can't be asked | open, as it was |
/// | open | transit is asked again for a changed trip | closed |
/// | open | closed by the page that opened it | closed |
///
/// The directions page says which of those a new plan is, in that order: a journey is
/// the same one from plan to plan by its rides' trips and stops ([JourneyIdentity]),
/// and one is of the departures taken as [ConnectionChain.takes] has it. It says too
/// when the clock has passed, and the time, as it does a new plan, and when
/// departures looked up are to be taken ([relist]): once no finger is down.
///
/// Departures looked up are forgotten, and what's being asked given up on, once a
/// plan has the journey again (or one of the departures taken), and when the journey
/// is closed.
///
/// The legs are kept while the plan lasts, so a departure chosen again, in this
/// opening or another, isn't fetched again; under a newer plan each is fetched again,
/// what was known standing in until then. The choices start afresh each opening.
class OpenJourney extends ChangeNotifier {
  OpenJourney({
    required this.directions,
    required this.legs,
    required this.lists,
  });

  /// The trip, which the directions page owns.
  final Directions directions;

  /// The legs of the departures chosen other than the plan's.
  final DepartureLegs legs;

  /// Each ride's departures looked up between its stops, while no plan has the
  /// journey.
  final DepartureLists lists;

  /// How many of [lists]' the open journey has taken ([DepartureLists.revision]).
  int _listsTaken = 0;

  /// The open journey with the departures looked up that it hasn't yet taken.
  ConnectionChain _withLists(ConnectionChain chain) {
    if (lists.revision == _listsTaken) return chain;
    _listsTaken = lists.revision;
    return chain.relist(lists.lists);
  }

  void _forgetLists() {
    lists.forget();
    _listsTaken = lists.revision;
  }

  /// The app's `unitSystem` setting, as the directions page follows it, so this page
  /// shows its distances from its first frame.
  UnitSystem get units => _units;
  UnitSystem _units = UnitSystem.locale;
  set units(UnitSystem units) {
    if (units == _units) return;
    _units = units;
    notifyListeners();
  }

  /// The open journey, with the departures chosen of its rides; null while none is
  /// open.
  ConnectionChain? get chain => _chain;
  ConnectionChain? _chain;

  bool get isOpen => _chain != null;

  /// How many times a journey has been opened, so an opening is closed only by
  /// whatever opened it.
  int _opens = 0;

  /// Opens [journey], each ride on the plan's own departure and the first ride's made
  /// as the clock has it at [now], and says which opening this is, for [close].
  int open(Journey journey, {DateTime? now}) {
    _opens++;
    _forgetLists();
    _chain = ConnectionChain(journey, now: now);
    notifyListeners();
    return _opens;
  }

  /// The opening [open] last said.
  int get opening => _opens;

  /// Takes [departure] of the ride at [ride] of the open journey.
  void select(int ride, int departure) {
    final chain = _chain;
    if (chain == null) return;
    _chain = chain.select(ride, departure);
    notifyListeners();
  }

  /// Moves the open journey's choices onto [journey], a newer plan of it, as far as
  /// they can be, at [now] if it's given.
  void rebase(Journey journey, {DateTime? now}) {
    final chain = _chain;
    if (chain == null || identical(chain.journey, journey)) return;
    // The plan lists its rides' departures itself.
    _forgetLists();
    final rebased = chain.rebase(journey);
    _chain = now == null ? rebased : rebased.at(now);
    notifyListeners();
  }

  /// Keeps the open journey as it is, for a newer plan without it: each ride's
  /// departures as last looked up, if they have been, and otherwise as that plan's
  /// [journeys] list them between its stops, where they do, at [now] if it's given.
  void refresh(List<Journey> journeys, {DateTime? now}) {
    final chain = _chain;
    if (chain == null) return;
    final refreshed = _withLists(chain)
        .refresh(journeys, keep: lists.lists.keys.toSet());
    _chain = now == null ? refreshed : refreshed.at(now);
    notifyListeners();
  }

  /// Asks for each ride's departures between its stops, for the open journey, which
  /// [plan] hasn't, as it's taken getting to stops by [access] at [now].
  void lookUp(TransitPlan plan, TransitAccess access, DateTime now) {
    final chain = _chain;
    if (chain != null) lists.ask(plan, chain, access, now);
  }

  /// Takes the departures looked up since the last were taken, at [now] if it's
  /// given; listeners are told only if there were any.
  void relist({DateTime? now}) {
    final chain = _chain;
    if (chain == null || lists.revision == _listsTaken) return;
    final relisted = _withLists(chain);
    _chain = now == null ? relisted : relisted.at(now);
    notifyListeners();
  }

  /// Makes the first ride's departures as the clock has it at [now]; listeners are told
  /// only if that changes a choice.
  void at(DateTime now) {
    final chain = _chain;
    if (chain == null) return;
    final timed = chain.at(now);
    final changed = !timed.choosesAs(chain);
    _chain = timed;
    if (changed) notifyListeners();
  }

  /// Closes the journey if [opening] is still the one open, or with none, whichever
  /// is.
  void close([int? opening]) {
    if (_disposed || _chain == null || (opening != null && opening != _opens)) {
      return;
    }
    _chain = null;
    _forgetLists();
    notifyListeners();
  }

  /// Whether the directions page has gone, as it may before the page over it.
  bool _disposed = false;

  @override
  void dispose() {
    _disposed = true;
    legs.dispose();
    lists.dispose();
    super.dispose();
  }
}

/// The journey open in [journey], pushed over the directions page: when it leaves and
/// arrives, its timeline, and its legs from top to bottom, with each ride's departures
/// to choose from.
///
/// | In | When | Then |
/// |---|---|---|
/// | opening | | the journey from its top, the sheet at 40% so the map shows it |
/// | showing the journey | a departure is chosen | the rest of the journey re-timed, nothing above the departure moving |
/// | showing the journey | the leg of a departure chosen arrives | its stops listed, its line on the map |
/// | showing the journey | back | the directions page, the sheet and its list where they were left |
/// | showing the journey | transit planned again, the new plan with this journey | the same journey as that plan has it: its times and timeline ease to the new ones, the departures chosen kept, what's open of its stops still open |
/// | showing the journey | transit planned again, the new plan with a journey of the departures taken | that journey, as the same journey planned again is shown: the departures taken kept, "Suggested" cross-fading onto their tiles, no row coming, going or changing height; the list under it selects it, the map staying where it is |
/// | showing the journey | transit planned again without this journey | the same journey as it was: each ride's tiles those last looked up between its two stops, if any were, and otherwise those the new plan lists between them, or as they were; the departures chosen kept while they're listed and can be made; times and timeline ease to what that makes |
/// | showing a journey the plan hasn't | a ride's departures looked up arrive, once no finger is down and a moment after | its tiles are those, as a newer list is shown: the departures chosen kept while they're listed and can be made, times and timeline easing; no row comes or goes, and none changes height but a title that needs more room at the roomier text sizes |
/// | showing a journey the plan hasn't, every departure of the first ride gone | departures still to come are looked up, once no finger is down and a moment after | the journey again: "No departures left" cross-fades to when to go, the notice under the tiles to how the departure taken runs, the rides after get their times and departures back, the timeline as that makes it |
/// | showing the journey | the clock passes the last moment the rider can make one of the first ride's departures, once no finger is down and a moment after | its tile turns Missed; if it was taken, the journey is re-timed onto the departure it can make that arrives first, its times, timeline and the map following |
/// | showing the journey | the first ride's last departure is missed so | every departure gone: "No departures left" over when it last left, every tile of the first ride Missed with a notice under them, the rides after without times or departures, the timeline as the plan had it |
/// | showing the journey | a ride comes to have no connection, or to follow one without (by the clock, or a choice) | its wait row says nothing, with no icon, or only "Change at" where it's a change, cross-fading, its height and the rail through it the same; its boarding and alighting stops' times fade out, their room kept; and back as it gets one |
/// | showing the journey, at the roomier text sizes | a newer list or plan drops a ride's longest headsign, or brings a longer one | its title keeps the room it had; it grows for a longer one as that list is shown, once no finger is down, and never shrinks while the page is open |
///
/// Whether a ride has a wait row is decided as it's first shown, from the plan's wait
/// and how many of the departures it lists run, and holds for the opening, so no row
/// comes or goes above a ride's departures whatever the clock, a choice or a newer
/// list does.
/// | showing the journey | transit asked again for a changed trip | the directions page, as for back |
///
/// The notice under the first ride's tiles says "No more departures today" when the
/// last of them that runs is the day's last between its stops, and "No departures
/// left" otherwise.
///
/// From the 40% anchor up the sheet rests wherever it's let go, and a fling up carries
/// on into scrolling the journey, so reading through it is one long scroll.
class TripDetailsPage extends StatefulWidget {
  const TripDetailsPage({super.key, required this.journey});

  /// The journey, which the directions page under this one keeps.
  final OpenJourney journey;

  @override
  State<TripDetailsPage> createState() => _TripDetailsPageState();
}

class _TripDetailsPageState extends State<TripDetailsPage> {
  /// Low enough for the map to show the journey's shape.
  static const _lower = SheetAnchor.fraction(0.4);

  static const _range = SheetRange(
    _lower,
    SheetAnchor.full(),
    carriesIntoScroll: true,
  );

  /// How often "Go in" and the like are shown again, as the directions page does.
  static const _tick = Duration(seconds: 30);

  /// The opening this page shows.
  late final int _opening = widget.journey.opening;

  /// What's shown: the open journey, or, once it's closed and this page is on its way
  /// out, the journey as it last was.
  late ConnectionChain _shown = widget.journey.chain!;

  /// Whether this page has been popped, or is popping itself.
  bool _leaving = false;

  Timer? _clock;

  late final Listenable _watched = Listenable.merge([
    widget.journey,
    widget.journey.legs,
    widget.journey.directions,
  ]);

  @override
  void initState() {
    super.initState();
    _watched.addListener(_changed);
    _clock = Timer.periodic(_tick, (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _clock?.cancel();
    _watched.removeListener(_changed);
    widget.journey.close(_opening);
    super.dispose();
  }

  void _changed() {
    final journey = widget.journey;
    final chain = journey.chain;
    if (chain != null && journey.opening == _opening) {
      setState(() => _shown = chain);
      return;
    }
    // Another trip closed it: back to the journeys.
    if (_leaving) return;
    _leaving = true;
    context.pop();
  }

  @override
  Widget build(BuildContext context) {
    final open = widget.journey;
    final directions = open.directions;
    final units = open.units.onDevice;
    final now = DirectionsScope.of(context).clock();
    final chain = _shown;
    return PopScope(
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) return;
        _leaving = true;
        open.close(_opening);
      },
      child: SheetContent(
        anchors: const [_lower],
        freeRanges: const [_range],
        slivers: [
          SliverToBoxAdapter(
            child: JourneyHeader(
              chain: chain,
              now: now,
              onBack: () => context.pop(),
            ),
          ),
          JourneyItinerary(
            chain: chain,
            fetched: [
              for (final choice in chain.rides)
                open.legs.legOf(choice, directions.transitAccess),
            ],
            onSelect: open.select,
            start: directions.from?.label ?? 'Start',
            destination: directions.to?.label ?? 'Destination',
            units: units,
            access: directions.transitAccess,
            now: now,
          ),
        ],
      ),
    );
  }
}
