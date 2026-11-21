import 'dart:async';

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
import '../units.dart';
import 'directions.dart';
import 'format.dart';
import 'option_row.dart';
import 'route_source.dart';
import 'trip_buttons.dart';

/// The steps of the route selected in [directions], pushed over the directions page:
/// back; how long the route takes and what it's called, and under that how far it goes
/// and when it arrives; then each step with how far it goes, and Start along the
/// bottom.
///
/// It follows the selection, which a tap on another route's callout on the map
/// changes while this shows. Start does as the directions page's does. Back returns to
/// directions with the sheet where it was left there.
///
/// Nothing is cut short but the route's name, after two lines: a step's instruction
/// takes as many lines as it needs.
class StepsPage extends StatefulWidget {
  const StepsPage({super.key, required this.directions, required this.to});

  /// The trip, which the directions page under this one owns.
  final Directions directions;

  /// The ID of the place the trip goes to.
  final String to;

  @override
  State<StepsPage> createState() => _StepsPageState();
}

class _StepsPageState extends State<StepsPage> {
  /// Low enough for the map to show the route.
  static const _lower = SheetAnchor.fraction(0.4);

  /// From there up the sheet rests wherever it's let go, and a fling up carries on
  /// into scrolling the steps.
  static const _range = SheetRange(
    _lower,
    SheetAnchor.full(),
    carriesIntoScroll: true,
  );

  /// How often the arrival is worked out again, as the directions page does.
  static const _tick = Duration(seconds: 30);

  StreamSubscription<UnitSystem>? _watchUnits;
  Timer? _clock;

  /// The app's [unitSystem] setting, as the directions page follows it.
  UnitSystem _units = UnitSystem.locale;

  @override
  void initState() {
    super.initState();
    _clock = Timer.periodic(_tick, (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _watchUnits ??= StoreScope.of(context).settings
        .watch(unitSystem)
        .listen((units) {
          if (units != _units) setState(() => _units = units);
        });
  }

  @override
  void dispose() {
    _watchUnits?.cancel();
    _clock?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.directions,
    builder: (context, _) {
      final cupertino = isCupertinoOf(context);
      final units = _units.onDevice;
      final route = widget.directions.selectedRoute;
      final steps = route?.steps ?? const <DirectionsStep>[];
      final roomy = isRoomyText(MediaQuery.textScalerOf(context));
      Widget step(int i) =>
          _StepRow(number: i + 1, step: steps[i], units: units, roomy: roomy);
      return SheetContent(
        anchors: const [_lower],
        freeRanges: const [_range],
        footer: TripButtons.start(
          onStart: route == null
              ? null
              : () =>
                    startNavigation(context, widget.directions, to: widget.to),
        ),
        slivers: [
          SliverToBoxAdapter(
            child: _StepsHeader(
              route: route,
              units: units,
              now: DirectionsScope.of(context).clock(),
            ),
          ),
          if (cupertino && steps.isNotEmpty)
            SliverPadding(
              padding: const EdgeInsets.only(bottom: 12),
              sliver: SliverToBoxAdapter(
                child: AppListSection(
                  header: 'Steps',
                  children: [for (var i = 0; i < steps.length; i++) step(i)],
                ),
              ),
            )
          else if (!cupertino)
            SliverList.builder(
              itemCount: steps.length,
              itemBuilder: (context, i) => Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  step(i),
                  if (i < steps.length - 1) const Divider(height: 1),
                ],
              ),
            ),
        ],
      );
    },
  );
}

/// Back, and what the route is: how long it takes and what it's called on one line,
/// the time first, then how far it goes and when it arrives.
class _StepsHeader extends StatelessWidget {
  const _StepsHeader({
    required this.route,
    required this.units,
    required this.now,
  });

  /// The route, or null while there's none selected.
  final DirectionsRoute? route;
  final UnitSystem units;
  final DateTime now;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final cupertino = isCupertinoOf(context);
    final route = this.route;
    final name = route == null
        ? null
        : routeName(AppLocalizations.of(context), route);
    final title = theme.textTheme.titleLarge!;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(4, 0, 12, 10),
          child: Row(
            children: [
              IconButton(
                tooltip: 'Back',
                icon: const BackButtonIcon(),
                onPressed: () => context.pop(),
              ),
              const SizedBox(width: 4),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  spacing: 2,
                  children: [
                    // The name gives way after two lines; the time never does, as
                    // it comes first.
                    Text.rich(
                      TextSpan(
                        children: [
                          TextSpan(
                            text: route == null
                                ? ''
                                : formatDuration(route.duration),
                            style: title.copyWith(fontWeight: FontWeight.w700),
                          ),
                          if (name != null)
                            TextSpan(
                              text: ' $name',
                              style: title.copyWith(
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                        ],
                      ),
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                    ),
                    if (route != null)
                      Text(
                        '${formatDistance(route.distance, units: units)} · '
                        '${routeArrival(context, route, now)}',
                        style: theme.textTheme.bodyMedium?.copyWith(
                          color: theme.colorScheme.onSurfaceVariant,
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
        ),
        if (!cupertino) const Divider(height: 1),
      ],
    );
  }
}

/// A numbered step of a route, with how far it goes: beside the instruction, or under
/// it at the roomier text sizes (see [roomyTextScale]). Its instruction takes as many
/// lines as it needs.
class _StepRow extends StatelessWidget {
  const _StepRow({
    required this.number,
    required this.step,
    required this.units,
    required this.roomy,
  });

  final int number;
  final DirectionsStep step;
  final UnitSystem units;
  final bool roomy;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final cupertino = isCupertinoOf(context);
    final muted = theme.colorScheme.onSurfaceVariant;
    final instruction = Text(
      step.instruction,
      style: theme.textTheme.bodyLarge,
    );
    // The last step, arriving, goes nowhere.
    final distance = step.distance > 0
        ? Text(
            formatDistance(step.distance, units: units),
            style: theme.textTheme.bodySmall?.copyWith(color: muted),
          )
        : null;
    return ConstrainedBox(
      constraints: BoxConstraints(minHeight: cupertino ? 44 : 60),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
        child: Row(
          children: [
            ConstrainedBox(
              constraints: const BoxConstraints(minWidth: 24),
              child: Text(
                '$number',
                textAlign: TextAlign.center,
                style: theme.textTheme.labelLarge?.copyWith(
                  color: cupertino ? theme.colorScheme.primary : muted,
                ),
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: roomy && distance != null
                  ? Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      spacing: 2,
                      children: [instruction, distance],
                    )
                  : instruction,
            ),
            if (!roomy && distance != null) ...[
              const SizedBox(width: 12),
              distance,
            ],
          ],
        ),
      ),
    );
  }
}
