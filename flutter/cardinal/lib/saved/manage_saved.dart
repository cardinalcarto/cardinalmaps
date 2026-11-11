import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoIcons;
import 'package:material_ui/material_ui.dart';

import '../store/store.dart';
import '../ui/adaptive.dart';
import 'destination_picker.dart';
import 'saved_names.dart';

/// Asks which lists the place [placeId] should be in: where it is now, ticked, and any
/// more it's added to. Returns the lists to have it in, null in the set for the top
/// level and an empty set for nowhere, or null if dismissed.
Future<Set<int?>?> showManageSaved(
  BuildContext context, {
  required String placeId,
}) => showAppSheet<Set<int?>>(
  context: context,
  builder: (context) => _ManageSaved(placeId: placeId),
);

class _ManageSaved extends StatefulWidget {
  const _ManageSaved({required this.placeId});

  final String placeId;

  @override
  State<_ManageSaved> createState() => _ManageSavedState();
}

class _ManageSavedState extends State<_ManageSaved> {
  SavedTree? _tree;
  StreamSubscription<SavedTree>? _watch;

  /// The lists with a row, ticked or not; null until the place's lists are known.
  List<int?>? _shown;
  final _ticked = <int?>{};

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _watch ??= StoreScope.of(context).saved.watch().listen((tree) {
      setState(() {
        _tree = tree;
        if (_shown == null) {
          _shown = [
            for (final copy in tree.copiesOf(widget.placeId)) copy.parent,
          ];
          _ticked.addAll(_shown!);
        }
      });
    });
  }

  @override
  void dispose() {
    _watch?.cancel();
    super.dispose();
  }

  void _toggle(int? list) => setState(() {
    if (!_ticked.remove(list)) _ticked.add(list);
  });

  Future<void> _add(SavedTree tree) async {
    final copies = tree.copiesOf(widget.placeId);
    if (copies.isEmpty) return;
    final picked = await showDestinationPicker(
      context,
      items: [copies.first],
      unavailable: _shown!.toSet(),
      verb: 'Add',
    );
    if (picked == null || !mounted) return;
    setState(() {
      _shown!.add(picked.list);
      _ticked.add(picked.list);
    });
  }

  @override
  Widget build(BuildContext context) {
    final tree = _tree;
    final shown = _shown;
    final cupertino = isCupertinoOf(context);
    final accent = Theme.of(context).colorScheme.primary;
    final done = shown == null
        ? null
        : () => Navigator.of(context).pop(_ticked);
    return SafeArea(
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxHeight: MediaQuery.sizeOf(context).height * 0.7,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            AppSheetBar(
              title: 'Saved in',
              onCancel: () => Navigator.of(context).pop(),
              onDone: done,
            ),
            Flexible(
              child: tree == null || shown == null
                  ? const SizedBox(height: 96)
                  : cupertino
                  ? ListView(
                      shrinkWrap: true,
                      children: [
                        AppListSection(
                          children: [
                            for (final list in shown)
                              AppListTile(
                                title: Text(
                                  locationPath(tree, list),
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                ),
                                trailing: _ticked.contains(list)
                                    ? Icon(
                                        CupertinoIcons.checkmark,
                                        color: accent,
                                      )
                                    : null,
                                onTap: () => _toggle(list),
                              ),
                            AppListTile(
                              leading: Icon(CupertinoIcons.add, color: accent),
                              title: Text(
                                'Add to list…',
                                style: TextStyle(color: accent),
                              ),
                              onTap: () => _add(tree),
                            ),
                          ],
                        ),
                      ],
                    )
                  : ListView(
                      shrinkWrap: true,
                      children: [
                        for (final list in shown)
                          CheckboxListTile(
                            dense: true,
                            visualDensity: VisualDensity.compact,
                            controlAffinity: ListTileControlAffinity.leading,
                            contentPadding: const EdgeInsetsDirectional.only(
                              start: 8,
                              end: 16,
                            ),
                            value: _ticked.contains(list),
                            onChanged: (_) => _toggle(list),
                            title: Text(
                              locationPath(tree, list),
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                        ListTile(
                          dense: true,
                          visualDensity: VisualDensity.compact,
                          contentPadding: const EdgeInsets.symmetric(
                            horizontal: 20,
                          ),
                          minLeadingWidth: 24,
                          horizontalTitleGap: 20,
                          leading: const Icon(Icons.add),
                          title: const Text('Add to list…'),
                          onTap: () => _add(tree),
                        ),
                      ],
                    ),
            ),
            // On iOS, Done is in the bar at the top.
            if (!cupertino)
              Padding(
                padding: const EdgeInsets.fromLTRB(8, 4, 8, 8),
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  children: [
                    FilledButton(onPressed: done, child: const Text('Done')),
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }
}
