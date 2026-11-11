import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoButton;
import 'package:material_ui/material_ui.dart';

import '../ui/adaptive.dart';

/// One of the map's buttons. On iOS just the icon, for [MapButtons] to group on glass;
/// elsewhere a floating action button of its own.
class MapControl extends StatelessWidget {
  const MapControl({
    super.key,
    required this.tooltip,
    required this.icon,
    required this.onPressed,
  });

  /// The control's size on iOS: Apple's minimum touch target.
  static const double size = 48;

  final String tooltip;
  final IconData icon;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    if (!isCupertinoOf(context)) {
      return FloatingActionButton(
        heroTag: null,
        tooltip: tooltip,
        onPressed: onPressed,
        child: Icon(icon),
      );
    }
    return Tooltip(
      message: tooltip,
      child: Semantics(
        button: true,
        label: tooltip,
        excludeSemantics: true,
        child: CupertinoButton(
          padding: EdgeInsets.zero,
          minimumSize: const Size.square(size),
          onPressed: onPressed,
          child: Icon(
            icon,
            size: 22,
            color: Theme.of(context).colorScheme.onSurface,
          ),
        ),
      ),
    );
  }
}
