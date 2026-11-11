import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoButton;
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../routes.dart';
import '../ui/adaptive.dart';
import '../ui/glass.dart';
import 'app_icons.dart';

/// The bar along the bottom of the app's top-level pages: Saved Places, Nearby Places,
/// Search, Nearby Transit and Offline Maps.
///
/// On iOS it floats on glass, labeled, with the current page tinted on a lens. Elsewhere
/// it's a row of icons with Search filled in the middle.
class AppNavigationBar extends StatelessWidget {
  const AppNavigationBar({super.key});

  /// Goes to the top-level page at [location]. Search, tapped on its own page, starts
  /// searching.
  static void _go(BuildContext context, String location) {
    final router = GoRouter.of(context);
    if (location == '/' && router.state.uri.path == '/') {
      searchRequests.value++;
    } else {
      router.go(location);
    }
  }

  @override
  Widget build(BuildContext context) =>
      isCupertinoOf(context) ? const _GlassTabBar() : const _MaterialBar();
}

/// A top-level page of the bar.
typedef _Tab = ({IconData icon, String label, String location});

const List<_Tab> _tabs = [
  (icon: AppIcons.saved, label: 'Saved', location: '/saved'),
  (icon: AppIcons.nearby, label: 'Nearby', location: '/nearby'),
  (icon: AppIcons.search, label: 'Search', location: '/'),
  (icon: AppIcons.transit, label: 'Transit', location: '/transit'),
  (icon: AppIcons.offline, label: 'Offline', location: '/offline'),
];

/// Which of [_tabs] [path] is on: Search, the home page, for any path of its own.
int _currentTab(String path) {
  final i = _tabs.indexWhere(
    (t) => t.location != '/' && path.startsWith(t.location),
  );
  return i == -1 ? _tabs.indexWhere((t) => t.location == '/') : i;
}

class _MaterialBar extends StatelessWidget {
  const _MaterialBar();

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    Widget button(IconData icon, String label, String location) => _Slot(
      child: IconButton(
        tooltip: label,
        color: colors.onSurface,
        icon: Icon(icon),
        onPressed: () => AppNavigationBar._go(context, location),
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
                  onPressed: () => AppNavigationBar._go(context, '/'),
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

/// The iOS bar: a glass capsule of the pages, floating over the bottom of the sheet,
/// with the current one on a lens.
class _GlassTabBar extends StatelessWidget {
  const _GlassTabBar();

  static const double _height = 60;

  @override
  Widget build(BuildContext context) {
    final router = GoRouter.of(context);
    return ListenableBuilder(
      listenable: router.routerDelegate,
      builder: (context, _) {
        final current = _currentTab(
          router.routerDelegate.currentConfiguration.uri.path,
        );
        return Padding(
          // Low, into the home indicator's margin, as iOS floats its tab bars.
          padding: const EdgeInsets.fromLTRB(16, 10, 16, 0),
          child: Align(
            alignment: Alignment.topCenter,
            child: SizedBox(
              height: _height,
              child: AppGlassLayer(
                child: GlassSurface(
                  radius: _height / 2,
                  floating: true,
                  child: Padding(
                    padding: const EdgeInsets.all(4),
                    child: Row(
                      children: [
                        for (final (i, tab) in _tabs.indexed)
                          Expanded(
                            child: _TabItem(tab: tab, selected: i == current),
                          ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

class _TabItem extends StatelessWidget {
  const _TabItem({required this.tab, required this.selected});

  final _Tab tab;
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = selected
        ? theme.colorScheme.primary
        : theme.colorScheme.onSurface;
    return Semantics(
      button: true,
      selected: selected,
      label: tab.label,
      excludeSemantics: true,
      child: CupertinoButton(
        padding: EdgeInsets.zero,
        minimumSize: Size.zero,
        pressedOpacity: 0.6,
        onPressed: () => AppNavigationBar._go(context, tab.location),
        child: DecoratedBox(
          // The lens iOS puts under the current page.
          decoration: ShapeDecoration(
            shape: const StadiumBorder(),
            color: selected
                ? theme.colorScheme.primary.withValues(alpha: 0.14)
                : null,
          ),
          child: SizedBox.expand(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(tab.icon, size: 22, color: color),
                const SizedBox(height: 2),
                Text(
                  tab.label,
                  maxLines: 1,
                  style: theme.textTheme.labelSmall?.copyWith(
                    fontSize: 10,
                    color: color,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
