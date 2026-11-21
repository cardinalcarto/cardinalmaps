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
import 'directions.dart';
import 'route_source.dart';
import 'transit_details.dart';
import 'transit_plan.dart';

/// The transit journey open in Trip details, kept by the directions page under it,
/// which draws it on the map: the departures chosen of the journey's rides while it's
/// open, and the legs fetched of the departures chosen other than the plan's.
///
/// | In | When | Then |
/// |---|---|---|
/// | closed | a journey is opened | open: each ride's departure the plan's own |
/// | open | a departure is chosen | open: the rest of the journey re-timed by it |
/// | open | transit is planned again, and the new plan has the journey | open, on the journey as that plan has it: the choices kept, as far as they can be |
/// | open | transit is planned again, and the new plan hasn't the journey | closed |
/// | open | transit is asked again for a changed trip | closed |
/// | open | closed by the page that opened it | closed |
///
/// The directions page says which of those a new plan is: a journey is the same one
/// from plan to plan by its rides' trips and stops ([JourneyIdentity]).
///
/// The legs are kept while the plan lasts, so a departure chosen again, in this
/// opening or another, isn't fetched again; under a newer plan each is fetched again,
/// what was known standing in until then. The choices start afresh each opening.
class OpenJourney extends ChangeNotifier {
  OpenJourney({required this.directions, required this.legs});

  /// The trip, which the directions page owns.
  final Directions directions;

  /// The legs of the departures chosen other than the plan's.
  final DepartureLegs legs;

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

  /// Opens [journey], each ride on the plan's own departure, and says which opening
  /// this is, for [close].
  int open(Journey journey) {
    _opens++;
    _chain = ConnectionChain(journey);
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
  /// they can be.
  void rebase(Journey journey) {
    final chain = _chain;
    if (chain == null || identical(chain.journey, journey)) return;
    _chain = chain.rebase(journey);
    notifyListeners();
  }

  /// Closes the journey if [opening] is still the one open, or with none, whichever
  /// is.
  void close([int? opening]) {
    if (_disposed || _chain == null || (opening != null && opening != _opens)) {
      return;
    }
    _chain = null;
    notifyListeners();
  }

  /// Whether the directions page has gone, as it may before the page over it.
  bool _disposed = false;

  @override
  void dispose() {
    _disposed = true;
    legs.dispose();
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
/// | showing the journey | transit planned again without this journey, or asked again for a changed trip | the directions page, as for back |
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
    // A plan without it, or another trip, closed it: back to the journeys.
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
              units: units,
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
