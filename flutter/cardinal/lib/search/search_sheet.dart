import 'dart:async';
import 'dart:math' as math;

import 'dart:ui' show PlatformDispatcher;

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../geocoder/address.dart';
import '../geocoder/geocoder.dart';
import '../map/map.dart';
import '../routes.dart';
import '../saved/saved_names.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import 'place_search.dart';

/// The home sheet: a search bar, peeking over the map, with results below it as you type.
///
/// It's browsing or searching, a mode it keeps itself rather than reading it off the
/// bar's focus, which moves of its own accord (the map takes it when it's pressed):
///
/// | In | When | Then |
/// |---|---|---|
/// | browsing: at the bar, no keyboard | the bar's tapped | searching: full, keyboard up |
/// | searching | dragged down | searching, at the bar, keyboard as it was |
/// | searching | the bar's tapped | searching: full, keyboard up |
/// | searching | back | browsing; the query and results wait for the next tap |
/// | browsing | back | the app's left |
/// | either | a page covers it | it notes its mode, anchor and keyboard, and lets go of focus |
/// | either | that page closes, however | as it noted; the keyboard as soon as the page can take focus |
/// | either | the app bar's Search button is tapped | searching: full, keyboard up |
///
/// The bar pins to the top of the sheet, with the results scrolling under it; tapping one
/// opens its place. Its page doesn't take focus when a page over it closes (it's a
/// [SheetPage] with `requestFocus: false`), which would hand the bar back focus it had
/// before the map took it: what comes back is only what it noted.
class SearchSheet extends StatefulWidget {
  const SearchSheet({super.key});

  @override
  State<SearchSheet> createState() => _SearchSheetState();
}

class _SearchSheetState extends State<SearchSheet> with RouteAware {
  static const _bar = #searchBar;

  final _text = TextEditingController();
  final _focus = FocusNode(debugLabel: 'search');
  PlaceSearch? _search;

  /// The pinned places, each with a chip under the bar while browsing.
  List<SavedPlace> _pinned = const [];
  StreamSubscription<SavedTree>? _watchSaved;

  /// Whether the sheet is searching; see [SearchSheet].
  bool _searching = false;

  /// What the sheet was when a page covered it, to be again when that page closes.
  ({bool searching, SheetAnchor anchor, bool keyboard})? _covered;

  SheetController? _sheet;
  ModalRoute<Object?>? _route;

  @override
  void initState() {
    super.initState();
    _focus.addListener(_focusChanged);
    searchRequests.addListener(_searchRequested);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _sheet = SheetScaffold.controllerOf(context);
    final route = ModalRoute.of(context);
    if (route != _route) {
      sheetPages.unsubscribe(this);
      if (route is PageRoute<Object?>) sheetPages.subscribe(this, route);
      _route = route;
    }
    _watchSaved ??= StoreScope.of(context).saved.watch().listen((tree) {
      setState(() => _pinned = tree.pinned);
    });
    _search ??= PlaceSearch(
      GeocoderScope.of(context),
      lang: PlatformDispatcher.instance.locale.toLanguageTag(),
    );
  }

  @override
  void dispose() {
    sheetPages.unsubscribe(this);
    searchRequests.removeListener(_searchRequested);
    _watchSaved?.cancel();
    _focus.dispose();
    _text.dispose();
    _search?.dispose();
    super.dispose();
  }

  /// Focus however it comes, a keyboard's Tab key say, means searching; only a tap on
  /// the bar pulls the sheet up as well.
  void _focusChanged() {
    if (_focus.hasFocus && !_searching) setState(() => _searching = true);
  }

  /// The bar's tapped: searching, full, the keyboard up.
  void _searchFull() {
    if (!_searching) setState(() => _searching = true);
    _sampleMap();
    _sheet!.animateTo(const SheetAnchor.full());
  }

  /// The app bar's Search button, tapped on this page: the same as tapping the bar.
  void _searchRequested() {
    _focus.requestFocus();
    _searchFull();
  }

  /// Back while searching: browsing, keeping the query and results for next time.
  void _stopSearching() {
    if (!_searching) return;
    setState(() => _searching = false);
    // Letting go of focus, rather than only losing it, keeps the page from giving it
    // back to the bar later.
    _focus.unfocus();
    _sheet!.animateTo(const SheetAnchor.section(_bar));
  }

  @override
  void didPushNext() {
    final sheet = _sheet!;
    _covered = (
      searching: _searching,
      anchor:
          sheet.restingAt ??
          (sheet.isAtMax
              ? const SheetAnchor.full()
              : const SheetAnchor.section(_bar)),
      keyboard: _focus.hasFocus,
    );
    _focus.unfocus();
  }

  @override
  void didPopNext() {
    final covered = _covered;
    _covered = null;
    if (covered == null) return;
    setState(() => _searching = covered.searching);
    _sheet!.animateTo(covered.anchor);
    if (covered.keyboard) _focusWhenAllowed();
  }

  /// Focuses the bar as soon as its page may take focus: no page may during a back
  /// gesture, which lasts until its transition is over, after the pop.
  void _focusWhenAllowed() {
    final gesture = Navigator.of(context).userGestureInProgressNotifier;
    if (!gesture.value) {
      _focus.requestFocus();
      return;
    }
    void ended() {
      if (gesture.value) return;
      gesture.removeListener(ended);
      // The page lets focus in again as it rebuilds for the gesture's end.
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted &&
            _covered == null &&
            (ModalRoute.of(context)?.isCurrent ?? false)) {
          _focus.requestFocus();
        }
      });
      WidgetsBinding.instance.scheduleFrame();
    }

    gesture.addListener(ended);
  }

  /// Results near what the map shows rank higher: what the sheet leaves uncovered now,
  /// before the sheet covers it. If it covers everything already, what was last seen.
  void _sampleMap() {
    final view = SheetScaffold.viewOf(context);
    final clear =
        EdgeInsets.only(top: MediaQuery.paddingOf(context).top) + view.obscured;
    final area = clear.deflateRect(Offset.zero & MediaQuery.sizeOf(context));
    if (area.isEmpty) return;
    final map = MapScope.of(context);
    if (map.screenToLngLat(area.center) case final p?) {
      _search!.focus = LngLat(p.lng, p.lat);
    }
    // The bounds of its corners, as the map may be turned.
    final corners = [
      area.topLeft,
      area.topRight,
      area.bottomLeft,
      area.bottomRight,
    ].map(map.screenToLngLat).nonNulls.toList();
    _search!.viewport = corners.length < 4
        ? null
        : (
            west: corners.map((c) => c.lng).reduce(math.min),
            south: corners.map((c) => c.lat).reduce(math.min),
            east: corners.map((c) => c.lng).reduce(math.max),
            north: corners.map((c) => c.lat).reduce(math.max),
          );
  }

  Future<void> _open(Place place) async {
    await StoreScope.of(context).places.put(place);
    if (!mounted) return;
    await context.push<void>(placeLocation(place.id));
  }

  /// Whether the pinned places' chips show, under the bar while browsing.
  bool get _chipsShowing => !_searching && _pinned.isNotEmpty;

  @override
  Widget build(BuildContext context) {
    final search = _search!;
    return PopScope(
      // Back stops searching before it leaves the app.
      canPop: !_searching,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _stopSearching();
      },
      child: ListenableBuilder(
        listenable: search,
        builder: (context, _) => SheetContent(
          showsBottomBar: true,
          // Just the bar, until searching.
          anchors: _searching
              ? const [SheetAnchor.section(_bar), SheetAnchor.full()]
              : const [SheetAnchor.section(_bar)],
          slivers: [
            SliverPersistentHeader(
              pinned: true,
              delegate: _SearchBarHeader(
                color: Theme.of(context).colorScheme.surfaceContainerLow,
                height: _chipsShowing
                    ? _SearchBarHeader.heightWithChips
                    : _SearchBarHeader.barHeight,
                child: SheetSection(
                  id: _bar,
                  child: Column(
                    children: [
                      Padding(
                        padding: EdgeInsets.fromLTRB(
                          16,
                          4,
                          16,
                          _chipsShowing ? 8 : 12,
                        ),
                        child: SearchBar(
                          controller: _text,
                          focusNode: _focus,
                          onTap: _searchFull,
                          hintText: 'Search',
                          elevation: const WidgetStatePropertyAll(0),
                          leading: const Padding(
                            padding: EdgeInsets.symmetric(horizontal: 8),
                            child: Icon(Icons.search),
                          ),
                          trailing: [
                            ListenableBuilder(
                              listenable: _text,
                              builder: (context, _) => _text.text.isEmpty
                                  ? const SizedBox.shrink()
                                  : IconButton(
                                      tooltip: 'Clear',
                                      icon: const Icon(Icons.close),
                                      onPressed: () {
                                        _text.clear();
                                        search.update('');
                                        // Ready to type again, if searching; otherwise
                                        // clearing it is all.
                                        if (_searching) _focus.requestFocus();
                                      },
                                    ),
                            ),
                          ],
                          onChanged: search.update,
                          onSubmitted: (_) {
                            if (search.results case [final first, ...]) {
                              _open(first);
                            }
                          },
                        ),
                      ),
                      if (_chipsShowing) _PinnedChips(pinned: _pinned),
                    ],
                  ),
                ),
              ),
            ),
            ListenableBuilder(
              listenable: search,
              builder: (context, _) => _Results(search: search, onTap: _open),
            ),
          ],
        ),
      ),
    );
  }
}

/// The search bar, pinned to the top of the sheet's content.
class _SearchBarHeader extends SliverPersistentHeaderDelegate {
  _SearchBarHeader({
    required this.color,
    required this.height,
    required this.child,
  });

  final Color color;
  final double height;
  final Widget child;

  /// The bar and its padding.
  static const double barHeight = 72;

  /// The bar, its padding, and a row of pinned places.
  static const double heightWithChips = 112;

  @override
  double get minExtent => height;

  @override
  double get maxExtent => height;

  @override
  Widget build(
    BuildContext context,
    double shrinkOffset,
    bool overlapsContent,
  ) => ColoredBox(color: color, child: child);

  @override
  bool shouldRebuild(_SearchBarHeader oldDelegate) =>
      color != oldDelegate.color ||
      height != oldDelegate.height ||
      child != oldDelegate.child;
}

/// A chip for each pinned place, which opens it.
class _PinnedChips extends StatelessWidget {
  const _PinnedChips({required this.pinned});

  final List<SavedPlace> pinned;

  @override
  Widget build(BuildContext context) => SizedBox(
    height: 36,
    child: ListView.separated(
      scrollDirection: Axis.horizontal,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      itemCount: pinned.length,
      separatorBuilder: (context, _) => const SizedBox(width: 8),
      itemBuilder: (context, i) => FilledButton.tonal(
        style: const ButtonStyle(visualDensity: VisualDensity.compact),
        onPressed: () => context.push<void>(placeLocation(pinned[i].id)),
        child: Text(
          savedName(pinned[i]),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
      ),
    ),
  );
}

class _Results extends StatelessWidget {
  const _Results({required this.search, required this.onTap});

  final PlaceSearch search;
  final void Function(Place place) onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final results = search.results;
    final message = switch (search) {
      PlaceSearch(error: final error?) => 'Search failed: $error',
      PlaceSearch(loading: false, results: [], text: final text)
          when text.trim().isNotEmpty =>
        'No results',
      _ => null,
    };
    return SliverMainAxisGroup(
      slivers: [
        SliverToBoxAdapter(
          child: SizedBox(
            height: 2,
            child: search.loading ? const LinearProgressIndicator() : null,
          ),
        ),
        if (message != null)
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Text(
                message,
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyMedium?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ),
          ),
        if (search.text.trim().isNotEmpty)
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(12, 4, 12, 12),
            sliver: SliverList.builder(
              itemCount: results.length,
              itemBuilder: (context, i) =>
                  _ResultCard(place: results[i], onTap: onTap),
            ),
          ),
      ],
    );
  }
}

class _ResultCard extends StatelessWidget {
  const _ResultCard({required this.place, required this.onTap});

  final Place place;
  final void Function(Place place) onTap;

  @override
  Widget build(BuildContext context) {
    final subtitle = placeSubtitle(place);
    return Card.filled(
      margin: const EdgeInsets.symmetric(vertical: 4),
      clipBehavior: Clip.antiAlias,
      child: ListTile(
        title: Text(
          placeTitle(place),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: subtitle == null
            ? null
            : Text(subtitle, maxLines: 1, overflow: TextOverflow.ellipsis),
        onTap: () => onTap(place),
      ),
    );
  }
}
