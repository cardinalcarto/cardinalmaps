/// Persistent state that routes refer to by ID.
///
/// - [Store] holds the stores for each kind of thing, and [StoreScope] provides it.
/// - [PlaceStore] holds [Place]s: POIs, addresses and administrative areas.
/// - [SavedStore] holds what the user has saved: nested [SavedList]s of [SavedPlace]s.
/// - [RecentSearchStore] holds local-only history of opened search results.
/// - [SettingsStore] holds the app's [Setting]s.
/// - [HeadsignStore] holds the headsign the rider last settled on for each route at each
///   stop, for nearby transit.
library;

export 'src/database.dart' show CardinalDatabase, SavedListRow, SavedPlaceRow;
export 'src/headsigns.dart';
export 'src/place.dart';
export 'src/places.dart' show PlaceStore;
export 'src/saved.dart';
export 'src/settings.dart';
export 'src/recent_searches.dart';
export 'src/store.dart';
