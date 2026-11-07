import 'dart:math' as math;

import 'dart:ui' show PlatformDispatcher;

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../geocoder/address.dart';
import '../geocoder/geocoder.dart';
import '../map/map.dart';
import '../routes.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import 'place_search.dart';

/// The home sheet: a search bar, peeking over the map, with results below it as you type.
///
/// Focusing the search bar pulls the sheet up to full height; the bar stays pinned at
/// the top while the results scroll under it. Tapping a result opens its place.
class SearchSheet extends StatefulWidget {
  const SearchSheet({super.key});

  @override
  State<SearchSheet> createState() => _SearchSheetState();
}

class _SearchSheetState extends State<SearchSheet> {
  static const _bar = #searchBar;

  final _text = TextEditingController();
  final _focus = FocusNode(debugLabel: 'search');
  PlaceSearch? _search;

  @override
  void initState() {
    super.initState();
    _focus.addListener(_focusChanged);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _search ??= PlaceSearch(
      GeocoderScope.of(context),
      lang: PlatformDispatcher.instance.locale.toLanguageTag(),
    );
  }

  @override
  void dispose() {
    _focus.dispose();
    _text.dispose();
    _search?.dispose();
    super.dispose();
  }

  void _focusChanged() {
    if (!_focus.hasFocus) return;
    _sampleMap();
    SheetScaffold.controllerOf(context).animateTo(const SheetAnchor.full());
  }

  /// Results near what the map shows rank higher: what the sheet leaves uncovered now,
  /// before the sheet covers it. If it covers everything already, what was last seen.
  void _sampleMap() {
    final view = SheetScaffold.viewOf(context);
    final clear = EdgeInsets.only(top: MediaQuery.paddingOf(context).top) + view.obscured;
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
    _focus.unfocus();
    await StoreScope.of(context).places.put(place);
    if (!mounted) return;
    // Its close button comes back here to search again.
    final closed = await context.push<bool>(placeLocation(place.id));
    if (closed == true && mounted) _focus.requestFocus();
  }

  @override
  Widget build(BuildContext context) {
    final search = _search!;
    return ListenableBuilder(
      listenable: Listenable.merge([_focus, search]),
      builder: (context, _) => SheetContent(
        // Just the bar, until there's something to show above the keyboard or below it.
        anchors: _focus.hasFocus || search.results.isNotEmpty
            ? const [SheetAnchor.section(_bar), SheetAnchor.full()]
            : const [SheetAnchor.section(_bar)],
        slivers: [
          SliverPersistentHeader(
            pinned: true,
            delegate: _SearchBarHeader(
              color: Theme.of(context).colorScheme.surfaceContainerLow,
              child: SheetSection(
                id: _bar,
                child: Padding(
                  padding: const EdgeInsets.fromLTRB(16, 4, 16, 12),
                  child: SearchBar(
                    controller: _text,
                    focusNode: _focus,
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
                                  _focus.requestFocus();
                                },
                              ),
                      ),
                    ],
                    onChanged: search.update,
                    onSubmitted: (_) {
                      if (search.results case [final first, ...]) _open(first);
                    },
                  ),
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
    );
  }
}

/// The search bar, pinned to the top of the sheet's content.
class _SearchBarHeader extends SliverPersistentHeaderDelegate {
  _SearchBarHeader({required this.color, required this.child});

  final Color color;
  final Widget child;

  /// The bar and its padding.
  static const double _height = 72;

  @override
  double get minExtent => _height;

  @override
  double get maxExtent => _height;

  @override
  Widget build(BuildContext context, double shrinkOffset, bool overlapsContent) =>
      ColoredBox(color: color, child: child);

  @override
  bool shouldRebuild(_SearchBarHeader oldDelegate) =>
      color != oldDelegate.color || child != oldDelegate.child;
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
      PlaceSearch(loading: false, results: [], text: final text) when text.trim().isNotEmpty =>
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
              itemBuilder: (context, i) => _ResultCard(place: results[i], onTap: onTap),
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
        title: Text(placeTitle(place), maxLines: 1, overflow: TextOverflow.ellipsis),
        subtitle: subtitle == null
            ? null
            : Text(subtitle, maxLines: 1, overflow: TextOverflow.ellipsis),
        onTap: () => onTap(place),
      ),
    );
  }
}
