import 'dart:async';
import 'dart:math' as math;

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../map/map.dart';
import '../map/map_drawing.dart';
import '../saved/saved_feedback.dart';
import '../sheet/sheet.dart';
import '../ui/haptics.dart';
import 'download_steps.dart';
import 'downloads/bucket.dart';
import 'downloads/engine.dart';
import 'downloads/geometry.dart';
import 'downloads/plan.dart';
import 'drawing/area_layer.dart';
import 'drawing/lasso.dart';
import 'format.dart';
import 'offline_scope.dart';

/// The steps of downloading an area.
enum DownloadStep {
  /// A finger draws around the area on the frozen map.
  drawing,

  /// The area's plan is being read from the bucket.
  measuring,

  /// Reading the plan failed.
  failed,

  /// Page 1: what the area needs, the free space, and its name.
  size,

  /// Page 2: transit (to be built).
  transit,
}

/// Downloading an area: drawing it on the map, measuring it, then the pages that
/// confirm it, all in the one sheet over the map. The map is held still throughout
/// (see `map/map_drawing.dart`), so it shows the area where it was drawn.
///
/// | In | When | Then |
/// |---|---|---|
/// | drawing | the finger lifts | its outline, closed round all it encloses, is on the map; measuring |
/// | drawing | its outline is too small, or off the map | drawing, saying why |
/// | drawing | Cancel, or back | the Offline tab |
/// | measuring | the plan comes | page 1 |
/// | measuring | the plan fails | failed, saying why |
/// | measuring, failed, page 1 | Redraw, or back | drawing, the area gone |
/// | failed | Try again | measuring |
/// | page 1 | Next (there are transit feeds) | page 2 |
/// | page 1 | Download (there are none) | the area downloads; the Offline tab |
/// | page 2 | Back, or back | page 1, as it was |
class DownloadAreaPage extends StatefulWidget {
  const DownloadAreaPage({super.key});

  @override
  State<DownloadAreaPage> createState() => _DownloadAreaPageState();
}

class _DownloadAreaPageState extends State<DownloadAreaPage> {
  static const _body = #downloadAreaBody;

  /// Spared beyond the download's size before it counts as fitting: a tenth of it, for
  /// the map's tiles as they're unpacked, and at least this much, so the phone isn't
  /// left full.
  static const spareBytes = 100 * 1000 * 1000;

  late final SparkleMapController _map;
  late final MapDrawing _drawing;

  DownloadStep _step = DownloadStep.drawing;

  /// Whether the last step change went forward, and whether it was from one page to
  /// the other, which slide.
  bool _forward = true;
  bool _slides = false;
  LassoProblem? _problem;
  GeoPolygon? _area;
  OfflinePlan? _plan;
  int? _free;
  bool _offline = false;
  TextEditingController? _name;
  bool _starting = false;

  /// Counts measurements, so one overtaken by a redraw is dropped when it comes.
  int _measuring = 0;

  @override
  void initState() {
    super.initState();
    _map = MapScope.of(context);
    _drawing = MapScope.drawingOf(context);
    // Once built: the layer over the map rebuilds as the map is frozen.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _step == DownloadStep.drawing) {
        _drawing.hold(this, onStroke: _stroke);
      }
    });
  }

  @override
  void dispose() {
    // Usually done as it was popped; the tree can't change while it's torn down.
    scheduleMicrotask(_leave);
    _name?.dispose();
    super.dispose();
  }

  /// Lets the map go, without the area.
  void _leave() {
    _measuring++;
    showArea(_map, null);
    _drawing.release(this);
  }

  static const _pages = {DownloadStep.size, DownloadStep.transit};

  void _go(DownloadStep step, {bool forward = true}) => setState(() {
    _forward = forward;
    _slides = _pages.contains(_step) && _pages.contains(step);
    _step = step;
  });

  void _stroke(List<Offset> stroke) {
    if (_step != DownloadStep.drawing || isLassoTap(stroke)) return;
    final lasso = closeLasso(thinStroke(stroke));
    final area = lasso.ring == null
        ? null
        : lassoPolygon(lasso.ring!, _map.screenToLngLat);
    if (area == null) {
      setState(() => _problem = lasso.problem ?? LassoProblem.offMap);
      return;
    }
    Haptics.impact();
    _area = area;
    showArea(_map, area);
    unawaited(_measure());
  }

  Future<void> _measure() async {
    final area = _area!;
    final attempt = ++_measuring;
    _problem = null;
    _drawing.hold(this);
    _go(DownloadStep.measuring);
    final scope = OfflineScope.of(context);
    final free = scope.freeBytes().then<int?>(
      (bytes) => bytes,
      onError: (Object e) {
        debugPrint('offline: free space unknown: $e');
        return null;
      },
    );
    final OfflinePlan plan;
    try {
      final downloads = await scope.downloads;
      if (downloads == null) throw StateError('no downloads in this build');
      plan = await downloads.plan(area);
    } on Object catch (e) {
      debugPrint('offline: measuring the area failed: $e');
      if (attempt != _measuring || !mounted) return;
      _offline = e is BucketError && e.status == null;
      _go(DownloadStep.failed);
      return;
    }
    final bytes = await free;
    if (attempt != _measuring || !mounted) return;
    _plan = plan;
    _free = bytes;
    _name ??= TextEditingController(
      text: defaultAreaName(context, DateTime.now()),
    );
    _go(DownloadStep.size);
  }

  void _redraw() {
    _measuring++;
    _plan = null;
    _area = null;
    _problem = null;
    showArea(_map, null);
    _drawing.hold(this, onStroke: _stroke);
    FocusScope.of(context).unfocus();
    _go(DownloadStep.drawing, forward: false);
  }

  void _cancel() {
    _leave();
    context.pop();
  }

  /// The map and routing bytes the plan downloads: its total, before transit.
  int get _bytes => _plan!.totals(const {}).downloadBytes;

  /// Whether [_bytes] fits in the free space, with [spareBytes] to spare; true if the
  /// free space isn't known.
  bool get _fits {
    final free = _free;
    if (free == null) return true;
    return free >= _bytes + math.max(_bytes ~/ 10, spareBytes);
  }

  String get _chosenName {
    final name = _name!.text.trim();
    return name.isEmpty ? defaultAreaName(context, DateTime.now()) : name;
  }

  Future<void> _download() async {
    final downloads = await OfflineScope.of(context).downloads;
    if (downloads == null || !mounted) return;
    setState(() => _starting = true);
    try {
      await downloads.start(_plan!, feeds: const {}, name: _chosenName);
    } on StalePlan catch (e) {
      // The releases moved on since it was measured: measure it again.
      debugPrint('offline: plan went stale: $e');
      if (!mounted) return;
      setState(() => _starting = false);
      unawaited(_measure());
      return;
    } on Object catch (e) {
      debugPrint('offline: starting the download failed: $e');
      if (!mounted) return;
      setState(() => _starting = false);
      showSheetMessage(context, "The download couldn't start. Try again.");
      return;
    }
    if (!mounted) return;
    Haptics.success();
    _cancel();
  }

  void _onPop(bool didPop, Object? result) {
    if (didPop) {
      _leave();
      return;
    }
    switch (_step) {
      case DownloadStep.drawing:
        break;
      case DownloadStep.measuring:
      case DownloadStep.failed:
      case DownloadStep.size:
        _redraw();
      case DownloadStep.transit:
        _go(DownloadStep.size, forward: false);
    }
  }

  Widget _content() => switch (_step) {
    DownloadStep.drawing => DrawingBar(problem: _problem, onCancel: _cancel),
    DownloadStep.measuring => MeasuringBar(onRedraw: _redraw),
    DownloadStep.failed => MeasureFailed(
      offline: _offline,
      onRedraw: _redraw,
      onRetry: _measure,
    ),
    DownloadStep.size => SizeStep(
      bytes: _bytes,
      free: _free,
      fits: _fits,
      hasTransit: _plan!.feeds.isNotEmpty,
      name: _name!,
      busy: _starting,
      onRedraw: _redraw,
      onNext: () {
        FocusScope.of(context).unfocus();
        _go(DownloadStep.transit);
      },
      onDownload: _download,
    ),
    DownloadStep.transit => TransitStep(
      onBack: () => _go(DownloadStep.size, forward: false),
    ),
  };

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: _step == DownloadStep.drawing,
    onPopInvokedWithResult: _onPop,
    child: SheetContent(
      anchors: const [SheetAnchor.section(_body)],
      // Its size is its step's; dragged down it would hide the buttons.
      draggable: false,
      slivers: [
        SliverToBoxAdapter(
          child: SheetSection(
            id: _body,
            child: StepSwitcher(
              step: _step,
              forward: _forward,
              slides: _slides,
              child: _content(),
            ),
          ),
        ),
      ],
    ),
  );
}

/// Changes from one step's content to the next: between the two pages they slide
/// the way they're going as they cross-fade, as pages pushed and popped do; between
/// other steps they just cross-fade. The sheet grows or shrinks to the new content.
class StepSwitcher extends StatelessWidget {
  const StepSwitcher({
    super.key,
    required this.step,
    required this.forward,
    required this.slides,
    required this.child,
  });

  final DownloadStep step;

  /// Whether it's going on to [step] rather than back to it.
  final bool forward;

  /// Whether the change to [step] slides: it's from one page to the other.
  final bool slides;
  final Widget child;

  static const duration = Duration(milliseconds: 300);

  /// How far the pages slide.
  static const _slide = 40.0;

  /// How far through a change the old content has faded out and the new starts in, as
  /// the sheet's own pages do.
  static const _handover = 0.35;

  @override
  Widget build(BuildContext context) {
    final rtl = Directionality.of(context) == TextDirection.rtl;
    return AnimatedSize(
      duration: duration,
      curve: Curves.easeOutCubic,
      alignment: Alignment.topCenter,
      child: AnimatedSwitcher(
        duration: duration,
        // Sized to the new content alone, the old laid over it from the top as it goes,
        // unclipped: the AnimatedSize clips to the sheet's size as it changes.
        layoutBuilder: (current, previous) => Stack(
          alignment: Alignment.topCenter,
          clipBehavior: Clip.none,
          children: [
            for (final old in previous)
              Positioned(top: 0, left: 0, right: 0, child: old),
            ?current,
          ],
        ),
        transitionBuilder: (child, animation) {
          final incoming = child.key == ValueKey(step);
          // Positive is the way it's going: new content comes from there.
          final direction = (forward ? 1.0 : -1.0) * (rtl ? -1 : 1);
          return AnimatedBuilder(
            animation: animation,
            child: child,
            builder: (context, child) {
              // The new content comes in over the rest of the change, decelerating
              // into place; the old leaves over its first part, speeding away. (The
              // old's animation runs backwards, from 1.)
              final double opacity, dx;
              if (incoming) {
                final u = ((animation.value - _handover) / (1 - _handover))
                    .clamp(0.0, 1.0);
                opacity = Curves.easeOut.transform(u);
                dx =
                    (1 - Curves.easeOutCubic.transform(u)) * _slide * direction;
              } else {
                final u = ((1 - animation.value) / _handover).clamp(0.0, 1.0);
                opacity = 1 - u;
                dx = -Curves.easeInCubic.transform(u) * _slide * direction;
              }
              return Opacity(
                opacity: opacity,
                child: Transform.translate(
                  offset: Offset(slides ? dx : 0, 0),
                  child: child,
                ),
              );
            },
          );
        },
        child: KeyedSubtree(key: ValueKey(step), child: child),
      ),
    );
  }
}
