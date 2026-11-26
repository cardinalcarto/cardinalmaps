import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoActionSheet,
        CupertinoActionSheetAction,
        CupertinoButton,
        CupertinoColors,
        CupertinoContextMenu,
        CupertinoContextMenuAction,
        CupertinoIcons,
        CupertinoMenuAnchor,
        CupertinoMenuItem,
        showCupertinoModalPopup;
import 'package:flutter_slidable/flutter_slidable.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../geocoder/address.dart';
import '../routes.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
import '../ui/glass.dart';
import 'destination_picker.dart';
import 'saved_dialogs.dart';
import 'saved_feedback.dart';
import 'saved_names.dart';

/// Saved: one list's nested lists and places, the top level if [listId] is null.
///
/// Browsing, or selecting the page's items, a mode of the page itself. The sheet stays
/// at the height it's at, either way, and can't be collapsed while selecting.
///
/// Both platforms:
///
/// | In | When | Then |
/// |---|---|---|
/// | browsing | a place's tapped | its card opens |
/// | browsing | a list's tapped | it opens as a page over this one, at the same height |
/// | browsing | back | up a list; from the top level, to search |
/// | selecting | a row's tapped | it's selected, or not |
/// | selecting | a row's swiped away | it goes, with its contents, with Undo |
/// | selecting | a handle's dragged | the row moves among the lists, or the places |
/// | selecting | Move | a picker; once chosen, the selection moves, with Undo |
/// | selecting | Copy, only places selected | a picker; once chosen, they're there too, with Undo |
/// | selecting | Pin, only places selected | they're pinned, or unpinned if they all were |
/// | selecting | Delete | the selection goes, with its contents, with Undo |
/// | selecting | Done, or back | browsing |
///
/// Android:
///
/// | In | When | Then |
/// |---|---|---|
/// | browsing | the back arrow, or a crumb | up to that list |
/// | browsing | ⋮ → Select | selecting, with nothing selected |
/// | browsing | a row's long-pressed | selecting, with that row selected |
/// | browsing | a row's ⋮ → Edit | its name, note and emoji |
/// | browsing | a place's ⋮ → Pin or Unpin | that copy's pinned, or unpinned |
/// | browsing | a row's ⋮ → Move, or a place's → Copy | a picker, then Undo |
/// | browsing | a row's ⋮ → Delete | selecting, with that row selected |
/// | selecting | a row's long-pressed | as a tap |
/// | selecting | Edit, one selected | its name, note and emoji |
/// | selecting | ✕ | browsing |
///
/// iOS, where the bottom bar gives way to the selection's toolbar while selecting:
///
/// | In | When | Then |
/// |---|---|---|
/// | browsing | the back button | up a list |
/// | browsing | the back button's long-pressed | a menu of the lists above; up to the one picked |
/// | browsing | ⋯ → Select | selecting, with nothing selected |
/// | browsing | ⋯ → New List | a new list here |
/// | browsing | a row's long-pressed | its menu: Pin or Unpin, Edit, Move, Copy, Select, Delete |
/// | browsing | a row's swiped left | More and Delete; all the way, it's deleted, with Undo |
/// | browsing | More | an action sheet of the row's menu |
/// | browsing | a place's swiped right | Pin or Unpin; all the way, it's done |
/// | browsing | a menu's Select | selecting, with that row selected |
/// | browsing | a menu's Delete | it goes at once, with Undo |
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

  /// Selecting, with [item] selected if there is one.
  void _select([SavedItem? item]) => setState(() {
    _editing = true;
    if (item != null) _selected.add(item.key);
  });

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
      // Over iOS's toolbar along the bottom.
      above: _editing && isCupertinoOf(context) ? _actionBarHeight + 8 : 0,
      action: undo == null ? null : 'Undo',
      onAction: undo == null ? null : () => unawaited(store.restore(undo)),
    );
  }

  Future<void> _newList() async {
    final name = await showAdaptiveDialog<String>(
      context: context,
      builder: (context) => const NewListDialog(),
    );
    if (name != null) await _store.createList(name, parent: widget.listId);
  }

  /// Moves [these], or else the selection, to a list the picker asks for.
  Future<void> _move([List<SavedItem>? these]) async {
    final items = these ?? _selection;
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

  /// Copies [these], or else the selection, all places, to a list the picker asks
  /// for.
  Future<void> _copy([List<SavedPlace>? these]) async {
    final items = these ?? _selection.cast<SavedPlace>();
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

  Future<void> _togglePinned(SavedPlace place) =>
      _store.setPinned([place.entryId], !place.pinned);

  /// Pins the selected places, or unpins them if they all are.
  Future<void> _pinSelection() {
    final places = _selection.cast<SavedPlace>();
    final pin = !places.every((place) => place.pinned);
    return _store.setPinned([for (final p in places) p.entryId], pin);
  }

  /// What a row's menu offers on iOS, as a context menu or an action sheet.
  List<_Choice> _choices(SavedItem item) => [
    if (item case final SavedPlace place)
      (
        label: place.pinned ? 'Unpin' : 'Pin',
        icon: place.pinned ? CupertinoIcons.pin_slash : CupertinoIcons.pin,
        destructive: false,
        onPressed: () => unawaited(_togglePinned(place)),
      ),
    (
      label: 'Edit',
      icon: CupertinoIcons.pencil,
      destructive: false,
      onPressed: () => unawaited(_editDetails(item)),
    ),
    (
      label: 'Move',
      icon: CupertinoIcons.folder,
      destructive: false,
      onPressed: () => unawaited(_move([item])),
    ),
    if (item case final SavedPlace place)
      (
        label: 'Copy',
        icon: CupertinoIcons.doc_on_doc,
        destructive: false,
        onPressed: () => unawaited(_copy([place])),
      ),
    (
      label: 'Select',
      icon: CupertinoIcons.checkmark_circle,
      destructive: false,
      onPressed: () => _select(item),
    ),
    (
      label: 'Delete',
      icon: CupertinoIcons.trash,
      destructive: true,
      onPressed: () => unawaited(_delete([item])),
    ),
  ];

  /// iOS's More: the row's menu, as an action sheet.
  Future<void> _more(SavedItem item) => showCupertinoModalPopup<void>(
    context: context,
    builder: (context) => CupertinoActionSheet(
      title: Text(savedName(item)),
      actions: [
        for (final choice in _choices(item))
          CupertinoActionSheetAction(
            isDestructiveAction: choice.destructive,
            onPressed: () {
              Navigator.of(context).pop();
              choice.onPressed();
            },
            child: Text(choice.label),
          ),
      ],
      cancelButton: CupertinoActionSheetAction(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
    ),
  );

  Future<void> _editDetails(SavedItem item) async {
    final fields =
        await showFormDialog<({String name, String note, String? emoji})>(
          context,
          EditItemDialog(
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
            editsEmoji: item is SavedPlace,
            emoji: item is SavedPlace ? item.emoji : null,
          ),
        );
    if (fields != null) {
      await _store.edit(
        item,
        name: fields.name,
        note: fields.note,
        emoji: fields.emoji,
      );
    }
  }

  /// Deletes [these], or else the selection.
  Future<void> _delete([List<SavedItem>? these]) async {
    final items = these ?? _selection;
    final undo = await _store.delete(items);
    _message(
      'Deleted ${items.length} ${items.length == 1 ? 'item' : 'items'}',
      undo: undo,
    );
  }

  /// A row's swiped away while selecting: its row goes at once, as the swipe has it,
  /// and the item after it, with Undo.
  void _swipedAway(SavedItem item) {
    setState(() {
      _selected.remove(item.key);
      _lists = [
        for (final list in _lists)
          if (list.key != item.key) list,
      ];
      _places = [
        for (final place in _places)
          if (place.key != item.key) place,
      ];
    });
    unawaited(_delete([item]));
  }

  /// One of a row's ⋮ actions, on Android.
  void _rowAction(SavedItem item, _RowAction action) {
    switch (action) {
      case _RowAction.edit:
        unawaited(_editDetails(item));
      case _RowAction.pin:
        if (item is SavedPlace) unawaited(_togglePinned(item));
      case _RowAction.move:
        unawaited(_move([item]));
      case _RowAction.copy:
        if (item is SavedPlace) unawaited(_copy([item]));
      case _RowAction.delete:
        // Deleting goes by way of selecting, rare as it is.
        _select(item);
    }
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
    final selection = _selection;
    final places = selection.whereType<SavedPlace>().toList();
    final cupertino = isCupertinoOf(context);
    final onlyPlaces =
        selection.isNotEmpty && places.length == selection.length;
    final actions = (
      move: selection.isEmpty ? null : _move,
      copy: onlyPlaces ? _copy : null,
      pin: onlyPlaces ? _pinSelection : null,
      unpins: onlyPlaces && places.every((place) => place.pinned),
      delete: selection.isEmpty ? null : _delete,
    );
    return PopScope(
      canPop: widget.listId != null && !_editing,
      onPopInvokedWithResult: _onPop,
      child: SlidableAutoCloseBehavior(
        child: Stack(
          children: [
            SheetContent(
              // On iOS the selection's toolbar takes the bar's place.
              showsBottomBar: !(_editing && cupertino),
              // Not while selecting: on iOS the bar that would bring it back is away.
              collapsible: !_editing,
              // The same height going into a list and back out of it.
              keepsHeight: true,
              anchors: const [SheetAnchor.fraction(0.4), SheetAnchor.full()],
              freeRanges: const [
                SheetRange(SheetAnchor.fraction(0.4), SheetAnchor.full()),
              ],
              slivers: [
                PinnedHeaderSliver(
                  child: SheetBuilder(
                    // Clear over glass while the sheet's low; the sheet's color once
                    // rows scroll under it.
                    builder: (context, sheet, child) =>
                        ColoredBox(color: sheet.surfaceColor, child: child),
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
                            actions: actions,
                            onEdit: selection.length == 1
                                ? () => _editDetails(selection.single)
                                : null,
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
                            onBack: widget.listId == null
                                ? null
                                : () => _goUp(1),
                            onNewList: _newList,
                            onSelect: _items.isEmpty ? null : _select,
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
                if (_editing && cupertino)
                  const SliverToBoxAdapter(
                    child: SizedBox(height: _actionBarHeight + 16),
                  ),
              ],
            ),
            // Android's actions are along the top, in the selection's header; iOS's
            // along the bottom of the screen, where the bar was, however high the
            // sheet is.
            if (_editing && cupertino)
              SheetBuilder(
                builder: (context, sheet, child) => Stack(
                  children: [
                    PositionedDirectional(
                      start: 8,
                      end: 8,
                      top:
                          sheet.geometry.contentHeight(sheet.extent) -
                          sheet.geometry.padding.bottom -
                          8 -
                          _actionBarHeight,
                      height: _actionBarHeight,
                      child: child!,
                    ),
                  ],
                ),
                child: _Toolbar(actions: actions),
              ),
          ],
        ),
      ),
    );
  }

  Widget _row(SavedItem item, int index) {
    final cupertino = isCupertinoOf(context);
    Widget row = _Row(
      item: item,
      index: index,
      editing: _editing,
      selected: _selected.contains(item.key),
      count: item is SavedList ? _tree?.count(item.id) ?? 0 : 0,
      onTap: () => _editing ? _toggle(item) : _openItem(item),
      onLongPress: cupertino
          ? null
          : () => _editing ? _toggle(item) : _select(item),
      onAction: cupertino || _editing ? null : (a) => _rowAction(item, a),
    );
    if (cupertino) {
      if (!_editing) row = _ContextMenu(choices: _choices(item), child: row);
      row = _iosSwipes(item, row);
    } else if (_editing) {
      // Swiped away only while selecting: browsing, a swipe's too easily made.
      row = Dismissible(
        key: ValueKey(('dismiss', item.key)),
        background: const _SwipeToDelete(
          alignment: AlignmentDirectional.centerStart,
        ),
        secondaryBackground: const _SwipeToDelete(
          alignment: AlignmentDirectional.centerEnd,
        ),
        onDismissed: (_) => _swipedAway(item),
        child: row,
      );
    }
    return KeyedSubtree(key: ValueKey(item.key), child: row);
  }

  /// iOS's swipe actions. Browsing, left for More and Delete, and right on a place
  /// for Pin; selecting, left for Delete alone. All the way, a swipe does the action
  /// at its edge.
  Widget _iosSwipes(SavedItem item, Widget row) {
    final delete = _SwipeAction(
      label: 'Delete',
      icon: CupertinoIcons.trash,
      color: CupertinoColors.systemRed,
      onPressed: () => _swipedAway(item),
    );
    final place = item is SavedPlace ? item : null;
    return Slidable(
      key: ValueKey(('slide', item.key)),
      groupTag: 'saved',
      startActionPane: _editing || place == null
          ? null
          : ActionPane(
              motion: const DrawerMotion(),
              extentRatio: 0.25,
              dismissible: DismissiblePane(
                // Pinned, the row stays.
                confirmDismiss: () async {
                  unawaited(_togglePinned(place));
                  return false;
                },
                closeOnCancel: true,
                onDismissed: () {},
              ),
              children: [
                _SwipeAction(
                  label: place.pinned ? 'Unpin' : 'Pin',
                  icon: place.pinned
                      ? CupertinoIcons.pin_slash_fill
                      : CupertinoIcons.pin_fill,
                  color: CupertinoColors.systemOrange,
                  onPressed: () => unawaited(_togglePinned(place)),
                ),
              ],
            ),
      endActionPane: ActionPane(
        motion: const DrawerMotion(),
        extentRatio: _editing ? 0.25 : 0.5,
        dismissible: DismissiblePane(onDismissed: () => _swipedAway(item)),
        children: [
          if (!_editing)
            _SwipeAction(
              label: 'More',
              icon: CupertinoIcons.ellipsis_circle_fill,
              color: CupertinoColors.systemGrey,
              onPressed: () => unawaited(_more(item)),
            ),
          delete,
        ],
      ),
      child: row,
    );
  }

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
    required this.onBack,
    required this.onNewList,
    required this.onSelect,
  });

  final String title;

  /// The lists above this one, from the top level down.
  final List<String> crumbs;

  /// A crumb's tapped: how many levels up it is.
  final void Function(int levels) onCrumb;

  /// Up a list, from a nested one; null at the top level.
  final VoidCallback? onBack;
  final VoidCallback onNewList;

  /// Selecting, null with nothing to select.
  final VoidCallback? onSelect;

  @override
  Widget build(BuildContext context) =>
      isCupertinoOf(context) ? _cupertino(context) : _material(context);

  Widget _material(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final back = onBack;
    return Padding(
      padding: EdgeInsetsDirectional.only(
        start: back == null ? 20 : 4,
        end: 8,
        top: 8,
        bottom: 8,
      ),
      child: Row(
        children: [
          if (back != null)
            IconButton(
              tooltip: 'Back',
              icon: const BackButtonIcon(),
              onPressed: back,
            ),
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
          PopupMenuButton<void>(
            tooltip: 'More options',
            // Over the whole app, not clipped to the sheet.
            useRootNavigator: true,
            itemBuilder: (context) => [
              PopupMenuItem(
                enabled: onSelect != null,
                onTap: onSelect,
                child: const Text('Select'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  /// iOS's: a back button named for the list above, over a large title, and ⋯.
  Widget _cupertino(BuildContext context) {
    final theme = Theme.of(context);
    final back = onBack;
    return Padding(
      padding: EdgeInsetsDirectional.only(
        start: back == null ? 20 : 8,
        end: 8,
        top: back == null ? 12 : 0,
        bottom: 8,
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                if (back != null && crumbs.isNotEmpty)
                  _BackButton(crumbs: crumbs, onBack: back, onUp: onCrumb),
                Padding(
                  padding: EdgeInsetsDirectional.only(
                    start: back == null ? 0 : 12,
                  ),
                  child: Text(
                    title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.headlineSmall?.copyWith(
                      fontWeight: FontWeight.bold,
                    ),
                  ),
                ),
              ],
            ),
          ),
          CupertinoMenuAnchor(
            useRootOverlay: true,
            menuChildren: [
              CupertinoMenuItem(
                trailing: const Icon(CupertinoIcons.checkmark_circle),
                onPressed: onSelect,
                child: const Text('Select'),
              ),
              CupertinoMenuItem(
                trailing: const Icon(CupertinoIcons.folder_badge_plus),
                onPressed: onNewList,
                child: const Text('New List'),
              ),
            ],
            builder: (context, controller, child) => Semantics(
              label: 'More options',
              button: true,
              excludeSemantics: true,
              child: CupertinoButton(
                padding: const EdgeInsets.all(8),
                onPressed: () =>
                    controller.isOpen ? controller.close() : controller.open(),
                child: const Icon(CupertinoIcons.ellipsis_circle),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// iOS's back button: the list above's name; long-pressed, a menu of every list above,
/// nearest first.
class _BackButton extends StatelessWidget {
  const _BackButton({
    required this.crumbs,
    required this.onBack,
    required this.onUp,
  });

  /// The lists above, from the top level down.
  final List<String> crumbs;
  final VoidCallback onBack;
  final void Function(int levels) onUp;

  @override
  Widget build(BuildContext context) => CupertinoMenuAnchor(
    useRootOverlay: true,
    menuChildren: [
      for (var levels = 1; levels <= crumbs.length; levels++)
        CupertinoMenuItem(
          onPressed: () => onUp(levels),
          child: Text(crumbs[crumbs.length - levels]),
        ),
    ],
    builder: (context, controller, child) => GestureDetector(
      onLongPress: controller.open,
      child: CupertinoButton(
        padding: const EdgeInsets.symmetric(vertical: 4),
        minimumSize: const Size(0, 32),
        onPressed: onBack,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(CupertinoIcons.chevron_back),
            Text(crumbs.last),
          ],
        ),
      ),
    ),
  );
}

/// The header while selecting: on Android, a contextual bar with the selection's
/// actions; on iOS, the count and Done, the actions being along the bottom.
class _EditingHeader extends StatelessWidget {
  const _EditingHeader({
    required this.count,
    required this.allSelected,
    required this.onSelectAll,
    required this.onClear,
    required this.onDone,
    required this.actions,
    required this.onEdit,
  });

  final int count;
  final bool allSelected;
  final VoidCallback onSelectAll;
  final VoidCallback onClear;
  final VoidCallback onDone;

  /// The selection's actions, null where they don't apply to it.
  final _Actions actions;

  /// Edits the one item selected; on Android, null unless there's one.
  final VoidCallback? onEdit;

  String get _title => count == 0 ? 'Select items' : '$count selected';

  @override
  Widget build(BuildContext context) =>
      isCupertinoOf(context) ? _cupertino(context) : _material(context);

  Widget _material(BuildContext context) => SizedBox(
    height: 64,
    child: Padding(
      padding: const EdgeInsetsDirectional.only(start: 4, end: 8),
      child: Row(
        children: [
          IconButton(
            tooltip: 'Done',
            icon: const Icon(Icons.close),
            onPressed: onDone,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              _title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.titleMedium,
            ),
          ),
          IconButton(
            tooltip: 'Move',
            icon: const Icon(Icons.drive_file_move_outlined),
            onPressed: actions.move,
          ),
          IconButton(
            tooltip: 'Delete',
            icon: const Icon(Icons.delete_outline),
            onPressed: actions.delete,
          ),
          PopupMenuButton<void>(
            tooltip: 'More options',
            useRootNavigator: true,
            itemBuilder: (context) => [
              PopupMenuItem(
                enabled: actions.copy != null,
                onTap: actions.copy,
                child: const Text('Copy'),
              ),
              PopupMenuItem(
                enabled: actions.pin != null,
                onTap: actions.pin,
                child: Text(actions.unpins ? 'Unpin' : 'Pin'),
              ),
              PopupMenuItem(
                enabled: onEdit != null,
                onTap: onEdit,
                child: const Text('Edit'),
              ),
              PopupMenuItem(
                onTap: allSelected ? onClear : onSelectAll,
                child: Text(allSelected ? 'Deselect all' : 'Select all'),
              ),
            ],
          ),
        ],
      ),
    ),
  );

  /// iOS's, as Files has it: Select All, the count, and Done.
  Widget _cupertino(BuildContext context) => SizedBox(
    height: 64,
    child: Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8),
      child: Row(
        children: [
          CupertinoButton(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            onPressed: allSelected ? onClear : onSelectAll,
            child: Text(allSelected ? 'Deselect All' : 'Select All'),
          ),
          Expanded(
            child: Text(
              _title,
              textAlign: TextAlign.center,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.titleMedium,
            ),
          ),
          CupertinoButton(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            onPressed: onDone,
            child: const Text(
              'Done',
              style: TextStyle(fontWeight: FontWeight.w600),
            ),
          ),
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

/// What a row's ⋮ menu offers, on Android.
enum _RowAction { edit, pin, move, copy, delete }

class _Row extends StatelessWidget {
  const _Row({
    required this.item,
    required this.index,
    required this.editing,
    required this.selected,
    required this.count,
    required this.onTap,
    required this.onLongPress,
    required this.onAction,
  });

  final SavedItem item;
  final int index;
  final bool editing;
  final bool selected;
  final int count;
  final VoidCallback onTap;
  final VoidCallback? onLongPress;

  /// One of the row's ⋮ actions; null for no ⋮ (iOS, or selecting).
  final ValueChanged<_RowAction>? onAction;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final cupertino = isCupertinoOf(context);
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
      // iOS marks a selected row by its circle alone.
      selectedTileColor: cupertino
          ? null
          : theme.colorScheme.secondaryContainer,
      leading: editing
          ? cupertino
                ? Icon(
                    selected
                        ? CupertinoIcons.checkmark_circle_fill
                        : CupertinoIcons.circle,
                    color: selected
                        ? theme.colorScheme.primary
                        : theme.colorScheme.outline,
                  )
                : Checkbox(
                    value: selected,
                    visualDensity: VisualDensity.compact,
                    onChanged: (_) => onTap(),
                  )
          : switch (item) {
              SavedPlace(emoji: final emoji?) => Text(
                emoji,
                style: const TextStyle(fontSize: 20),
              ),
              SavedList() => Icon(
                _icon(context, Icons.folder_outlined, CupertinoIcons.folder),
                color: cupertino ? theme.colorScheme.primary : null,
              ),
              SavedPlace(pinned: true) => Icon(
                _icon(context, Icons.push_pin, CupertinoIcons.pin_fill),
                color: cupertino ? theme.colorScheme.primary : null,
              ),
              SavedPlace() => Icon(
                _icon(context, Icons.place_outlined, CupertinoIcons.placemark),
                color: cupertino ? theme.colorScheme.primary : null,
              ),
            },
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
      // Selecting, the actions are the selection's.
      trailing: editing
          ? _handle(context)
          : onAction != null
          ? Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (item is SavedPlace && item.pinned && item.emoji != null)
                  const Icon(Icons.push_pin, size: 16),
                if (item is SavedList)
                  Text('$count', style: theme.textTheme.bodySmall),
                _menu(item),
              ],
            )
          // The emoji has taken the pin's place at the start of the row.
          : item is SavedPlace && item.pinned && item.emoji != null
          ? Icon(
              _icon(context, Icons.push_pin, CupertinoIcons.pin_fill),
              size: 16,
            )
          : item is SavedList
          ? Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text('$count', style: theme.textTheme.bodySmall),
                if (cupertino) const SizedBox(width: 4),
                Icon(
                  _icon(
                    context,
                    Icons.chevron_right,
                    CupertinoIcons.chevron_forward,
                  ),
                  size: cupertino ? 16 : null,
                  color: cupertino ? theme.colorScheme.outline : null,
                ),
              ],
            )
          : null,
      onTap: onTap,
      onLongPress: onLongPress,
    );
  }

  Widget _handle(BuildContext context) => ReorderableDragStartListener(
    index: index,
    child: Padding(
      padding: const EdgeInsets.all(8),
      child: Icon(
        _icon(context, Icons.drag_handle, CupertinoIcons.line_horizontal_3),
      ),
    ),
  );

  Widget _menu(SavedItem item) => PopupMenuButton<_RowAction>(
    tooltip: 'More options',
    useRootNavigator: true,
    onSelected: onAction,
    itemBuilder: (context) => [
      const PopupMenuItem(value: _RowAction.edit, child: Text('Edit')),
      if (item case SavedPlace(:final pinned))
        PopupMenuItem(
          value: _RowAction.pin,
          child: Text(pinned ? 'Unpin' : 'Pin'),
        ),
      const PopupMenuItem(value: _RowAction.move, child: Text('Move')),
      if (item is SavedPlace)
        const PopupMenuItem(value: _RowAction.copy, child: Text('Copy')),
      const PopupMenuItem(value: _RowAction.delete, child: Text('Delete')),
    ],
  );
}

/// What a row swiped away while selecting shows behind it.
class _SwipeToDelete extends StatelessWidget {
  const _SwipeToDelete({required this.alignment});

  final AlignmentGeometry alignment;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return ColoredBox(
      color: colors.errorContainer,
      child: Align(
        alignment: alignment,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24),
          child: Icon(Icons.delete_outline, color: colors.onErrorContainer),
        ),
      ),
    );
  }
}

/// What the selection's actions do, null where they don't apply to it.
typedef _Actions = ({
  VoidCallback? move,
  VoidCallback? copy,
  VoidCallback? pin,

  /// Whether Pin unpins: every place selected is pinned.
  bool unpins,
  VoidCallback? delete,
});

/// One of a row's choices on iOS, in its context menu or action sheet.
typedef _Choice = ({
  String label,
  IconData icon,
  bool destructive,
  VoidCallback onPressed,
});

/// iOS's toolbar of the selection's actions, floating on glass where the bottom bar
/// was.
class _Toolbar extends StatelessWidget {
  const _Toolbar({required this.actions});

  final _Actions actions;

  @override
  Widget build(BuildContext context) {
    Widget action(
      IconData icon,
      String label,
      VoidCallback? onPressed, {
      bool destructive = false,
    }) => Expanded(
      child: TextButton(
        onPressed: onPressed,
        style: ButtonStyle(
          padding: const WidgetStatePropertyAll(EdgeInsets.zero),
          // iOS colors what deletes red.
          foregroundColor: destructive
              ? WidgetStateProperty.resolveWith(
                  (states) => states.contains(WidgetState.disabled)
                      ? null
                      : Theme.of(context).colorScheme.error,
                )
              : null,
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
    return AppGlassLayer(
      child: GlassSurface(
        radius: 28,
        floating: true,
        child: Row(
          children: [
            action(CupertinoIcons.folder, 'Move', actions.move),
            action(CupertinoIcons.doc_on_doc, 'Copy', actions.copy),
            action(
              actions.unpins ? CupertinoIcons.pin_slash : CupertinoIcons.pin,
              actions.unpins ? 'Unpin' : 'Pin',
              actions.pin,
            ),
            action(
              CupertinoIcons.trash,
              'Delete',
              actions.delete,
              destructive: true,
            ),
          ],
        ),
      ),
    );
  }
}

/// A row's long-press menu on iOS: the row lifts, with its choices under it.
class _ContextMenu extends StatelessWidget {
  const _ContextMenu({required this.choices, required this.child});

  final List<_Choice> choices;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.surfaceContainerLow;
    return CupertinoContextMenu.builder(
      actions: [
        for (final choice in choices)
          Builder(
            builder: (context) => CupertinoContextMenuAction(
              isDestructiveAction: choice.destructive,
              trailingIcon: choice.icon,
              onPressed: () {
                Navigator.of(context).pop();
                choice.onPressed();
              },
              child: Text(choice.label),
            ),
          ),
      ],
      // Lifted, on the sheet's color with rounded corners; in the list, as it was.
      builder: (context, animation) {
        final lift = (animation.value / CupertinoContextMenu.animationOpensAt)
            .clamp(0.0, 1.0);
        return Material(
          color: Color.lerp(color.withValues(alpha: 0), color, lift),
          borderRadius: BorderRadius.circular(12 * lift),
          clipBehavior: Clip.antiAlias,
          child: child,
        );
      },
    );
  }
}

/// One of iOS's swipe actions: an icon over its label, on [color].
class _SwipeAction extends StatelessWidget {
  const _SwipeAction({
    required this.label,
    required this.icon,
    required this.color,
    required this.onPressed,
  });

  final String label;
  final IconData icon;
  final Color color;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) => CustomSlidableAction(
    backgroundColor: color,
    foregroundColor: const Color(0xFFFFFFFF),
    padding: EdgeInsets.zero,
    onPressed: (_) => onPressed(),
    child: Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(icon, color: const Color(0xFFFFFFFF)),
          const SizedBox(height: 4),
          Text(
            label,
            maxLines: 1,
            style: Theme.of(context).textTheme.labelMedium
                ?.copyWith(color: const Color(0xFFFFFFFF)),
          ),
        ],
      ),
    ),
  );
}

/// [cupertino] on iOS, [material] elsewhere.
IconData _icon(BuildContext context, IconData material, IconData cupertino) =>
    isCupertinoOf(context) ? cupertino : material;
