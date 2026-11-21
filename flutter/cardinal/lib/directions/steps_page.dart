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

/// The steps of the route selected in [directions], pushed over the directions page:
/// back, how long the route takes and what it's called, then each step with how far
/// it goes.
///
/// It follows the selection, which a tap on another route's callout on the map
/// changes while this shows. Back returns to directions with the sheet where it was
/// left there.
class StepsPage extends StatefulWidget {
  const StepsPage({super.key, required this.directions});

  /// The trip, which the directions page under this one owns.
  final Directions directions;

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

  StreamSubscription<UnitSystem>? _watchUnits;

  /// The app's [unitSystem] setting, as the directions page follows it.
  UnitSystem _units = UnitSystem.locale;

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
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.directions,
    builder: (context, _) {
      final theme = Theme.of(context);
      final cupertino = isCupertinoOf(context);
      final units = _units.onDevice;
      final route = widget.directions.selectedRoute;
      final name = route == null
          ? null
          : routeName(AppLocalizations.of(context), route);
      final steps = route?.steps ?? const <DirectionsStep>[];
      return SheetContent(
        anchors: const [_lower],
        freeRanges: const [_range],
        slivers: [
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(4, 0, 16, 8),
              child: Row(
                children: [
                  IconButton(
                    tooltip: 'Back',
                    icon: const Icon(Icons.arrow_back),
                    onPressed: () => context.pop(),
                  ),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          route == null ? '' : formatDuration(route.duration),
                          style: theme.textTheme.titleLarge,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                        if (name != null)
                          Text(
                            name,
                            style: theme.textTheme.bodyMedium?.copyWith(
                              color: theme.colorScheme.onSurfaceVariant,
                            ),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                          ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
          if (cupertino && steps.isNotEmpty)
            SliverPadding(
              padding: const EdgeInsets.only(bottom: 12),
              sliver: SliverToBoxAdapter(
                child: AppListSection(
                  header: 'Steps',
                  children: [
                    for (final (i, step) in steps.indexed)
                      _CupertinoStep(number: i + 1, step: step, units: units),
                  ],
                ),
              ),
            )
          else if (!cupertino)
            SliverPadding(
              padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
              sliver: SliverList.builder(
                itemCount: steps.length,
                itemBuilder: (context, i) =>
                    _StepCard(number: i + 1, step: steps[i], units: units),
              ),
            ),
        ],
      );
    },
  );
}

/// A numbered step of a route.
class _StepCard extends StatelessWidget {
  const _StepCard({
    required this.number,
    required this.step,
    required this.units,
  });

  final int number;
  final DirectionsStep step;
  final UnitSystem units;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card.filled(
      margin: const EdgeInsets.symmetric(vertical: 4),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        child: Row(
          children: [
            SizedBox(
              width: 28,
              child: Text(
                '$number',
                textAlign: TextAlign.center,
                style: theme.textTheme.labelLarge,
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Text(step.instruction, style: theme.textTheme.bodyMedium),
            ),
            const SizedBox(width: 12),
            Text(
              formatDistance(step.distance, units: units),
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A numbered step of a route, in iOS's grouped list.
class _CupertinoStep extends StatelessWidget {
  const _CupertinoStep({
    required this.number,
    required this.step,
    required this.units,
  });

  final int number;
  final DirectionsStep step;
  final UnitSystem units;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return AppListTile(
      leading: Text(
        '$number',
        style: theme.textTheme.titleSmall?.copyWith(
          color: theme.colorScheme.primary,
        ),
      ),
      title: Text(step.instruction, maxLines: 3),
      trailing: Text(
        formatDistance(step.distance, units: units),
        style: theme.textTheme.bodySmall?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}
