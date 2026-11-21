part of '../sheet.dart';

/// A log of where each release takes the sheet and why, for getting to the bottom of
/// how a fling felt on a device: what speed the finger had, what speed the sheet was
/// let go at, and which rule decided where it went.
///
/// It's off, and compiled out, unless the app is built with
/// `--dart-define=CARDINAL_SHEET_LOG=true`. Then each release prints one line through
/// [debugPrint], which is `print` underneath, so it shows in a profile build as in a
/// debug one: on Android under the `flutter` tag, as `adb logcat -s flutter` shows.
///
/// A line is `SHEET release` and then `key=value` pairs, with no spaces within a
/// value. Lengths are in logical pixels up from the bottom of the window, speeds in
/// logical pixels per second, positive upwards. Nothing but numbers and the names of
/// rules goes into one.
abstract final class SheetLog {
  /// Whether releases are logged.
  static const enabled = bool.fromEnvironment('CARDINAL_SHEET_LOG');

  /// What every line starts with.
  static const prefix = 'SHEET';

  /// The line for a release at [extent] moving at [velocity], as
  /// [SheetController.release] was given it, among [anchors] and [ranges].
  ///
  /// - `via` is what let go of the sheet: `content` for a drag on the page's content,
  ///   its footer or its held row, which the content's scrolling passes on; `sheet`
  ///   for anything else, the handle and the scaffold's own drags among them.
  /// - `finger` is how fast the finger was moving when it lifted, by an estimate of
  ///   the library's own from the touches themselves, before any gesture recognizer
  ///   has decided whether that's a fling or capped its speed, with how far and for
  ///   how long it measured (`finger_px`, `finger_ms`); `none` if no finger lifted
  ///   for this release. [finger] is that estimate.
  /// - `carry` is 1 if the content could scroll on from the top with the speed left
  ///   ([carries]), as a drag on the content can and one on the handle can't.
  /// - `anchors` are the page's anchors, lowest first, each as `extent@gravity`, and
  ///   `ranges` its ranges, each as `lower..upper`.
  /// - `rule` is the [SheetRule] that decided, or `no-anchors` on a page with nowhere
  ///   to rest ([decision] is null); `at`, `arrival` and `gravity` say where that rule
  ///   weighed the fling's speed, how fast it was going there, and the gravity it was
  ///   weighed against, where it weighed one.
  /// - `landing` is where the fling would have carried the sheet if nothing caught it.
  /// - `rest` is `anchor` or `range`, `rest_extent` where that is, and `rest_range` the
  ///   range it rests within.
  static String release({
    required double extent,
    required double velocity,
    required List<ResolvedAnchor> anchors,
    required List<ResolvedRange> ranges,
    required SheetDecision? decision,
    bool content = false,
    bool carries = false,
    VelocityEstimate? finger,
  }) {
    final fields = <String, String>{
      'extent': _length(extent),
      'velocity': _velocity(velocity),
      'via': content ? 'content' : 'sheet',
      'finger': finger == null ? 'none' : _velocity(-finger.pixelsPerSecond.dy),
      if (finger != null) ...{
        'finger_px': _length(-finger.offset.dy),
        'finger_ms': '${finger.duration.inMilliseconds}',
      },
      'carry': carries ? '1' : '0',
      'anchors': anchors.isEmpty
          ? 'none'
          : [
              for (final a in anchors)
                '${_length(a.extent)}@${_speed(a.gravity)}',
            ].join(','),
      'ranges': ranges.isEmpty
          ? 'none'
          : [for (final r in ranges) _span(r)].join(','),
      'rule': decision == null ? 'no-anchors' : ruleName(decision.rule),
    };
    if (decision != null) {
      fields.addAll({
        if (decision.at case final at?) 'at': _length(at),
        if (decision.arrival case final arrival?) 'arrival': _speed(arrival),
        if (decision.gravity case final gravity?) 'gravity': _speed(gravity),
        'landing': _length(decision.landing),
        'rest': decision.range == null ? 'anchor' : 'range',
        'rest_extent': _length(decision.extent),
        if (decision.range case final range?) 'rest_range': _span(range),
      });
    }
    return [
      prefix,
      'release',
      for (final MapEntry(:key, :value) in fields.entries) '$key=$value',
    ].join(' ');
  }

  /// The line for a drag that ended at [reported], as the content's scrolling had it,
  /// of which the sheet was let go at [released] instead: as when the sheet is as high
  /// as it goes and the fling is the content's to scroll with.
  ///
  /// The scaffold's own drags have no such line: they let go of the sheet at just the
  /// speed their gesture detector reports.
  static String dragEnd({required double reported, required double released}) =>
      '$prefix drag-end via=content reported=${_velocity(reported)} '
      'released=${_velocity(released)}';

  /// How a line names [rule].
  static String ruleName(SheetRule rule) => switch (rule) {
    SheetRule.slowToNearest => 'slow-to-nearest',
    SheetRule.slowInRange => 'slow-in-range',
    SheetRule.glideWithinRange => 'glide-within-range',
    SheetRule.glideIntoRange => 'glide-into-range',
    SheetRule.rangeEnd => 'range-end',
    SheetRule.noAnchorAhead => 'no-anchor-ahead',
    SheetRule.ranOutBeforeAnchor => 'ran-out-before-anchor',
    SheetRule.caughtAtAnchor => 'caught-at-anchor',
    SheetRule.passedAllAnchors => 'passed-all-anchors',
    SheetRule.carriedIntoScroll => 'carried-into-scroll',
  };

  static String _length(double value) => _fixed(value, 1);

  static String _velocity(double value) => _fixed(value, 1);

  static String _speed(double value) =>
      value.isFinite ? _fixed(value, 0) : 'inf';

  /// [value] to [digits] decimal places, with nothing that rounds to zero negative.
  static String _fixed(double value, int digits) {
    final text = value.toStringAsFixed(digits);
    return double.parse(text) == 0 ? 0.0.toStringAsFixed(digits) : text;
  }

  static String _span(ResolvedRange range) =>
      '${_length(range.lower)}..${_length(range.upper)}';
}

/// [child], which is the sheet, with a [_SheetFingerLog] around it if releases are
/// logged.
Widget _withFingerLog(SheetController controller, Widget child) =>
    SheetLog.enabled
    ? _SheetFingerLog(controller: controller, child: child)
    : child;

/// Estimates how fast each finger on the sheet is moving when it lifts, for
/// [SheetLog]: from the touches themselves, as a gesture recognizer does, but with
/// none of what a recognizer then does to the speed.
class _SheetFingerLog extends StatefulWidget {
  const _SheetFingerLog({required this.controller, required this.child});

  final SheetController controller;
  final Widget child;

  @override
  State<_SheetFingerLog> createState() => _SheetFingerLogState();
}

class _SheetFingerLogState extends State<_SheetFingerLog> {
  final Map<int, VelocityTracker> _trackers = {};

  void _track(PointerEvent event) => _trackers
      .putIfAbsent(event.pointer, () => VelocityTracker.withKind(event.kind))
      .addPosition(event.timeStamp, event.position);

  @override
  Widget build(BuildContext context) => Listener(
    onPointerDown: (event) {
      widget.controller._finger = null;
      _track(event);
    },
    onPointerMove: _track,
    // Before any recognizer hears of it, and so before the release it leads to.
    onPointerUp: (event) => widget.controller._finger = _trackers
        .remove(event.pointer)
        ?.getVelocityEstimate(),
    onPointerCancel: (event) => _trackers.remove(event.pointer),
    child: widget.child,
  );
}
