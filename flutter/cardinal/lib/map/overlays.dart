/// Whether the map's optional overlays may show: what you've chosen to add to the map, such
/// as your saved places, as opposed to the map itself and what a screen draws on it.
///
/// Screens that need a clear map, like directions, are wrapped in [HidesMapOverlays] where
/// their routes are declared. An overlay listens to [MapOverlays] and draws nothing while
/// [MapOverlays.shown] is false. Neither has to know about the other, and what you've
/// chosen stays as it was, to come back when the screen goes.
library;

import 'package:material_ui/material_ui.dart';

import 'map.dart';

/// See the library's comment.
class MapOverlays extends ChangeNotifier {
  int _hiding = 0;

  /// Whether overlays may show: nothing is hiding them.
  bool get shown => _hiding == 0;

  /// Hides overlays until the function returned is called. Screens can overlap as one
  /// opens over another, so overlays come back only when the last has let go. Calling
  /// the function again does nothing.
  VoidCallback hide() {
    if (_hiding++ == 0) notifyListeners();
    var held = true;
    return () {
      if (!held) return;
      held = false;
      if (--_hiding == 0) notifyListeners();
    };
  }
}

/// Hides the map's overlays for as long as [child] is on the screen.
class HidesMapOverlays extends StatefulWidget {
  const HidesMapOverlays({super.key, required this.child});

  final Widget child;

  @override
  State<HidesMapOverlays> createState() => _HidesMapOverlaysState();
}

class _HidesMapOverlaysState extends State<HidesMapOverlays> {
  VoidCallback? _release;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _release ??= MapScope.overlaysOf(context).hide();
  }

  @override
  void dispose() {
    _release?.call();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
