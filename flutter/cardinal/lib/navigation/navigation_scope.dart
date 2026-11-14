import 'package:material_ui/material_ui.dart';

import 'navigation_controller.dart';

/// Whether the navigation screen is showing.
///
/// The screen is a page of the sheet, which hides the sheet, and what it shows over the
/// map is in the sheet scaffold's floating layer (`NavigationLayer`), which the page
/// can't reach: the page says it's showing here, and the layer listens. Directions,
/// under it, listen too, to leave the map to it.
class NavigationScreen extends ChangeNotifier {
  int _showing = 0;

  /// Whether a navigation page is showing.
  bool get shown => _showing > 0;

  /// Shows the navigation screen until the function returned is called; calling it
  /// again does nothing.
  VoidCallback show() {
    if (_showing++ == 0) notifyListeners();
    var held = true;
    return () {
      if (!held) return;
      held = false;
      if (--_showing == 0) notifyListeners();
    };
  }
}

/// Provides the app's one [NavigationController], and whether its screen shows, to the
/// widgets below.
class NavigationScope extends InheritedWidget {
  const NavigationScope({
    super.key,
    required this.controller,
    required this.screen,
    required super.child,
  });

  final NavigationController controller;
  final NavigationScreen screen;

  static NavigationScope of(BuildContext context) => maybeOf(context)!;

  /// The scope, or null outside one, as in tests of widgets that don't need it.
  static NavigationScope? maybeOf(BuildContext context) =>
      context.getInheritedWidgetOfExactType<NavigationScope>();

  @override
  bool updateShouldNotify(NavigationScope oldWidget) =>
      controller != oldWidget.controller || screen != oldWidget.screen;
}
