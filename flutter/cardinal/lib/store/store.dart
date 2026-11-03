/// Persistent state that routes refer to by ID.
///
/// - [Store] holds the stores for each kind of thing, and [StoreScope] provides it.
/// - [PlaceStore] holds [Place]s: POIs, addresses and administrative areas.
library;

export 'src/database.dart' show CardinalDatabase;
export 'src/place.dart';
export 'src/places.dart';
export 'src/store.dart';
