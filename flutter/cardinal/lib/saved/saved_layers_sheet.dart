import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../store/store.dart';
import 'saved_names.dart';

/// Opens the full-screen sheet for what the map's Saved Places layer shows, over the
/// layer picker. Each tick applies as it's made.
Future<void> showSavedLayersSheet(BuildContext context) =>
    showModalBottomSheet<void>(
      context: context,
      useRootNavigator: true,
      isScrollControlled: true,
      enableDrag: false,
      constraints: const BoxConstraints.expand(),
      shape: const RoundedRectangleBorder(),
      builder: (context) => const _SavedLayersSheet(),
    );

/// A row of the tree: a [SavedList] or a [SavedPlace].
typedef _Row = ({SavedItem item, int depth});

/// All of Saved as a tree, every list and place with its own checkbox.
///
/// A place is on or off. A list is worked out from what's in it: on if all of it is, off
/// if none of it is, and partly on otherwise. Ticking a list turns on all of it, and
/// unticking turns off all of it; a list that's partly on is turned on.
class _SavedLayersSheet extends StatefulWidget {
  const _SavedLayersSheet();

  @override
  State<_SavedLayersSheet> createState() => _SavedLayersSheetState();
}

class _SavedLayersSheetState extends State<_SavedLayersSheet> {
  static const _rowHeight = 48.0;

  final _filter = TextEditingController();
  StreamSubscription<SavedTree>? _watch;
  SavedTree? _tree;

  /// The lists showing what's in them; null until the tree's first known.
  Set<int>? _expanded;

  @override
  void initState() {
    super.initState();
    _filter.addListener(() => setState(() {}));
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _watch ??= StoreScope.of(context).saved.watch().listen((tree) {
      setState(() {
        _tree = tree;
        // Open where the picks are, which partly-on lists lead to.
        _expanded ??= {
          for (final (:list, depth: _) in tree.walk)
            if (tree.shownState(list.id) == null) list.id,
        };
      });
    });
  }

  @override
  void dispose() {
    _watch?.cancel();
    _filter.dispose();
    super.dispose();
  }

  String get _query => _filter.text.trim().toLowerCase();

  List<_Row> _rows(SavedTree tree) {
    final query = _query;
    if (query.isNotEmpty) {
      return [
        for (final (:list, depth: _) in tree.walk)
          if (list.name.toLowerCase().contains(query)) (item: list, depth: 0),
        for (final place in tree.allPlaces)
          if (savedName(place).toLowerCase().contains(query))
            (item: place, depth: 0),
      ];
    }
    final rows = <_Row>[];
    void visit(int? parent, int depth) {
      for (final list in tree.listsIn(parent)) {
        rows.add((item: list, depth: depth));
        if (_expanded!.contains(list.id)) visit(list.id, depth + 1);
      }
      for (final place in tree.placesIn(parent)) {
        rows.add((item: place, depth: depth));
      }
    }

    visit(null, 0);
    return rows;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final tree = _tree;
    final rows = tree == null || _expanded == null
        ? const <_Row>[]
        : _rows(tree);
    return Padding(
      padding: EdgeInsets.only(
        // From the window: a modal sheet takes the status bar out of its media query.
        top: MediaQueryData.fromView(View.of(context)).padding.top,
        bottom: MediaQuery.viewInsetsOf(context).bottom,
      ),
      child: SafeArea(
        top: false,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsetsDirectional.fromSTEB(16, 4, 4, 4),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      'Saved places on the map',
                      style: theme.textTheme.titleLarge,
                    ),
                  ),
                  IconButton(
                    tooltip: 'Close',
                    icon: const Icon(Icons.close),
                    onPressed: () => Navigator.of(context).pop(),
                  ),
                ],
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
              child: TextField(
                controller: _filter,
                decoration: const InputDecoration(
                  isDense: true,
                  prefixIcon: Icon(Icons.search),
                  hintText: 'Filter',
                  border: OutlineInputBorder(),
                ),
              ),
            ),
            Expanded(
              child: tree != null && rows.isEmpty && _query.isEmpty
                  ? Padding(
                      padding: const EdgeInsets.all(24),
                      child: Text(
                        'No saved places yet',
                        textAlign: TextAlign.center,
                        style: theme.textTheme.bodyMedium?.copyWith(
                          color: theme.colorScheme.onSurfaceVariant,
                        ),
                      ),
                    )
                  : ListView.builder(
                      itemExtent: _rowHeight,
                      itemCount: rows.length,
                      itemBuilder: (context, index) =>
                          _tile(tree!, rows[index]),
                    ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _tile(SavedTree tree, _Row row) {
    final theme = Theme.of(context);
    final item = row.item;
    final filtering = _query.isNotEmpty;
    final state = switch (item) {
      SavedList(:final id) => tree.shownState(id),
      SavedPlace(:final shownOnMap) => shownOnMap,
    };
    void toggle() =>
        unawaited(StoreScope.of(context).saved.setShown(item, state != true));
    final id = item is SavedList ? item.id : null;
    final expandable = id != null && tree.count(id) > 0 && !filtering;
    final open = _expanded!.contains(id);
    return InkWell(
      onTap: toggle,
      child: Padding(
        padding: EdgeInsetsDirectional.only(
          start: 8.0 + row.depth * 16,
          end: 16,
        ),
        child: Row(
          children: [
            SizedBox(
              width: 32,
              child: !expandable
                  ? null
                  : IconButton(
                      tooltip: open ? 'Collapse' : 'Expand',
                      visualDensity: VisualDensity.compact,
                      padding: EdgeInsets.zero,
                      icon: Icon(
                        open ? Icons.expand_more : Icons.chevron_right,
                      ),
                      onPressed: () => setState(() {
                        if (!_expanded!.remove(id)) _expanded!.add(id);
                      }),
                    ),
            ),
            Checkbox(
              value: state,
              tristate: true,
              visualDensity: VisualDensity.compact,
              onChanged: (_) => toggle(),
            ),
            const SizedBox(width: 4),
            SizedBox(
              width: 24,
              child: switch (item) {
                SavedPlace(emoji: final emoji?) => Text(
                  emoji,
                  textAlign: TextAlign.center,
                  style: const TextStyle(fontSize: 18),
                ),
                SavedPlace() => const Icon(Icons.place_outlined),
                SavedList() => const Icon(Icons.folder_outlined),
              },
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Text(
                _title(tree, item, filtering),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.bodyLarge,
              ),
            ),
            if (item is SavedList)
              Text('${tree.count(item.id)}', style: theme.textTheme.bodySmall),
          ],
        ),
      ),
    );
  }

  /// A row's text: filtered rows have left the tree, so say where they are.
  String _title(SavedTree tree, SavedItem item, bool filtering) {
    if (!filtering) return savedName(item);
    return switch (item) {
      SavedList(:final id) => listPath(tree, id),
      SavedPlace(:final parent) =>
        parent == null
            ? savedName(item)
            : '${listPath(tree, parent)} › ${savedName(item)}',
    };
  }
}
