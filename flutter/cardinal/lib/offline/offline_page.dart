import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoActivityIndicator, CupertinoIcons;
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../map/map.dart';
import '../sheet/sheet.dart';
import '../ui/adaptive.dart';
import '../ui/symbols.dart';
import 'downloads/database.dart';
import 'downloads/engine.dart';
import 'drawing/area_layer.dart';
import 'format.dart';
import 'offline_scope.dart';

/// Where drawing an area to download starts, over the Offline tab.
const downloadAreaLocation = '/offline/download';

/// The Offline tab: the areas downloaded, and Download an area.
///
/// | Downloads | Shows |
/// |---|---|
/// | not in this build | that they aren't available |
/// | opening | the title alone |
/// | no areas | what downloading is for, and Download an area |
/// | areas | each with its state, and Download an area in the header |
///
/// Each area is downloading (with its progress), finishing, ready, failed (with Retry),
/// or being deleted. Deleting one asks first: from its menu on Android, by swiping it
/// on iOS. Tapping one shows its bounds on the map, framed above the sheet, until it's
/// tapped again, Download an area is tapped, it's deleted, or the tab is left.
class OfflinePage extends StatefulWidget {
  const OfflinePage({super.key});

  @override
  State<OfflinePage> createState() => _OfflinePageState();
}

class _OfflinePageState extends State<OfflinePage> {
  static const _content = #offlineContent;

  /// Downloads, once open: null until then, and [_none] if this build has none.
  OfflineDownloads? _downloads;
  bool _none = false;
  bool _opening = false;
  StreamSubscription<List<RegionProgress>>? _watch;
  List<RegionProgress>? _areas;

  /// Areas swiped away on iOS, gone from the list before downloads say so.
  final _dismissed = <int>{};

  late SparkleMapController _map;

  /// The area whose bounds the map shows.
  int? _shown;

  /// Space kept around a shown area, in logical pixels, as around a place.
  static const _margin = 48.0;

  /// How close the map zooms in on a small area.
  static const _maxZoom = 16.0;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _map = MapScope.of(context);
    if (_opening) return;
    _opening = true;
    unawaited(_open(OfflineScope.of(context).downloads));
  }

  Future<void> _open(Future<OfflineDownloads?> opening) async {
    final downloads = await opening;
    if (!mounted) return;
    if (downloads == null) {
      setState(() => _none = true);
      return;
    }
    _downloads = downloads;
    _watch = downloads.watch().listen((regions) {
      if (!mounted) return;
      setState(() {
        _areas = [
          for (final r in regions)
            if (r.kind == RegionKind.explicit && !_dismissed.contains(r.id)) r,
        ];
      });
      if (_shown case final shown? when !_areas!.any((a) => a.id == shown)) {
        _hide();
      }
    });
  }

  @override
  void dispose() {
    _watch?.cancel();
    _clearMap();
    super.dispose();
  }

  /// Shows [area]'s bounds on the map, or stops if they're shown.
  void _toggle(RegionProgress area) {
    final polygon = area.polygon;
    if (polygon == null) return;
    if (_shown == area.id) {
      _hide();
      return;
    }
    setState(() => _shown = area.id);
    showArea(_map, polygon);
    final b = polygon.bounds;
    _map.setSubject(
      west: b.west,
      south: b.south,
      east: b.east,
      north: b.north,
      maxZoom: _maxZoom,
      margin: _margin,
    );
  }

  void _hide() {
    if (_shown == null) return;
    _clearMap();
    setState(() => _shown = null);
  }

  /// Takes the shown area off the map, and lets the camera go of it where it is.
  void _clearMap() {
    if (_shown == null) return;
    showArea(_map, null);
    _map.clearSubject();
  }

  void _download() {
    // Drawing uses the same layer.
    _hide();
    unawaited(context.push<void>(downloadAreaLocation));
  }

  /// Asks whether to delete [area], and deletes it if so; true if it's going.
  Future<bool> _delete(RegionProgress area) async {
    final name = _name(area);
    final delete = await showAppDialog<bool>(
      context: context,
      title: 'Delete “$name”?',
      message: 'Its maps and directions will be removed from this device.',
      actions: const [
        AppDialogAction(label: 'Cancel', value: false),
        AppDialogAction(label: 'Delete', value: true, isDestructive: true),
      ],
    );
    if (delete != true || !mounted) return false;
    if (_shown == area.id) _hide();
    setState(() {
      _dismissed.add(area.id);
      _areas = _areas?.where((a) => a.id != area.id).toList();
    });
    unawaited(_downloads!.delete(area.id));
    return true;
  }

  String _name(RegionProgress area) =>
      area.name ?? defaultAreaName(context, area.createdAt);

  @override
  Widget build(BuildContext context) {
    final areas = _areas;
    return SheetContent(
      showsBottomBar: true,
      anchors: const [SheetAnchor.section(_content)],
      slivers: [
        SliverToBoxAdapter(
          child: SheetSection(
            id: _content,
            // The sheet follows as areas come and go, rather than jumping.
            child: AnimatedSize(
              duration: const Duration(milliseconds: 250),
              curve: Curves.easeOutCubic,
              alignment: Alignment.topCenter,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _Header(
                    onDownload: areas == null || areas.isEmpty
                        ? null
                        : _download,
                  ),
                  if (_none)
                    const _Unavailable()
                  else if (areas != null && areas.isEmpty)
                    _Empty(onDownload: _download)
                  else if (areas != null)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 8),
                      child: AppListSection(
                        children: [
                          for (final area in areas)
                            _AreaRow(
                              key: ValueKey(area.id),
                              area: area,
                              name: _name(area),
                              shown: _shown == area.id,
                              onTap: () => _toggle(area),
                              onRetry: () =>
                                  unawaited(_downloads!.retry(area.id)),
                              onDelete: () => _delete(area),
                            ),
                        ],
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.onDownload});

  /// Null leaves the button out: the empty page has its own.
  final VoidCallback? onDownload;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsetsDirectional.only(
      start: 20,
      end: 12,
      top: 4,
      bottom: 8,
    ),
    child: ConstrainedBox(
      constraints: const BoxConstraints(minHeight: 48),
      child: Row(
        children: [
          Expanded(
            child: Text(
              'Offline maps',
              style: Theme.of(context).textTheme.titleLarge,
            ),
          ),
          if (onDownload case final onDownload?)
            AppButton(
              kind: AppButtonKind.tinted,
              icon: AppSymbols.of(context).add,
              label: 'Download an area',
              onPressed: onDownload,
            ),
        ],
      ),
    ),
  );
}

class _Empty extends StatelessWidget {
  const _Empty({required this.onDownload});

  final VoidCallback onDownload;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Download an area to see its map and get directions there without '
            'a connection.',
            style: theme.textTheme.bodyMedium?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: 16),
          AppButton(
            icon: _icon(context, Icons.download, CupertinoIcons.arrow_down),
            label: 'Download an area',
            onPressed: onDownload,
          ),
        ],
      ),
    );
  }
}

class _Unavailable extends StatelessWidget {
  const _Unavailable();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.colorScheme.onSurfaceVariant;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
      child: Row(
        children: [
          Icon(
            _icon(context, Icons.cloud_off_outlined, CupertinoIcons.cloud),
            size: 20,
            color: muted,
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              "Offline downloads aren't available in this build.",
              style: theme.textTheme.bodyMedium?.copyWith(color: muted),
            ),
          ),
        ],
      ),
    );
  }
}

/// A downloaded area: its name, its state, and how big it is and when it was started.
class _AreaRow extends StatelessWidget {
  const _AreaRow({
    super.key,
    required this.area,
    required this.name,
    required this.shown,
    required this.onTap,
    required this.onRetry,
    required this.onDelete,
  });

  final RegionProgress area;
  final String name;

  /// Whether the map shows its bounds.
  final bool shown;

  /// Shows its bounds, or stops.
  final VoidCallback onTap;
  final VoidCallback onRetry;

  /// Asks, then deletes; true if it's going.
  final Future<bool> Function() onDelete;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final cupertino = isCupertinoOf(context);
    final colors = theme.colorScheme;
    final area = this.area;
    final failed = area.state == RegionState.failed;
    final deleting = area.state == RegionState.deleting;
    final small = theme.textTheme.bodySmall?.copyWith(
      color: failed ? colors.error : colors.onSurfaceVariant,
    );
    final date = areaDate(context, area.createdAt);
    final (String status, double? progress, bool bar) = switch (area.state) {
      RegionState.planned || RegionState.downloading => (
        area.bytesTotal > 0
            ? 'Downloading · ${formatBytes(area.bytesDone)} of '
                  '${formatBytes(area.bytesTotal)}'
            : 'Downloading…',
        area.bytesTotal > 0 ? area.bytesDone / area.bytesTotal : null,
        true,
      ),
      RegionState.installing => ('Finishing…', null, true),
      RegionState.ready => (
        '${formatBytes(area.bytesTotal)} · $date',
        1,
        false,
      ),
      // Short, beside Retry.
      RegionState.failed => ('Download failed', null, false),
      RegionState.deleting => ('Deleting…', null, false),
    };
    final subtitle = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(
          status,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: small,
        ),
        if (bar)
          Padding(
            padding: const EdgeInsets.only(top: 6, bottom: 2),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: LinearProgressIndicator(
                value: progress,
                minHeight: 4,
                backgroundColor: colors.onSurface.withValues(alpha: 0.08),
              ),
            ),
          ),
      ],
    );
    final Widget trailing = deleting
        ? (cupertino
              ? const CupertinoActivityIndicator()
              : const SizedBox.square(
                  dimension: 20,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ))
        : Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (failed)
                AppButton(
                  kind: AppButtonKind.plain,
                  label: 'Retry',
                  onPressed: onRetry,
                ),
              // On iOS an area is deleted by swiping it.
              if (!cupertino)
                PopupMenuButton<_Action>(
                  tooltip: 'More',
                  icon: const Icon(Icons.more_vert),
                  // Once the menu has closed, so the question opens over the page.
                  onSelected: (_) => unawaited(onDelete()),
                  itemBuilder: (context) => const [
                    PopupMenuItem(value: _Action.delete, child: Text('Delete')),
                  ],
                ),
            ],
          );
    final row = Opacity(
      opacity: deleting ? 0.5 : 1,
      child: AppListTile(
        leading: Icon(
          failed
              ? _icon(
                  context,
                  Icons.error_outline,
                  CupertinoIcons.exclamationmark_circle,
                )
              : shown
              ? _icon(context, Icons.map, CupertinoIcons.map_fill)
              : _icon(context, Icons.map_outlined, CupertinoIcons.map),
          color: failed
              ? colors.error
              : (cupertino || shown ? colors.primary : colors.onSurfaceVariant),
        ),
        title: Text(name, maxLines: 1, overflow: TextOverflow.ellipsis),
        subtitle: subtitle,
        trailing: trailing,
        onTap: deleting ? null : onTap,
      ),
    );
    if (!cupertino || deleting) return row;
    return Dismissible(
      key: ValueKey('dismiss ${area.id}'),
      direction: DismissDirection.endToStart,
      confirmDismiss: (_) => onDelete(),
      background: ColoredBox(
        color: colors.error,
        child: Align(
          alignment: AlignmentDirectional.centerEnd,
          child: Padding(
            padding: const EdgeInsetsDirectional.only(end: 20),
            child: Text(
              'Delete',
              style: theme.textTheme.labelLarge?.copyWith(
                color: const Color(0xFFFFFFFF),
              ),
            ),
          ),
        ),
      ),
      child: row,
    );
  }
}

/// What an area's menu does.
enum _Action { delete }

/// [cupertino] on iOS, [material] elsewhere.
IconData _icon(BuildContext context, IconData material, IconData cupertino) =>
    isCupertinoOf(context) ? cupertino : material;
