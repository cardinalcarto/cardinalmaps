import 'dart:async';
import 'dart:math' as math;

import 'dart:ui' show PlatformDispatcher;

import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoActivityIndicator,
        CupertinoButton,
        CupertinoButtonSize,
        CupertinoIcons,
        CupertinoSearchTextField;
import 'package:geolocator/geolocator.dart' show Geolocator, LocationPermission;
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../directions/format.dart';
import '../directions/route_source.dart';
import '../geocoder/address.dart';
import '../geocoder/geocoder.dart';
import '../map/location.dart';
import '../map/map.dart';
import '../routes.dart';
import '../saved/saved_names.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
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

  /// Where you are, for how far each result is, on iOS: asked for when searching
  /// starts, if you've allowed it already. Null until then, or if you haven't.
  Fix? _here;

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

  /// Finds [_here], once, without asking for permission.
  Future<void> _findHere() async {
    if (_here != null) return;
    final source = DirectionsScope.of(context).location;
    final permission = await source.checkPermission();
    if (permission != LocationPermission.always &&
        permission != LocationPermission.whileInUse) {
      return;
    }
    final fix = await source.current();
    if (mounted && fix != null) setState(() => _here = fix);
  }

  /// The bar's tapped: searching, full, the keyboard up.
  void _searchFull() {
    if (!_searching) setState(() => _searching = true);
    if (isCupertinoOf(context)) unawaited(_findHere());
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

  /// Clears the query. Ready to type again, if searching; otherwise clearing it is all.
  void _clear() {
    _text.clear();
    _search!.update('');
    if (_searching) _focus.requestFocus();
  }

  /// Return in the field: opens the first result.
  void _openFirst() {
    if (_search!.results case [final first, ...]) _open(first);
  }

  @override
  Widget build(BuildContext context) {
    final search = _search!;
    final cupertino = isCupertinoOf(context);
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
                height: _SearchBarHeader.heightOf(
                  cupertino: cupertino,
                  chips: _chipsShowing,
                ),
                child: SheetSection(
                  id: _bar,
                  child: Column(
                    children: [
                      Padding(
                        padding: EdgeInsets.fromLTRB(
                          16,
                          cupertino ? 8 : 4,
                          cupertino ? 8 : 16,
                          _chipsShowing ? 8 : 12,
                        ),
                        child: cupertino
                            ? _CupertinoSearchField(
                                text: _text,
                                focus: _focus,
                                searching: _searching,
                                onTap: _searchFull,
                                onChanged: search.update,
                                onSubmitted: _openFirst,
                                onClear: _clear,
                                onCancel: _stopSearching,
                              )
                            : SearchBar(
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
                                            onPressed: _clear,
                                          ),
                                  ),
                                ],
                                onChanged: search.update,
                                onSubmitted: (_) => _openFirst(),
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
              builder: (context, _) =>
                  _Results(search: search, here: _here, onTap: _open),
            ),
          ],
        ),
      ),
    );
  }
}

/// The search bar, pinned to the top of the sheet's content, on the sheet's color.
class _SearchBarHeader extends SliverPersistentHeaderDelegate {
  _SearchBarHeader({required this.height, required this.child});

  final double height;
  final Widget child;

  /// The bar and its padding, and a row of pinned places under it if [chips]: Material's
  /// 56 high search bar, or iOS's 44 high search field.
  static double heightOf({required bool cupertino, required bool chips}) =>
      switch ((cupertino, chips)) {
        (false, false) => 72,
        (false, true) => 112,
        (true, false) => 64,
        (true, true) => 104,
      };

  @override
  double get minExtent => height;

  @override
  double get maxExtent => height;

  @override
  Widget build(
    BuildContext context,
    double shrinkOffset,
    bool overlapsContent,
  ) => SheetBuilder(
    // Clear over glass while the sheet peeks; the sheet's color once results scroll
    // under it.
    builder: (context, sheet, child) =>
        ColoredBox(color: sheet.surfaceColor, child: child),
    child: child,
  );

  @override
  bool shouldRebuild(_SearchBarHeader oldDelegate) =>
      height != oldDelegate.height || child != oldDelegate.child;
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
      itemBuilder: (context, i) {
        void open() => context.push<void>(placeLocation(pinned[i].id));
        final name = Text(
          savedName(pinned[i]),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        );
        return isCupertinoOf(context)
            ? CupertinoButton.tinted(
                sizeStyle: CupertinoButtonSize.small,
                onPressed: open,
                child: name,
              )
            : FilledButton.tonal(
                style: const ButtonStyle(visualDensity: VisualDensity.compact),
                onPressed: open,
                child: name,
              );
      },
    ),
  );
}

class _Results extends StatelessWidget {
  const _Results({
    required this.search,
    required this.here,
    required this.onTap,
  });

  final PlaceSearch search;

  /// Where you are, for how far each result is; null if that isn't known.
  final Fix? here;
  final void Function(Place place) onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final cupertino = isCupertinoOf(context);
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
        if (cupertino)
          SliverToBoxAdapter(
            // Room for the spinner only while it spins.
            child: search.loading
                ? const SizedBox(
                    height: 28,
                    child: CupertinoActivityIndicator(),
                  )
                : const SizedBox.shrink(),
          )
        else
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
        if (cupertino && search.text.trim().isNotEmpty && results.isNotEmpty)
          // Straight on the sheet, as iOS lists what a search finds, with the
          // separators starting at the text.
          SliverPadding(
            padding: const EdgeInsets.only(bottom: 12),
            sliver: SliverList.builder(
              itemCount: results.length,
              itemBuilder: (context, i) => Column(
                children: [
                  _ResultRow(place: results[i], here: here, onTap: onTap),
                  if (i < results.length - 1)
                    const Divider(indent: _ResultRow.textInset, height: 0.5),
                ],
              ),
            ),
          )
        else if (!cupertino && search.text.trim().isNotEmpty)
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

/// iOS's search field, with Cancel beside it while searching.
class _CupertinoSearchField extends StatelessWidget {
  const _CupertinoSearchField({
    required this.text,
    required this.focus,
    required this.searching,
    required this.onTap,
    required this.onChanged,
    required this.onSubmitted,
    required this.onClear,
    required this.onCancel,
  });

  /// Apple's height for a search field.
  static const double height = 44;

  final TextEditingController text;
  final FocusNode focus;
  final bool searching;
  final VoidCallback onTap;
  final ValueChanged<String> onChanged;
  final VoidCallback onSubmitted;
  final VoidCallback onClear;

  /// Stops searching, as back does: the query and results wait for the next tap.
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Row(
      children: [
        Expanded(
          child: SizedBox(
            height: height,
            child: CupertinoSearchTextField(
              controller: text,
              focusNode: focus,
              placeholder: 'Search',
              style: theme.textTheme.bodyLarge,
              itemSize: 20,
              borderRadius: BorderRadius.circular(height / 2),
              padding: const EdgeInsetsDirectional.fromSTEB(6, 11, 8, 11),
              prefixInsets: const EdgeInsetsDirectional.fromSTEB(12, 0, 0, 2),
              suffixInsets: const EdgeInsetsDirectional.only(end: 10),
              onTap: onTap,
              onChanged: onChanged,
              onSubmitted: (_) => onSubmitted(),
              onSuffixTap: onClear,
            ),
          ),
        ),
        AnimatedSize(
          duration: const Duration(milliseconds: 200),
          curve: Curves.easeOut,
          child: searching
              ? CupertinoButton(
                  padding: const EdgeInsetsDirectional.only(start: 12, end: 8),
                  onPressed: onCancel,
                  child: const Text('Cancel'),
                )
              : const SizedBox(width: 8),
        ),
      ],
    );
  }
}

/// A result in iOS's list: what it is, by its icon, then its name, and how far it is
/// before its address.
class _ResultRow extends StatelessWidget {
  const _ResultRow({
    required this.place,
    required this.here,
    required this.onTap,
  });

  final Place place;
  final Fix? here;
  final void Function(Place place) onTap;

  /// Where the text starts, past the icon: where separators start.
  static const double textInset = 64;

  static IconData _icon(String kind) => switch (kind) {
    'poi' => CupertinoIcons.location_solid,
    'address' => CupertinoIcons.house_fill,
    'street' => CupertinoIcons.map,
    _ => CupertinoIcons.globe,
  };

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    // Places in the accent; addresses, streets and areas quieter.
    final poi = place.kind == 'poi';
    final glyph = poi ? colors.primary : colors.onSurfaceVariant;
    final address = placeSubtitle(place);
    final here = this.here;
    final distance = here == null
        ? null
        : formatDistance(
            Geolocator.distanceBetween(
              here.lat,
              here.lng,
              place.center.lat,
              place.center.lon,
            ),
            miles: usesMiles(PlatformDispatcher.instance.locale),
          );
    final subtitle = [?distance, ?address].join(' · ');
    return AppListTile(
      leading: DecoratedBox(
        decoration: BoxDecoration(
          color: glyph.withValues(alpha: 0.14),
          shape: BoxShape.circle,
        ),
        // CupertinoListTile's leading size: the text starts at [textInset].
        child: SizedBox.square(
          dimension: 28,
          child: Icon(_icon(place.kind), size: 15, color: glyph),
        ),
      ),
      // Room to breathe: results are read at a glance, one after another.
      padding: const EdgeInsetsDirectional.fromSTEB(20, 12, 14, 12),
      title: Text(
        placeTitle(place),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      subtitle: subtitle.isEmpty
          ? null
          : Padding(
              padding: const EdgeInsets.only(top: 3),
              child: Text(
                subtitle,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
      onTap: () => onTap(place),
    );
  }
}
