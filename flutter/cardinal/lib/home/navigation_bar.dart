import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../routes.dart';
import 'app_icons.dart';

/// The bar along the bottom of the app's top-level pages: Saved Places, Nearby Places,
/// Search, Nearby Transit and Offline Maps, with Search filled and in the middle.
class AppNavigationBar extends StatelessWidget {
  const AppNavigationBar({super.key});

  /// Goes to the top-level page at [location]. Search, tapped on its own page, starts
  /// searching.
  void _go(BuildContext context, String location) {
    final router = GoRouter.of(context);
    if (location == '/' && router.state.uri.path == '/') {
      searchRequests.value++;
    } else {
      router.go(location);
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    Widget button(IconData icon, String label, String location) => _Slot(
      child: IconButton(
        tooltip: label,
        color: colors.onSurface,
        icon: Icon(icon),
        onPressed: () => _go(context, location),
      ),
    );

    return Material(
      color: colors.surfaceContainer,
      child: SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              button(AppIcons.saved, 'Saved places', '/saved'),
              button(AppIcons.nearby, 'Nearby places', '/nearby'),
              _Slot(
                child: IconButton.filled(
                  tooltip: 'Search',
                  icon: const Icon(AppIcons.search),
                  onPressed: () => _go(context, '/'),
                ),
              ),
              button(AppIcons.transit, 'Nearby transit', '/transit'),
              button(AppIcons.offline, 'Offline maps', '/offline'),
            ],
          ),
        ),
      ),
    );
  }
}

/// A button's place in the bar, as wide as its touch target.
class _Slot extends StatelessWidget {
  const _Slot({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) =>
      SizedBox(width: 48, child: Center(child: child));
}
