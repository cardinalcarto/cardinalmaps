import '../geocoder/address.dart';
import '../store/store.dart';

/// What to call [item] in Saved: for a place, the user's name for it or its own.
String savedName(SavedItem item) => switch (item) {
  SavedList(:final name) => name,
  SavedPlace(:final customName, :final place) =>
    customName ?? placeTitle(place),
};

/// The second line under [item] in Saved: the user's note, or for a place where it is.
String? savedSubtitle(SavedItem item) => switch (item) {
  SavedList(:final note) => note,
  SavedPlace(:final note, :final place) => note ?? placeSubtitle(place),
};

/// Where a list is, for messages: a list's name, or Saved for the top level.
String destinationName(SavedTree tree, int? list) =>
    list == null ? 'Saved' : tree.list(list)?.name ?? 'Saved';

/// The path to [list] from the top of Saved, such as `Saved › Food › Italian`.
String locationPath(SavedTree tree, int? list) {
  final saved = list == null ? null : tree.list(list);
  return [
    'Saved',
    if (saved != null) ...[
      ...tree.ancestors(saved.id).map((l) => l.name),
      saved.name,
    ],
  ].join(' › ');
}

/// The path to [list] from the top of Saved, without Saved, such as `Food › Italian`.
String listPath(SavedTree tree, int list) => [
  ...tree.ancestors(list).map((l) => l.name),
  ?tree.list(list)?.name,
].join(' › ');
