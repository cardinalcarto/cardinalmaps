import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import 'map/map.dart';
import 'place/place_page.dart';
import 'search/search_sheet.dart';
import 'sheet/sheet.dart';

/// The location of a place's screen, `/place/<id>`, for a [Place](store/store.dart) ID.
///
/// IDs such as `osm:way/123` contain a slash, so the ID is escaped; the route's `id`
/// parameter is the ID again.
String placeLocation(String id) => '/place/${Uri.encodeComponent(id)}';

/// Shows the place with [id], tapped on the map. It takes the place of a place card
/// already open, so that going back leaves the map's places all at once.
void openPlaceFromMap(GoRouter router, String id) {
  final location = placeLocation(id);
  // Of the page on top: a pushed page isn't in the router's configuration's URI.
  final current = router.state.uri.toString();
  if (current == location) return;
  if (current.startsWith('/place/')) {
    router.pushReplacement(location);
  } else {
    router.push(location);
  }
}

/// The app's routes: sheet pages over the map.
///
/// The app restores where it was (see `MainApp`): routes carry only IDs, and what they
/// show is in the store, so restoring the location restores the screens.
GoRouter appRouter(SparkleMapController map) => GoRouter(
  restorationScopeId: 'router',
  routes: [
    ShellRoute(
      restorationScopeId: 'sheet',
      builder: (context, state, child) => Scaffold(
        resizeToAvoidBottomInset: false,
        body: SheetScaffold(
          body: CardinalMap(controller: map),
          sheet: child,
        ),
      ),
      routes: [
        GoRoute(
          path: '/',
          pageBuilder: (context, state) => _page(state, const SearchSheet()),
          routes: [
            GoRoute(
              path: 'place/:id',
              pageBuilder: (context, state) =>
                  _page(state, PlacePage(id: state.pathParameters['id']!)),
            ),
          ],
        ),
      ],
    ),
  ],
);

/// A sheet page for [state]'s route, restorable.
SheetPage<void> _page(GoRouterState state, Widget child) =>
    SheetPage(key: state.pageKey, restorationId: state.pageKey.value, child: child);
