import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import 'directions/directions_page.dart';
import 'home/navigation_bar.dart';
import 'home/stub_page.dart';
import 'map/buttons.dart';
import 'map/map.dart';
import 'map/map_drawing.dart';
import 'map/overlays.dart';
import 'offline/download_page.dart';
import 'offline/offline_page.dart';
import 'place/place_page.dart';
import 'saved/saved_page.dart';
import 'search/search_sheet.dart';
import 'sheet/sheet.dart';
import 'ui/glass.dart';

/// Tells the sheet's pages when a page covers them and when it closes again, so the
/// page under it can be as it was: see `SearchSheet`.
final sheetPages = RouteObserver<PageRoute<Object?>>();

/// Counts taps on the app bar's Search button on the search page, which is how
/// `SearchSheet` hears of them.
final searchRequests = ValueNotifier<int>(0);

/// The location of a place's screen, `/place/<id>`, for a [Place](store/store.dart) ID.
///
/// IDs such as `osm:way/123` contain a slash, so the ID is escaped; the route's `id`
/// parameter is the ID again.
String placeLocation(String id) => '/place/${Uri.encodeComponent(id)}';

/// The location of the screen with directions to the place with [id].
String directionsLocation(String id) => '${placeLocation(id)}/directions';

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
/// The app bar's pages are siblings: going to one replaces the one showing. Back on any
/// but search goes to search, which has started over.
///
/// The app restores where it was (see `MainApp`): routes carry only IDs, and what they
/// show is in the store, so restoring the location restores the screens.
GoRouter appRouter(SparkleMapController map) => GoRouter(
  restorationScopeId: 'router',
  routes: [
    ShellRoute(
      restorationScopeId: 'sheet',
      observers: [sheetPages],
      builder: (context, state, child) => Scaffold(
        resizeToAvoidBottomInset: false,
        body: SheetScaffold(
          body: CardinalMap(controller: map),
          floating: const MapDrawingLayer(child: MapButtons()),
          bottomBar: const AppNavigationBar(),
          backdrop: sheetGlassOf(context),
          sheet: child,
        ),
      ),
      routes: sheetRoutes(),
    ),
  ],
);

/// The pages of the sheet.
List<RouteBase> sheetRoutes() => [
  GoRoute(
    path: '/',
    // The search sheet decides its own focus: see `SearchSheet`.
    pageBuilder: (context, state) =>
        _page(state, const SearchSheet(), requestFocus: false),
    routes: [
      GoRoute(
        path: 'place/:id',
        pageBuilder: (context, state) =>
            _page(state, PlacePage(id: state.pathParameters['id']!)),
        routes: [
          GoRoute(
            path: 'directions',
            pageBuilder: (context, state) => _page(
              state,
              HidesMapOverlays(
                child: DirectionsPage(to: state.pathParameters['id']!),
              ),
            ),
          ),
        ],
      ),
    ],
  ),
  GoRoute(
    path: '/saved',
    pageBuilder: (context, state) => _page(state, const SavedPage()),
    routes: [
      GoRoute(
        path: ':listId',
        pageBuilder: (context, state) => _page(
          state,
          SavedPage(listId: int.parse(state.pathParameters['listId']!)),
        ),
      ),
    ],
  ),
  _topLevel('/nearby', const StubPage(title: 'Nearby places')),
  _topLevel('/transit', const StubPage(title: 'Nearby transit')),
  _topLevel(
    '/offline',
    const OfflinePage(),
    routes: [
      GoRoute(
        path: 'download',
        pageBuilder: (context, state) => _page(state, const DownloadAreaPage()),
      ),
    ],
  ),
];

/// An app bar page other than search.
GoRoute _topLevel(
  String path,
  Widget child, {
  List<RouteBase> routes = const [],
}) => GoRoute(
  path: path,
  routes: routes,
  pageBuilder: (context, state) => _page(
    state,
    PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) context.go('/');
      },
      child: child,
    ),
  ),
);

/// A sheet page for [state]'s route, restorable.
SheetPage<void> _page(
  GoRouterState state,
  Widget child, {
  bool? requestFocus,
}) => SheetPage(
  key: state.pageKey,
  restorationId: state.pageKey.value,
  requestFocus: requestFocus,
  child: child,
);
