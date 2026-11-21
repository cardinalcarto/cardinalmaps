/// The two ends of a trip at the top of the directions sheet, with the buttons that
/// leave directions and swap the ends.
library;

import 'dart:math' as math;

import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoButton,
        CupertinoIcons,
        CupertinoListSection,
        CupertinoListTile;
import 'package:material_ui/material_ui.dart';

import '../ui/adaptive.dart';
import 'directions.dart';
import 'option_row.dart' show textLinesHeight;

/// From and To as two rows of one card, each tapped to choose that end, and beside the
/// card, outside it, the button that closes directions over the one that swaps the
/// ends.
///
/// A filled card on Android, a grouped inset one on iOS, tinted rather than opaque. A
/// row is at least 44 tall
/// and grows with the system's text; a place's name stays on one line.
class TripCard extends StatelessWidget {
  const TripCard({
    super.key,
    required this.from,
    required this.to,
    required this.onEdit,
    required this.onClose,
    required this.onSwap,
  });

  final RouteEndpoint? from;
  final RouteEndpoint? to;
  final void Function(RouteField field) onEdit;
  final VoidCallback onClose;

  /// Null while there aren't two ends to swap.
  final VoidCallback? onSwap;

  static const _swap = 'Swap start and destination';

  /// The least a row is tall, and the side of each button beside the card.
  static const _minRow = 44.0;

  /// Above and below a row's text on Android, and the line between the rows.
  static const _rowPadding = 8.0, _divider = 1.0;

  /// Above the buttons beside iOS's card, which has room above it.
  static const _buttonsTop = 8.0;

  /// Above and below iOS's grouped rows: the margins
  /// [CupertinoListSection.insetGrouped] puts round them.
  static const _sectionTop = 20.0, _sectionBottom = 10.0;

  /// How tall the card is at [context]'s text size, whatever its ends say: each row's
  /// one line, or the buttons beside it if they're taller. On iOS a row is a
  /// [CupertinoListTile], which is at least 44 tall too, with nothing above or below its
  /// line, and the line between the rows is a hairline.
  static double heightOf(BuildContext context) {
    final cupertino = isCupertinoOf(context);
    final text = Theme.of(context).textTheme;
    final line = math.max(
      textLinesHeight(context, text.bodySmall!),
      textLinesHeight(context, text.bodyLarge!),
    );
    const buttons = 2 * _minRow;
    if (cupertino) {
      final row = math.max(_minRow, line);
      final divider = 1 / MediaQuery.devicePixelRatioOf(context);
      return math.max(
        _sectionTop + 2 * row + divider + _sectionBottom,
        _buttonsTop + buttons,
      );
    }
    final row = math.max(_minRow, 2 * _rowPadding + line);
    return math.max(2 * row + _divider, buttons);
  }

  @override
  Widget build(BuildContext context) {
    final cupertino = isCupertinoOf(context);
    final theme = Theme.of(context);
    Widget end(RouteField field) {
      final label = field == RouteField.from ? 'From' : 'To';
      final endpoint = field == RouteField.from ? from : to;
      final content = Row(
        children: [
          ConstrainedBox(
            constraints: BoxConstraints(minWidth: cupertino ? 36 : 34),
            child: Text(
              label,
              maxLines: 1,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              endpoint?.label ?? '',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.bodyLarge,
            ),
          ),
        ],
      );
      if (cupertino) {
        return AppListTile(title: content, onTap: () => onEdit(field));
      }
      return InkWell(
        onTap: () => onEdit(field),
        child: ConstrainedBox(
          constraints: const BoxConstraints(minHeight: _minRow),
          child: Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: 12,
              vertical: _rowPadding,
            ),
            child: content,
          ),
        ),
      );
    }

    final ends = [end(RouteField.from), end(RouteField.to)];
    return Padding(
      padding: EdgeInsets.only(
        left: cupertino ? 0 : 12,
        right: cupertino ? 6 : 4,
      ),
      child: IntrinsicHeight(
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: cupertino
                  // A tint, as the sheet is glass while it's low.
                  ? AppListSection(
                      color: groupedRowTint(context),
                      children: ends,
                    )
                  : Card.filled(
                      margin: EdgeInsets.zero,
                      clipBehavior: Clip.antiAlias,
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(14),
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          ends.first,
                          const Divider(height: _divider, indent: 56),
                          ends.last,
                        ],
                      ),
                    ),
            ),
            Padding(
              // Beside the card itself: iOS's has room above it.
              padding: EdgeInsets.only(top: cupertino ? _buttonsTop : 0),
              child: Column(
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: [
                  SizedBox.square(
                    dimension: _minRow,
                    child: AppCloseButton(onPressed: onClose),
                  ),
                  SizedBox.square(
                    dimension: _minRow,
                    child: cupertino
                        ? _RoundButton(
                            label: _swap,
                            icon: CupertinoIcons.arrow_up_arrow_down,
                            onPressed: onSwap,
                          )
                        : IconButton(
                            tooltip: _swap,
                            icon: const Icon(Icons.swap_vert),
                            onPressed: onSwap,
                          ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// iOS's round grey button with a glyph, as its close button is.
class _RoundButton extends StatelessWidget {
  const _RoundButton({
    required this.label,
    required this.icon,
    required this.onPressed,
  });

  final String label;
  final IconData icon;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Semantics(
      button: true,
      enabled: onPressed != null,
      label: label,
      excludeSemantics: true,
      child: CupertinoButton(
        padding: EdgeInsets.zero,
        minimumSize: const Size.square(44),
        onPressed: onPressed,
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: colors.onSurface.withValues(alpha: 0.08),
            shape: BoxShape.circle,
          ),
          child: SizedBox.square(
            dimension: 30,
            child: Icon(
              icon,
              size: 14,
              weight: 700,
              color: colors.onSurfaceVariant,
            ),
          ),
        ),
      ),
    );
  }
}
