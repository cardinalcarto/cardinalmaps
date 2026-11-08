import 'dart:async';
import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import '../store/store.dart';
import 'saved_dialogs.dart';
import 'saved_names.dart';

/// Asks where to [verb] (Move, Copy or Add) [items], from Saved's lists. Returns the
/// chosen list, null within the record for the top level, or null if cancelled.
///
/// Top-level lists show with their nested lists folded away, except those leading to
/// anything in [unavailable]; typing in the filter shows every list that matches
/// instead, by its path. Disabled are [unavailable], a list being moved and what's in
/// it, and anywhere all the [items] already are.
Future<({int? list})?> showDestinationPicker(
  BuildContext context, {
  required List<SavedItem> items,
  required Set<int?> unavailable,
  String verb = 'Move',
}) => showModalBottomSheet<({int? list})>(
  context: context,
  isScrollControlled: true,
  // Above the app's sheet, whose pages must stay current.
  useRootNavigator: true,
  showDragHandle: true,
  builder: (context) =>
      _DestinationPicker(items: items, unavailable: unavailable, verb: verb),
);

/// A row of the picker: the top level if [list] is null.
typedef _Row = ({SavedList? list, int depth});

class _DestinationPicker extends StatefulWidget {
  const _DestinationPicker({
    required this.items,
    required this.unavailable,
    required this.verb,
  });

  final List<SavedItem> items;
  final Set<int?> unavailable;
  final String verb;

  @override
  State<_DestinationPicker> createState() => _DestinationPickerState();
}

class _DestinationPickerState extends State<_DestinationPicker> {
  static const _rowHeight = 48.0;

  /// The title, filter and buttons together.
  static const _chromeHeight = 168.0;

  final _filter = TextEditingController();

  /// The picked destination, null within the record for the top level. It stays picked
  /// while hidden, but only counts while it shows.
  ({int? list})? _picked;
  SavedTree? _tree;
  StreamSubscription<SavedTree>? _watch;

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
        _expanded ??= {
          for (final list in widget.unavailable.nonNulls)
            ...tree.ancestors(list).map((ancestor) => ancestor.id),
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
        if ('saved'.contains(query)) (list: null, depth: 0),
        for (final (:list, depth: _) in tree.walk)
          if (list.name.toLowerCase().contains(query)) (list: list, depth: 0),
      ];
    }
    final rows = <_Row>[(list: null, depth: 0)];
    void visit(int? parent, int depth) {
      for (final list in tree.listsIn(parent)) {
        rows.add((list: list, depth: depth));
        if (_expanded!.contains(list.id)) visit(list.id, depth + 1);
      }
    }

    visit(null, 0);
    return rows;
  }

  /// Whether every item is a place with a copy in [list] already.
  bool _allThere(SavedTree tree, int? list) => widget.items.every(
    (item) =>
        item is SavedPlace &&
        tree.copiesOf(item.id).any((copy) => copy.parent == list),
  );

  /// The picked destination, if it's showing.
  ({int? list})? _pickedIn(List<_Row> rows) {
    final picked = _picked;
    return picked != null && rows.any((row) => row.list?.id == picked.list)
        ? picked
        : null;
  }

  Future<void> _newList(({int? list})? under) async {
    final parent = under?.list;
    final saved = StoreScope.of(context).saved;
    final tree = _tree!;
    final name = await showDialog<String>(
      context: context,
      builder: (context) => const NewListDialog(),
    );
    if (name == null) return;
    final id = await saved.createList(name, parent: parent);
    if (!mounted) return;
    // Where it can be seen.
    _filter.clear();
    setState(() {
      _expanded!.addAll([
        ?parent,
        if (parent != null) ...tree.ancestors(parent).map((list) => list.id),
      ]);
      _picked = (list: id);
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final tree = _tree;
    final count = widget.items.length;
    final media = MediaQuery.of(context);
    final keyboard = media.viewInsets.bottom;
    // By how many lists there are, not how many show, to hold still while filtering.
    final height = math.min(
      (media.size.height - keyboard) * 0.7,
      _chromeHeight + _rowHeight * ((tree?.walk.length ?? 0) + 1),
    );
    final rows = tree == null ? const <_Row>[] : _rows(tree);
    final disabled = tree == null
        ? const <int>{}
        : {
            for (final item in widget.items)
              if (item is SavedList) ...{item.id, ...tree.descendants(item.id)},
          };
    final picked = _pickedIn(rows);
    return Padding(
      padding: EdgeInsets.only(bottom: keyboard),
      child: SafeArea(
        child: SizedBox(
          height: height,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
                child: Text(
                  '${widget.verb} $count ${count == 1 ? 'item' : 'items'} to',
                  style: theme.textTheme.titleMedium,
                ),
              ),
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
                child: TextField(
                  controller: _filter,
                  decoration: const InputDecoration(
                    isDense: true,
                    prefixIcon: Icon(Icons.search),
                    hintText: 'Filter lists',
                    border: OutlineInputBorder(),
                  ),
                ),
              ),
              Expanded(
                child: tree == null
                    ? const SizedBox.shrink()
                    : ListView.builder(
                        itemExtent: _rowHeight,
                        itemCount: rows.length,
                        itemBuilder: (context, index) {
                          final row = rows[index];
                          final id = row.list?.id;
                          return _tile(
                            tree,
                            row,
                            disabled:
                                widget.unavailable.contains(id) ||
                                disabled.contains(id) ||
                                _allThere(tree, id),
                          );
                        },
                      ),
              ),
              Padding(
                padding: const EdgeInsets.fromLTRB(8, 4, 8, 8),
                child: Row(
                  children: [
                    TextButton(
                      onPressed: tree == null ? null : () => _newList(picked),
                      child: const Text('New list here'),
                    ),
                    const Spacer(),
                    FilledButton(
                      onPressed: picked == null
                          ? null
                          : () => Navigator.of(context).pop(picked),
                      child: Text('${widget.verb} here'),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _tile(SavedTree tree, _Row row, {required bool disabled}) {
    final list = row.list;
    final id = list?.id;
    final filtering = _query.isNotEmpty;
    final nested = id != null && tree.listsIn(id).isNotEmpty;
    final open = _expanded!.contains(id);
    return ListTile(
      dense: true,
      visualDensity: VisualDensity.compact,
      enabled: !disabled,
      selected: _picked != null && _picked!.list == id,
      selectedTileColor: Theme.of(context).colorScheme.secondaryContainer,
      contentPadding: EdgeInsetsDirectional.only(
        start: 8.0 + row.depth * 16,
        end: 16,
      ),
      minLeadingWidth: 24,
      horizontalTitleGap: 8,
      leading: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          SizedBox(
            width: 32,
            child: !nested || filtering
                ? null
                : IconButton(
                    tooltip: open ? 'Collapse' : 'Expand',
                    visualDensity: VisualDensity.compact,
                    padding: EdgeInsets.zero,
                    icon: Icon(open ? Icons.expand_more : Icons.chevron_right),
                    onPressed: () => setState(() {
                      if (!_expanded!.remove(id)) _expanded!.add(id);
                    }),
                  ),
          ),
          Icon(id == null ? Icons.star_border : Icons.folder_outlined),
        ],
      ),
      title: Text(
        list == null
            ? 'Saved'
            : filtering
            ? listPath(tree, list.id)
            : list.name,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      onTap: () => setState(() => _picked = (list: id)),
    );
  }
}
