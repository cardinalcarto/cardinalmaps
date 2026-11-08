import 'dart:async';

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../geocoder/address.dart';
import '../routes.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import 'destination_picker.dart';
import 'saved_dialogs.dart';
import 'saved_feedback.dart';
import 'saved_names.dart';

/// Saved: one list's nested lists and places, the top level if [listId] is null.
///
/// Browsing, or editing the page's items, a mode of the page itself:
///
/// | In | When | Then |
/// |---|---|---|
/// | browsing | a place's tapped | its card opens |
/// | browsing | a list's tapped | it opens as a page over this one |
/// | browsing | a crumb's tapped | back to that list |
/// | browsing | back | up a list; from the top level, to search |
/// | browsing | Edit | editing, with nothing selected |
/// | editing | a row's tapped | it's selected, or not |
/// | editing | a handle's dragged | the row moves among the lists, or the places |
/// | editing | Move | a picker; once chosen, the selection moves, with Undo |
/// | editing | Copy, only places selected | a picker; once chosen, they're there too, with Undo |
/// | editing | Pin | the selected places are pinned, or if all are, unpinned |
/// | editing | Edit details, one selected | its name and note |
/// | editing | Delete | the selection goes, with its contents, with Undo |
/// | editing | Done, or back | browsing |
///
/// Selected items that go away (moved, deleted) leave the selection.
class SavedPage extends StatefulWidget {
  const SavedPage({super.key, this.listId});

  final int? listId;

  @override
  State<SavedPage> createState() => _SavedPageState();
}

class _SavedPageState extends State<SavedPage> {
  /// Space for the action bar, over the end of the rows while editing.
  static const _actionBarHeight = 56.0;

  StreamSubscription<SavedTree>? _watch;
  SavedTree? _tree;
  List<SavedList> _lists = const [];
  List<SavedPlace> _places = const [];

  bool _editing = false;
  final _selected = <Object>{};

  SavedStore get _store => StoreScope.of(context).saved;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _watch ??= _store.watch().listen(_changed);
  }

  @override
  void dispose() {
    _watch?.cancel();
    super.dispose();
  }

  void _changed(SavedTree tree) {
    setState(() {
      _tree = tree;
      _lists = tree.listsIn(widget.listId);
      _places = tree.placesIn(widget.listId);
      _selected.retainAll({
        for (final item in [..._lists, ..._places]) item.key,
      });
    });
  }

  List<SavedItem> get _items => [..._lists, ..._places];

  List<SavedItem> get _selection => [
    for (final item in _items)
      if (_selected.contains(item.key)) item,
  ];

  void _edit() => setState(() => _editing = true);

  void _stopEditing() => setState(() {
    _editing = false;
    _selected.clear();
  });

  void _toggle(SavedItem item) => setState(() {
    if (!_selected.remove(item.key)) _selected.add(item.key);
  });

  void _onPop(bool didPop, Object? result) {
    if (didPop) return;
    if (_editing) {
      _stopEditing();
    } else if (widget.listId == null) {
      context.go('/');
    }
  }

  void _openItem(SavedItem item) {
    switch (item) {
      case SavedList(:final id):
        unawaited(context.push<void>('/saved/$id'));
      case SavedPlace(:final id):
        unawaited(context.push<void>(placeLocation(id)));
    }
  }

  /// Goes up to the list [levels] above this one.
  void _goUp(int levels) {
    final router = GoRouter.of(context);
    for (var i = 0; i < levels; i++) {
      router.pop();
    }
  }

  void _message(String message, {SavedSnapshot? undo}) {
    if (!mounted) return;
    final store = _store;
    showSheetMessage(
      context,
      message,
      above: _editing ? _actionBarHeight + 8 : 0,
      action: undo == null ? null : 'Undo',
      onAction: undo == null ? null : () => unawaited(store.restore(undo)),
    );
  }

  Future<void> _newList() async {
    final name = await showDialog<String>(
      context: context,
      builder: (context) => const NewListDialog(),
    );
    if (name != null) await _store.createList(name, parent: widget.listId);
  }

  Future<void> _move() async {
    final items = _selection;
    final picked = await showDestinationPicker(
      context,
      items: items,
      unavailable: {widget.listId},
    );
    if (picked == null || !mounted) return;
    final tree = _tree;
    final undo = await _store.move(items, picked.list);
    if (tree != null) {
      _message('Moved to ${destinationName(tree, picked.list)}', undo: undo);
    }
  }

  Future<void> _copy() async {
    final items = _selection.cast<SavedPlace>();
    final picked = await showDestinationPicker(
      context,
      items: items,
      unavailable: {widget.listId},
      verb: 'Copy',
    );
    if (picked == null || !mounted) return;
    final tree = _tree;
    final undo = await _store.copy(items, picked.list);
    if (tree != null) {
      _message('Copied to ${destinationName(tree, picked.list)}', undo: undo);
    }
  }

  Future<void> _togglePinned() async {
    final places = _selection.whereType<SavedPlace>().toList();
    await _store.setPinned([
      for (final place in places) place.entryId,
    ], !places.every((p) => p.pinned));
  }

  Future<void> _editDetails() async {
    final item = _selection.single;
    final fields = await showDialog<({String name, String note})>(
      context: context,
      builder: (context) => EditItemDialog(
        title: item is SavedList ? 'Edit list' : 'Edit place',
        name: switch (item) {
          SavedList(:final name) => name,
          SavedPlace(:final customName) => customName ?? '',
        },
        note: item.note,
        hint: switch (item) {
          SavedList() => null,
          SavedPlace(:final place) => placeTitle(place),
        },
      ),
    );
    if (fields != null) {
      await _store.edit(item, name: fields.name, note: fields.note);
    }
  }

  Future<void> _delete() async {
    final items = _selection;
    final undo = await _store.delete(items);
    _message(
      'Deleted ${items.length} ${items.length == 1 ? 'item' : 'items'}',
      undo: undo,
    );
  }

  void _reorderLists(int from, int to) {
    setState(() => _lists = _moved(_lists, from, to));
    unawaited(_store.reorderLists([for (final list in _lists) list.id]));
  }

  void _reorderPlaces(int from, int to) {
    setState(() => _places = _moved(_places, from, to));
    unawaited(
      _store.reorderPlaces([for (final place in _places) place.entryId]),
    );
  }

  List<T> _moved<T>(List<T> items, int from, int to) {
    final moved = [...items];
    moved.insert(to, moved.removeAt(from));
    return moved;
  }

  @override
  Widget build(BuildContext context) {
    final tree = _tree;
    final colors = Theme.of(context).colorScheme;
    final view = SheetScaffold.viewOf(context);
    final bottom =
        view.geometry.padding.bottom + SheetScaffold.bottomBarHeightOf(context);
    final selection = _selection;
    final places = selection.whereType<SavedPlace>().toList();
    return PopScope(
      canPop: widget.listId != null && !_editing,
      onPopInvokedWithResult: _onPop,
      child: Stack(
        children: [
          SheetContent(
            showsBottomBar: true,
            anchors: const [SheetAnchor.full()],
            // A full screen, with nowhere to be dragged to.
            draggable: false,
            slivers: [
              PinnedHeaderSliver(
                child: ColoredBox(
                  color: colors.surfaceContainerLow,
                  child: _editing
                      ? _EditingHeader(
                          count: selection.length,
                          allSelected:
                              _items.isNotEmpty &&
                              selection.length == _items.length,
                          onSelectAll: () => setState(() {
                            _selected
                              ..clear()
                              ..addAll(_items.map((item) => item.key));
                          }),
                          onClear: () => setState(_selected.clear),
                          onDone: _stopEditing,
                        )
                      : _BrowsingHeader(
                          title: widget.listId == null
                              ? 'Saved places'
                              : tree?.list(widget.listId!)?.name ?? '',
                          crumbs: widget.listId == null
                              ? const []
                              : [
                                  'Saved',
                                  ...?tree
                                      ?.ancestors(widget.listId!)
                                      .map((list) => list.name),
                                ],
                          onCrumb: _goUp,
                          onNewList: _newList,
                          onEdit: _items.isEmpty ? null : _edit,
                        ),
                ),
              ),
              if (tree != null && _items.isEmpty)
                const SliverToBoxAdapter(child: _Empty()),
              SliverReorderableList(
                itemCount: _lists.length,
                onReorderItem: _reorderLists,
                proxyDecorator: _lifted,
                itemBuilder: (context, index) => _row(_lists[index], index),
              ),
              SliverReorderableList(
                itemCount: _places.length,
                onReorderItem: _reorderPlaces,
                proxyDecorator: _lifted,
                itemBuilder: (context, index) => _row(_places[index], index),
              ),
              if (_editing)
                const SliverToBoxAdapter(
                  child: SizedBox(height: _actionBarHeight + 16),
                ),
            ],
          ),
          if (_editing)
            PositionedDirectional(
              start: 8,
              end: 8,
              bottom: bottom + 8,
              height: _actionBarHeight,
              child: _ActionBar(
                onMove: selection.isEmpty ? null : _move,
                onCopy: selection.isEmpty || places.length != selection.length
                    ? null
                    : _copy,
                pinLabel: places.isNotEmpty && places.every((p) => p.pinned)
                    ? 'Unpin'
                    : 'Pin',
                onPin: places.isEmpty ? null : _togglePinned,
                onEdit: selection.length == 1 ? _editDetails : null,
                onDelete: selection.isEmpty ? null : _delete,
              ),
            ),
        ],
      ),
    );
  }

  Widget _row(SavedItem item, int index) => _Row(
    key: ValueKey(item.key),
    item: item,
    index: index,
    editing: _editing,
    selected: _selected.contains(item.key),
    count: item is SavedList ? _tree?.count(item.id) ?? 0 : 0,
    onTap: () => _editing ? _toggle(item) : _openItem(item),
  );

  Widget _lifted(Widget child, int index, Animation<double> animation) =>
      Material(
        elevation: 4,
        color: Theme.of(context).colorScheme.surfaceContainerLow,
        child: child,
      );
}

class _BrowsingHeader extends StatelessWidget {
  const _BrowsingHeader({
    required this.title,
    required this.crumbs,
    required this.onCrumb,
    required this.onNewList,
    required this.onEdit,
  });

  final String title;

  /// The lists above this one, from the top level down.
  final List<String> crumbs;

  /// A crumb's tapped: how many levels up it is.
  final void Function(int levels) onCrumb;
  final VoidCallback onNewList;
  final VoidCallback? onEdit;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    return Padding(
      padding: const EdgeInsetsDirectional.only(
        start: 20,
        end: 8,
        top: 8,
        bottom: 8,
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                if (crumbs.isNotEmpty)
                  SingleChildScrollView(
                    scrollDirection: Axis.horizontal,
                    reverse: true,
                    child: Row(
                      children: [
                        for (final (index, crumb) in crumbs.indexed) ...[
                          InkWell(
                            onTap: () => onCrumb(crumbs.length - index),
                            child: Padding(
                              padding: const EdgeInsets.symmetric(vertical: 2),
                              child: Text(crumb, style: muted),
                            ),
                          ),
                          Icon(
                            Icons.chevron_right,
                            size: 14,
                            color: muted?.color,
                          ),
                        ],
                      ],
                    ),
                  ),
                Text(
                  title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.titleLarge,
                ),
              ],
            ),
          ),
          IconButton(
            tooltip: 'New list',
            icon: const Icon(Icons.create_new_folder_outlined),
            onPressed: onNewList,
          ),
          TextButton(onPressed: onEdit, child: const Text('Edit')),
        ],
      ),
    );
  }
}

class _EditingHeader extends StatelessWidget {
  const _EditingHeader({
    required this.count,
    required this.allSelected,
    required this.onSelectAll,
    required this.onClear,
    required this.onDone,
  });

  final int count;
  final bool allSelected;
  final VoidCallback onSelectAll;
  final VoidCallback onClear;
  final VoidCallback onDone;

  @override
  Widget build(BuildContext context) => SizedBox(
    height: 64,
    child: Padding(
      padding: const EdgeInsetsDirectional.only(start: 20, end: 8),
      child: Row(
        children: [
          Expanded(
            child: Text(
              count == 0 ? 'Select items' : '$count selected',
              style: Theme.of(context).textTheme.titleMedium,
            ),
          ),
          TextButton(
            onPressed: allSelected ? onClear : onSelectAll,
            child: Text(allSelected ? 'Clear' : 'Select all'),
          ),
          TextButton(onPressed: onDone, child: const Text('Done')),
        ],
      ),
    ),
  );
}

class _Empty extends StatelessWidget {
  const _Empty();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Text(
        'No saved places yet',
        textAlign: TextAlign.center,
        style: theme.textTheme.bodyMedium?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({
    super.key,
    required this.item,
    required this.index,
    required this.editing,
    required this.selected,
    required this.count,
    required this.onTap,
  });

  final SavedItem item;
  final int index;
  final bool editing;
  final bool selected;
  final int count;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final item = this.item;
    final subtitle = savedSubtitle(item);
    return ListTile(
      dense: true,
      visualDensity: VisualDensity.compact,
      contentPadding: const EdgeInsetsDirectional.only(start: 20, end: 16),
      minLeadingWidth: 24,
      horizontalTitleGap: 16,
      minTileHeight: 72,
      selected: editing && selected,
      selectedTileColor: theme.colorScheme.secondaryContainer,
      leading: editing
          ? Checkbox(
              value: selected,
              visualDensity: VisualDensity.compact,
              onChanged: (_) => onTap(),
            )
          : Icon(switch (item) {
              SavedList() => Icons.folder_outlined,
              SavedPlace(pinned: true) => Icons.push_pin,
              SavedPlace() => Icons.place_outlined,
            }),
      title: Text(
        savedName(item),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: theme.textTheme.bodyLarge,
      ),
      subtitle: subtitle == null
          ? null
          : Text(
              subtitle,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
      trailing: editing
          ? ReorderableDragStartListener(
              index: index,
              child: const Padding(
                padding: EdgeInsets.all(8),
                child: Icon(Icons.drag_handle),
              ),
            )
          : item is SavedList
          ? Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text('$count', style: theme.textTheme.bodySmall),
                const Icon(Icons.chevron_right),
              ],
            )
          : null,
      onTap: onTap,
    );
  }
}

class _ActionBar extends StatelessWidget {
  const _ActionBar({
    required this.onMove,
    required this.onCopy,
    required this.pinLabel,
    required this.onPin,
    required this.onEdit,
    required this.onDelete,
  });

  final VoidCallback? onMove;
  final VoidCallback? onCopy;
  final String pinLabel;
  final VoidCallback? onPin;
  final VoidCallback? onEdit;
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    Widget action(IconData icon, String label, VoidCallback? onPressed) =>
        Expanded(
          child: TextButton(
            onPressed: onPressed,
            style: const ButtonStyle(
              padding: WidgetStatePropertyAll(EdgeInsets.zero),
            ),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(icon, size: 20),
                Text(label, maxLines: 1, overflow: TextOverflow.fade),
              ],
            ),
          ),
        );
    return Material(
      color: Theme.of(context).colorScheme.surfaceContainerHigh,
      elevation: 2,
      borderRadius: BorderRadius.circular(28),
      clipBehavior: Clip.antiAlias,
      child: Row(
        children: [
          action(Icons.drive_file_move_outlined, 'Move', onMove),
          action(Icons.content_copy, 'Copy', onCopy),
          action(Icons.push_pin_outlined, pinLabel, onPin),
          action(Icons.edit_outlined, 'Edit', onEdit),
          action(Icons.delete_outline, 'Delete', onDelete),
        ],
      ),
    );
  }
}
