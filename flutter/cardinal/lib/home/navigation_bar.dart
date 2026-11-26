import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoButton;
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../routes.dart';
import '../sheet/sheet.dart';
import '../ui/adaptive.dart';
import '../ui/glass.dart';
import 'app_icons.dart';

/// The bar along the bottom of the app's top-level pages: Saved Places,
/// Search, Nearby Transit and Offline Maps.
///
/// On iOS it floats on glass, with the current page tinted on a lens. Elsewhere
/// it uses Material 3's labeled destinations and pill-shaped active indicator. While the
/// sheet is collapsed no page is current, and tapping the page under it brings the
/// sheet back.
class AppNavigationBar extends StatelessWidget {
  const AppNavigationBar({super.key});

  /// Height above the system navigation area.
  static double heightOf(BuildContext context) =>
      isCupertinoOf(context) ? 56 : materialHeight;

  static const double materialHeight = 80;

  /// Goes to [tab]'s top-level page. Tapped on its own page, it brings back the sheet
  /// if it's collapsed, and otherwise, for Search, starts searching.
  static void _go(BuildContext context, _Tab tab) {
    final router = GoRouter.of(context);
    final sheet = SheetScaffold.controllerOf(context);
    final path = router.state.uri.path;
    if (sheet.isCollapsed && _tabs[_currentTab(path)] == tab) {
      sheet.expand();
    } else if (tab.location == '/' && path == '/') {
      searchRequests.value++;
    } else {
      router.go(tab.location);
    }
  }

  @override
  Widget build(BuildContext context) =>
      isCupertinoOf(context) ? const _GlassTabBar() : const _MaterialBar();
}

/// A top-level page of the bar: its visible [label] and Material [tooltip].
typedef _Tab = ({IconData icon, String label, String tooltip, String location});

const List<_Tab> _tabs = [
  (
    icon: AppIcons.saved,
    label: 'Saved',
    tooltip: 'Saved places',
    location: '/saved',
  ),
  (icon: AppIcons.search, label: 'Search', tooltip: 'Search', location: '/'),
  (
    icon: AppIcons.transit,
    label: 'Transit',
    tooltip: 'Nearby transit',
    location: '/transit',
  ),
  (
    icon: AppIcons.offline,
    label: 'Offline',
    tooltip: 'Offline maps',
    location: '/offline',
  ),
];

/// Which of [_tabs] [path] is on: Search, the home page, for any path of its own.
int _currentTab(String path) {
  final i = _tabs.indexWhere(
    (t) => t.location != '/' && path.startsWith(t.location),
  );
  return i == -1 ? _tabs.indexWhere((t) => t.location == '/') : i;
}

/// Builds [builder] with which of [_tabs] is selected: the one the router's on, or none
/// while the sheet is collapsed.
class _SelectedTab extends StatelessWidget {
  const _SelectedTab({required this.builder});

  final Widget Function(BuildContext context, int? selected) builder;

  @override
  Widget build(BuildContext context) {
    final router = GoRouter.of(context);
    final collapsed = SheetScaffold.controllerOf(context).collapsed;
    return ListenableBuilder(
      listenable: Listenable.merge([router.routerDelegate, collapsed]),
      builder: (context, _) => builder(
        context,
        collapsed.value
            ? null
            : _currentTab(router.routerDelegate.currentConfiguration.uri.path),
      ),
    );
  }
}

class _MaterialBar extends StatelessWidget {
  const _MaterialBar();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    // NavigationBar requires a selected index, but a collapsed sheet has none.
    return Material(
      color: colors.surfaceContainer,
      child: SafeArea(
        top: false,
        child: _SelectedTab(
          builder: (context, selected) => Row(
            children: [
              for (final (i, tab) in _tabs.indexed)
                Expanded(
                  child: Semantics(
                    selected: i == selected,
                    button: true,
                    child: Tooltip(
                      message: tab.tooltip,
                      child: InkWell(
                        onTap: () => AppNavigationBar._go(context, tab),
                        child: Center(
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              AnimatedContainer(
                                duration: const Duration(milliseconds: 200),
                                width: 64,
                                height: 32,
                                decoration: ShapeDecoration(
                                  shape: const StadiumBorder(),
                                  color: i == selected
                                      ? colors.secondaryContainer
                                      : Colors.transparent,
                                ),
                                child: Icon(
                                  tab.icon,
                                  size: 24,
                                  color: i == selected
                                      ? colors.onSecondaryContainer
                                      : colors.onSurfaceVariant,
                                ),
                              ),
                              const SizedBox(height: 4),
                              MediaQuery.withClampedTextScaling(
                                maxScaleFactor: 1.3,
                                child: Text(
                                  tab.label,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: theme.textTheme.labelMedium?.copyWith(
                                    color: i == selected
                                        ? colors.onSurface
                                        : colors.onSurfaceVariant,
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The iOS bar: a glass capsule of the pages, floating over the bottom of the sheet,
/// with the current one on a lens.
class _GlassTabBar extends StatelessWidget {
  const _GlassTabBar();

  static const double _height = 60;

  @override
  Widget build(BuildContext context) {
    return _SelectedTab(
      builder: (context, current) {
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

  /// The most the system's text scale enlarges a label, as the platforms' own bars
  /// stop their labels growing, and as the directions tabs do: the bar is a fixed
  /// height, and the label is under its icon.
  static const double _maxTextScale = 1.3;

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
        onPressed: () => AppNavigationBar._go(context, tab),
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
                MediaQuery.withClampedTextScaling(
                  maxScaleFactor: _maxTextScale,
                  child: Text(
                    tab.label,
                    maxLines: 1,
                    style: theme.textTheme.labelSmall?.copyWith(
                      fontSize: 10,
                      color: color,
                    ),
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
