import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import 'directions/directions.dart';
import 'directions/directions_page.dart';
import 'directions/steps_page.dart';
import 'directions/trip_details_page.dart';
import 'home/navigation_bar.dart';
import 'home/stub_page.dart';
import 'map/buttons.dart';
import 'map/map.dart';
import 'map/map_drawing.dart';
import 'map/overlays.dart';
import 'navigation/navigation_page.dart';
import 'offline/download_page.dart';
import 'offline/geocoder_models.dart';
import 'offline/offline_page.dart';
import 'place/place_page.dart';
import 'saved/saved_page.dart';
import 'search/search_sheet.dart';
import 'sheet/sheet.dart';
import 'transit/nearby_page.dart';
import 'transit/nearby_transit.dart' show NearbyRoute;
import 'transit/route_detail_page.dart';
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

/// The location of the steps of the route selected in directions to the place with
/// [id]. The directions page pushes it with its trip, which this page shows the
/// selected route of; without one, as when the app restores it, it's directions.
String stepsLocation(String id) => '${directionsLocation(id)}/steps';

/// The location of the details of the transit journey open in directions to the place
/// with [id]. The directions page pushes it with the journey it opens; without one, as
/// when the app restores it, it's directions.
String tripDetailsLocation(String id) => '${directionsLocation(id)}/trip';

/// The location of the navigation screen, over directions to the place with [id].
String navigationLocation(String id) => '${directionsLocation(id)}/navigate';

/// The location of a nearby route's detail: [routeId] of [feed] from [stopId], on the
/// page of [headsigns] (none for the departures without one).
///
/// `/transit/route/<feed>/<route>/<stop>?headsign=…`, each ID escaped as a place's is,
/// with a `headsign` for each of the page's, in its order.
String routeDetailLocation({
  required String feed,
  required String routeId,
  required String stopId,
  required List<String> headsigns,
}) => Uri(
  path: [
    '/transit/route',
    Uri.encodeComponent(feed),
    Uri.encodeComponent(routeId),
    Uri.encodeComponent(stopId),
  ].join('/'),
  queryParameters: headsigns.isEmpty ? null : {'headsign': headsigns},
).toString();

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
      // Offline search's models are offered over the map once it's up.
      builder: (context, state, child) => GeocoderModelsPrompt(
        child: Scaffold(
          resizeToAvoidBottomInset: false,
          body: SheetScaffold(
            body: CardinalMap(controller: map),
            floating: const NavigationLayer(
              child: MapDrawingLayer(child: MapButtons()),
            ),
            bottomBar: const AppNavigationBar(),
            backdrop: sheetGlassOf(context),
            sheet: child,
          ),
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
            routes: [
              GoRoute(
                path: 'steps',
                redirect: (context, state) => state.extra is Directions
                    ? null
                    : directionsLocation(state.pathParameters['id']!),
                pageBuilder: (context, state) => _page(
                  state,
                  HidesMapOverlays(
                    child: StepsPage(
                      directions: state.extra! as Directions,
                      to: state.pathParameters['id']!,
                    ),
                  ),
                ),
              ),
              GoRoute(
                path: 'trip',
                redirect: (context, state) => switch (state.extra) {
                  OpenJourney(isOpen: true) => null,
                  _ => directionsLocation(state.pathParameters['id']!),
                },
                pageBuilder: (context, state) => _page(
                  state,
                  HidesMapOverlays(
                    child: TripDetailsPage(
                      journey: state.extra! as OpenJourney,
                    ),
                  ),
                ),
              ),
              GoRoute(
                path: 'navigate',
                pageBuilder: (context, state) => _page(
                  state,
                  const HidesMapOverlays(child: NavigationPage()),
                ),
              ),
            ],
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
  _topLevel(
    '/transit',
    const NearbyTransitPage(),
    routes: [
      GoRoute(
        path: 'route/:feed/:route/:stop',
        pageBuilder: (context, state) => _page(
          state,
          HidesMapOverlays(
            child: RouteDetailPage(
              feed: state.pathParameters['feed']!,
              routeId: state.pathParameters['route']!,
              stopId: state.pathParameters['stop']!,
              headsigns: state.uri.queryParametersAll['headsign'] ?? const [],
              // The row it was opened from, unless the app restored it.
              from: state.extra is NearbyRoute
                  ? state.extra! as NearbyRoute
                  : null,
            ),
          ),
        ),
      ),
    ],
  ),
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
