/// The geocoder's models, which offline search needs: offered once at startup in a sheet
/// that can be put off, and again from the Offline tab, then downloaded in the background
/// as a region of their own (see `OfflineDownloads.startGeocoderModels`).
library;

import 'dart:async';
import 'dart:ui' show PlatformDispatcher;

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoActivityIndicator, CupertinoIcons;
import 'package:flutter/scheduler.dart';
import 'package:material_ui/material_ui.dart';

import '../map/iso_639.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
import '../ui/startup_offers.dart';
import '../ui/toast.dart';
import 'downloads/database.dart';
import 'downloads/engine.dart';
import 'format.dart';
import 'offline_scope.dart';

/// Whether the startup sheet has been shown and answered (or put off): it's offered at
/// startup only once, and from the Offline tab after that.
final geocoderModelsOffered = Setting.flag(
  'offline.geocoderModelsOffered',
  false,
);

/// The language of the categories model to download: the device's, as ISO 639-3 (the
/// release's file names), or null if it has no such code.
String? geocoderModelLanguage() =>
    iso639_3[PlatformDispatcher.instance.locale.languageCode];

/// Shows the sheet offering [plan], and starts downloading it if that's chosen. Either
/// way, the startup sheet won't come back.
Future<void> offerGeocoderModels(
  BuildContext context,
  OfflineDownloads downloads,
  GeocoderModelsPlan plan,
) async {
  final settings = StoreScope.of(context).settings;
  final download = await showAppSheet<bool>(
    context: context,
    builder: (context) => GeocoderModelsSheet(plan: plan),
  );
  await settings.set(geocoderModelsOffered, true);
  if (download == true) {
    try {
      await downloads.startGeocoderModels(plan);
    } on Object catch (e) {
      debugPrint('offline: starting the geocoder models: $e');
    }
  }
}

/// What offline search needs and how big it is, with Download and Not now.
class GeocoderModelsSheet extends StatelessWidget {
  const GeocoderModelsSheet({super.key, required this.plan});

  final GeocoderModelsPlan plan;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodyMedium?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    Widget item(String label, int? bytes) => Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Row(
        children: [
          Expanded(child: Text(label, style: theme.textTheme.bodyMedium)),
          Text(formatBytes(bytes ?? 0), style: muted),
        ],
      ),
    );
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 0, 20, 12),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text('Search offline', style: theme.textTheme.titleLarge),
            const SizedBox(height: 8),
            Text(
              'Search works without a connection, right on your phone. It just '
              'needs a one-time download.',
              style: muted,
            ),
            const SizedBox(height: 10),
            item('Addresses and places', plan.tagger.bytes),
            if (plan.categories case final categories?)
              item('Categories, like “coffee”', categories.bytes),
            const SizedBox(height: 16),
            AppButton(
              label: 'Download · ${formatBytes(plan.bytes)}',
              onPressed: () => Navigator.of(context).pop(true),
            ),
            const SizedBox(height: 4),
            AppButton(
              kind: AppButtonKind.plain,
              label: 'Not now',
              onPressed: () => Navigator.of(context).pop(false),
            ),
          ],
        ),
      ),
    );
  }
}

/// Offers the geocoder's models once the first frame is out and the startup offers held
/// in its [StartupOffersScope] (your location's) are answered, if downloads are in this
/// build, the sheet hasn't been answered before, the models aren't downloaded or on
/// their way, and the release can be read. Otherwise, and once it's answered, it's just
/// [child].
class GeocoderModelsPrompt extends StatefulWidget {
  const GeocoderModelsPrompt({super.key, required this.child});

  final Widget child;

  @override
  State<GeocoderModelsPrompt> createState() => _GeocoderModelsPromptState();
}

class _GeocoderModelsPromptState extends State<GeocoderModelsPrompt> {
  bool _checked = false;

  /// What's offered at startup before the sheet, such as your location: the sheet
  /// waits for them to be answered, so it never opens under them.
  final _offers = StartupOffers();

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_checked) return;
    _checked = true;
    SchedulerBinding.instance.addPostFrameCallback((_) => unawaited(_check()));
  }

  Future<void> _check() async {
    final settings = StoreScope.of(context).settings;
    final opening = OfflineScope.of(context).downloads;
    if (await settings.get(geocoderModelsOffered)) return;
    final downloads = await opening;
    if (downloads == null) return;
    final GeocoderModelsPlan? plan;
    try {
      if (await downloads.watchGeocoderModels().first != null) return;
      plan = await downloads.planGeocoderModels(geocoderModelLanguage());
    } on Object catch (e) {
      // Offline, say: offered on a later launch.
      debugPrint('offline: planning the geocoder models: $e');
      return;
    }
    if (plan == null) return;
    await _offers.answered();
    if (!mounted) return;
    await offerGeocoderModels(context, downloads, plan);
  }

  @override
  Widget build(BuildContext context) =>
      StartupOffersScope(offers: _offers, child: widget.child);
}

/// The Offline tab's row for the models, under the search toggle: Download until
/// they're downloaded, then their progress, or Retry if they failed. Nothing once
/// they're ready, or in a build without downloads.
class GeocoderModelsRow extends StatefulWidget {
  const GeocoderModelsRow({super.key});

  @override
  State<GeocoderModelsRow> createState() => _GeocoderModelsRowState();
}

class _GeocoderModelsRowState extends State<GeocoderModelsRow> {
  OfflineDownloads? _downloads;
  StreamSubscription<RegionProgress?>? _watch;

  /// The models' region, once watched; null in it while there's none.
  ({RegionProgress? region})? _models;
  bool _planning = false;
  bool _opening = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_opening) return;
    _opening = true;
    unawaited(_open(OfflineScope.of(context).downloads));
  }

  Future<void> _open(Future<OfflineDownloads?> opening) async {
    final downloads = await opening;
    if (downloads == null || !mounted) return;
    _downloads = downloads;
    _watch = downloads.watchGeocoderModels().listen((region) {
      if (mounted) setState(() => _models = (region: region));
    });
  }

  @override
  void dispose() {
    _watch?.cancel();
    super.dispose();
  }

  Future<void> _offer() async {
    final downloads = _downloads!;
    setState(() => _planning = true);
    GeocoderModelsPlan? plan;
    try {
      plan = await downloads.planGeocoderModels(geocoderModelLanguage());
    } on Object catch (e) {
      debugPrint('offline: planning the geocoder models: $e');
    }
    if (!mounted) return;
    setState(() => _planning = false);
    if (plan == null) {
      showToast(context, "Couldn't reach the download server");
      return;
    }
    await offerGeocoderModels(context, downloads, plan);
  }

  @override
  Widget build(BuildContext context) {
    final models = _models;
    if (models == null) return const SizedBox.shrink();
    final region = models.region;
    if (region?.state == RegionState.ready) return const SizedBox.shrink();
    final theme = Theme.of(context);
    final cupertino = isCupertinoOf(context);
    final colors = theme.colorScheme;
    final failed = region?.state == RegionState.failed;
    final small = theme.textTheme.bodySmall?.copyWith(
      color: failed ? colors.error : colors.onSurfaceVariant,
    );
    final downloading =
        region != null && !failed && region.state != RegionState.deleting;
    final status = switch (region?.state) {
      null || RegionState.deleting => 'One-time download',
      RegionState.failed => 'Download failed',
      RegionState.installing => 'Finishing…',
      _ =>
        region!.bytesTotal > 0
            ? 'Downloading · ${formatBytes(region.bytesDone)} of '
                  '${formatBytes(region.bytesTotal)}'
            : 'Downloading…',
    };
    final Widget? trailing = switch (region?.state) {
      null || RegionState.deleting =>
        _planning
            ? (cupertino
                  ? const CupertinoActivityIndicator()
                  : const SizedBox.square(
                      dimension: 20,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    ))
            : AppButton(
                kind: AppButtonKind.plain,
                label: 'Download',
                onPressed: _offer,
              ),
      RegionState.failed => AppButton(
        kind: AppButtonKind.plain,
        label: 'Retry',
        onPressed: () => unawaited(_downloads!.retry(region!.id)),
      ),
      _ => null,
    };
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: AppListSection(
        children: [_row(context, region, trailing, status, small, downloading)],
      ),
    );
  }

  Widget _row(
    BuildContext context,
    RegionProgress? region,
    Widget? trailing,
    String status,
    TextStyle? small,
    bool downloading,
  ) {
    final cupertino = isCupertinoOf(context);
    final colors = Theme.of(context).colorScheme;
    final failed = region?.state == RegionState.failed;
    return AppListTile(
      leading: Icon(
        failed
            ? (cupertino
                  ? CupertinoIcons.exclamationmark_circle
                  : Icons.error_outline)
            : (cupertino ? CupertinoIcons.arrow_down : Icons.download),
        color: failed
            ? colors.error
            : (cupertino ? colors.primary : colors.onSurfaceVariant),
      ),
      title: const Text('Offline search'),
      subtitle: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(status, style: small),
          if (downloading)
            Padding(
              padding: const EdgeInsets.only(top: 6, bottom: 2),
              child: ClipRRect(
                borderRadius: BorderRadius.circular(2),
                child: LinearProgressIndicator(
                  value:
                      region!.bytesTotal > 0 &&
                          region.state != RegionState.installing
                      ? region.bytesDone / region.bytesTotal
                      : null,
                  minHeight: 4,
                  backgroundColor: colors.onSurface.withValues(alpha: 0.08),
                ),
              ),
            ),
        ],
      ),
      trailing: trailing,
    );
  }
}
