import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoActivityIndicator, CupertinoIcons;
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../geocoder/address.dart';
import '../geocoder/geocoder.dart';
import '../map/map.dart';
import '../map/selection.dart';
import '../saved/destination_picker.dart';
import '../saved/manage_saved.dart';
import '../saved/saved_feedback.dart';
import '../saved/saved_names.dart';
import '../routes.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
import '../ui/symbols.dart';

/// A place's card: its name and where it is, with its pin (and boundary) on the map.
///
/// Reads the place from the store by [id]; if it isn't there (a restored or linked
/// route), fetches it from the geocoder. The map flies to show it in what the sheet
/// leaves uncovered, following the sheet as it settles and moves.
class PlacePage extends StatefulWidget {
  const PlacePage({super.key, required this.id});

  final String id;

  @override
  State<PlacePage> createState() => _PlacePageState();
}

class _PlacePageState extends State<PlacePage> with RouteAware {
  static const _header = #placeHeader;

  Place? _place;
  bool _missing = false;
  bool _fetched = false;

  /// Whether the map has been sent to the place.
  bool _focused = false;
  StreamSubscription<Place?>? _watch;
  StreamSubscription<SavedTree>? _watchSaved;
  late SparkleMapController _map;

  SavedTree? _savedTree;
  List<SavedPlace> get _copies => _savedTree?.copiesOf(widget.id) ?? const [];

  /// This page's route animation: once it has come all the way in, the page is leaving
  /// whenever it's less than all the way in (closing, or a back gesture under way).
  Animation<double>? _route;
  bool _arrived = false;
  bool _leaving = false;

  /// Whether the map was keeping the place in view when the page started leaving, to
  /// keep it in view again if the page stays after all.
  bool _hadSubject = false;

  /// Whether the app is in dark mode, which has a pin of its own.
  bool _dark = false;

  /// Whether directions are open over this card, drawing the map's trip themselves.
  bool _covered = false;

  ModalRoute<Object?>? _modalRoute;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _map = MapScope.of(context);
    final dark = Theme.of(context).brightness == Brightness.dark;
    if (dark != _dark) {
      _dark = dark;
      if (_place != null && !_covered) showSelection(_map, _place, dark: dark);
    }
    if (_watch == null) {
      final places = StoreScope.of(context).places;
      _place = places.cached(widget.id);
      _watch = places.watch(widget.id).listen(_changed);
      if (_place != null) _show();
    }
    _watchSaved ??= StoreScope.of(context).saved.watch().listen((tree) {
      setState(() => _savedTree = tree);
    });
    final modalRoute = ModalRoute.of(context);
    if (modalRoute != _modalRoute) {
      sheetPages.unsubscribe(this);
      if (modalRoute is PageRoute<Object?>) {
        sheetPages.subscribe(this, modalRoute);
      }
      _modalRoute = modalRoute;
    }
    final route = modalRoute?.animation;
    if (route != _route) {
      _route?.removeListener(_routeMoved);
      // Only from its changes: before its transition starts, it may say all the way in.
      _route = route?..addListener(_routeMoved);
    }
  }

  @override
  void dispose() {
    sheetPages.unsubscribe(this);
    _watch?.cancel();
    _watchSaved?.cancel();
    _route?.removeListener(_routeMoved);
    clearSelection(_map, widget.id);
    super.dispose();
  }

  void _changed(Place? place) {
    if (place == null) {
      _fetch();
      return;
    }
    if (place == _place && !_missing) return;
    setState(() {
      _place = place;
      _missing = false;
    });
    _show();
  }

  @override
  void didPushNext() => _covered = true;

  /// Directions closed: the place is what the map shows again.
  @override
  void didPopNext() {
    _covered = false;
    final place = _place;
    showSelection(_map, place, dark: _dark);
    if (place != null) focusPlace(_map, place);
  }

  /// Fetch the place from the geocoder, once, into the store.
  Future<void> _fetch() async {
    if (_fetched) return;
    _fetched = true;
    final store = StoreScope.of(context);
    final places = store.places;
    final geocoders = GeocoderScope.of(context);
    try {
      // From where search goes: the place's ID is the one search found it by. Only the
      // device has its own IDs.
      final online =
          await store.settings.get(searchOnline) && !isLocalPlaceId(widget.id);
      final place = await geocoders.chosen(online: online).place(widget.id);
      if (place != null) {
        await places.put(place);
        return;
      }
    } on Object catch (e) {
      debugPrint('fetching ${widget.id}: $e');
    }
    if (mounted && _place == null) setState(() => _missing = true);
  }

  /// Show the place on the map, and the first time, send the map to it. The map keeps
  /// it in what the sheet leaves uncovered from then on, as the sheet settles here and
  /// moves, until it's moved by hand.
  void _show() {
    // Directions over the card draw the map's trip themselves: a second pin here would
    // mark a place that may no longer be the destination.
    if (_covered) return;
    final place = _place;
    showSelection(_map, place, dark: _dark);
    if (place == null || _focused) return;
    _focused = true;
    focusPlace(_map, place);
  }

  /// As the page leaves, let the camera go, so the sheet growing back to the page
  /// below's height doesn't zoom the map out to keep the place in view. If it comes
  /// back (a back gesture let go of), keep the place in view again, unless the map had
  /// been moved by hand.
  void _routeMoved() {
    final shown = (_route?.value ?? 1) >= 1;
    if (!_arrived) {
      _arrived = shown;
    } else if (!shown && !_leaving) {
      _leaving = true;
      _hadSubject = _map.hasSubject;
      _map.clearSubject();
    } else if (shown && _leaving) {
      _leaving = false;
      final place = _place;
      if (_hadSubject && place != null) focusPlace(_map, place);
    }
  }

  /// Saves the place to the top of Saved, offering to file it in a list.
  Future<void> _save() async {
    final saved = StoreScope.of(context).saved;
    await saved.savePlace(widget.id);
    if (!mounted) return;
    showSheetMessage(context, 'Saved', action: 'Move…', onAction: _moveSaved);
  }

  /// Moves the copy just saved at the top level.
  Future<void> _moveSaved() async {
    final saved = StoreScope.of(context).saved;
    final item = _copies.where((copy) => copy.parent == null).firstOrNull;
    if (item == null) return;
    final picked = await showDestinationPicker(
      context,
      items: [item],
      unavailable: {item.parent},
    );
    if (picked == null || !mounted) return;
    await saved.move([item], picked.list);
  }

  /// Asks which lists the place should be in, and puts it there, with Undo.
  Future<void> _manage() async {
    final saved = StoreScope.of(context).saved;
    final before = {for (final copy in _copies) copy.parent};
    final after = await showManageSaved(context, placeId: widget.id);
    if (after == null || !mounted) return;
    final undo = await saved.setLocations(widget.id, after);
    final added = after.difference(before);
    final removed = before.difference(after);
    if (!mounted || (added.isEmpty && removed.isEmpty)) return;
    final tree = await saved.watch().first;
    if (!mounted) return;
    showSheetMessage(
      context,
      switch ((added.toList(), removed.toList())) {
        _ when after.isEmpty => 'Removed from Saved',
        ([final one], []) => 'Added to ${destinationName(tree, one)}',
        ([], [final one]) => 'Removed from ${destinationName(tree, one)}',
        _ => 'Updated where it\'s saved',
      },
      action: 'Undo',
      onAction: () => unawaited(saved.restore(undo)),
    );
  }

  /// The place's actions: sharing a row equally on iOS, as Apple's place cards do;
  /// wrapped at their own widths elsewhere.
  Widget _actions(bool cupertino, List<Widget> actions) => cupertino
      ? Row(
          children: [
            for (final (i, action) in actions.indexed) ...[
              if (i > 0) const SizedBox(width: 8),
              Expanded(child: action),
            ],
          ],
        )
      : Wrap(spacing: 8, runSpacing: 4, children: actions);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final cupertino = isCupertinoOf(context);
    final symbols = AppSymbols.of(context);
    final place = _place;
    final isSaved = _copies.isNotEmpty;
    final subtitle = place == null ? null : placeSubtitle(place);
    final actions = _actions(cupertino, [
      AppActionButton(
        kind: AppButtonKind.filled,
        icon: cupertino ? CupertinoIcons.arrow_turn_up_right : Icons.directions,
        label: 'Directions',
        onPressed: () => context.push<void>(directionsLocation(widget.id)),
      ),
      if (_savedTree != null)
        AppActionButton(
          icon: isSaved ? symbols.saved : symbols.save,
          label: isSaved ? 'Manage' : 'Save',
          onPressed: isSaved ? _manage : _save,
        ),
    ]);
    final header = Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                place != null
                    ? placeTitle(place)
                    : _missing
                    ? 'Place not found'
                    : '',
                style: theme.textTheme.headlineSmall,
              ),
              if (subtitle != null)
                Padding(
                  padding: const EdgeInsets.only(top: 4),
                  child: Text(
                    subtitle,
                    style: theme.textTheme.bodyMedium?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ),
              if (place == null && !_missing)
                Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: cupertino
                      ? const CupertinoActivityIndicator()
                      : const LinearProgressIndicator(),
                ),
              // Elsewhere the actions sit under the address, beside the close button.
              if (place != null && !cupertino)
                Padding(padding: const EdgeInsets.only(top: 8), child: actions),
            ],
          ),
        ),
        AppCloseButton(
          onPressed: () => context.canPop() ? context.pop() : context.go('/'),
        ),
      ],
    );
    return SheetContent(
      anchors: const [SheetAnchor.section(_header)],
      slivers: [
        SliverToBoxAdapter(
          child: SheetSection(
            id: _header,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 4, 8, 16),
              child: cupertino && place != null
                  // On iOS the actions are a row across the card, under its title.
                  ? Column(
                      children: [
                        header,
                        Padding(
                          padding: const EdgeInsets.fromLTRB(0, 16, 8, 0),
                          child: actions,
                        ),
                      ],
                    )
                  : header,
            ),
          ),
        ),
      ],
    );
  }
}
