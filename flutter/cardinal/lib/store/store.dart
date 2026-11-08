/// Persistent state that routes refer to by ID.
///
/// - [Store] holds the stores for each kind of thing, and [StoreScope] provides it.
/// - [PlaceStore] holds [Place]s: POIs, addresses and administrative areas.
/// - [SavedStore] holds what the user has saved: nested [SavedList]s of [SavedPlace]s.
/// - [SettingsStore] holds the app's [Setting]s.
library;

export 'src/database.dart' show CardinalDatabase, SavedListRow, SavedPlaceRow;
export 'src/place.dart';
export 'src/places.dart';
export 'src/saved.dart';
export 'src/settings.dart';
export 'src/store.dart';
