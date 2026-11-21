/// Transit's journeys as rows of the directions sheet's list, each as tall as any
/// other way's row, and the line above them: how live the times are, how stops are
/// reached, and the times along the axis the rows share.
library;

import 'dart:math' as math;

import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoButton,
        CupertinoIcons,
        CupertinoMenuAnchor,
        CupertinoMenuItem;
import 'package:material_ui/material_ui.dart';

import '../sheet/sheet.dart';
import '../ui/adaptive.dart';
import 'animated_rows.dart';
import 'connections.dart';
import 'format.dart';
import 'option_row.dart';
import 'scrub.dart';
import 'speed_profile.dart' show routeGraphHeight;
import 'transit_plan.dart';
import 'transit_sheet.dart';
import 'transit_timeline.dart';

/// What [journey]'s row says of it after its times, each only if there's any: how
/// many times it changes vehicles, a ride that stays on as another trip being one
/// vehicle, and how long it's on the streets, getting to, between and from stops.
List<String> journeyFacts(Journey journey) {
  final transfers = journey.rides.length - 1;
  final streets = journey.legs.whereType<StreetLeg>();
  final seconds = streets.fold(0, (sum, leg) => sum + leg.duration);
  final by = switch (streets.firstOrNull?.mode) {
    'cycling' => 'cycling',
    'wheelchair' => 'by wheelchair',
    _ => 'walking',
  };
  return [
    if (transfers == 1) '1 transfer',
    if (transfers > 1) '$transfers transfers',
    if ((seconds / 60).round() > 0) '${formatDuration(seconds.toDouble())} $by',
  ];
}

/// A transit journey as a row, line for line a street route's ([OptionRow]): when to
/// go, large, and how long it takes; when it leaves and arrives, and what else there
/// is to know of it ([journeyFacts]); its timeline on the axis the list shares; and
/// the other routes that can be taken for its rides, each under its ride. At the
/// roomier text sizes (see [roomyTextScale]) how long it takes has a line of its own
/// and its times and the rest have two.
///
/// It's as tall as every other row of the sheet ([OptionRowMetrics.height]). The
/// selected row has a tonal fill and when to go in the primary color; nothing else
/// about it changes, so selecting one moves nothing.
///
/// Given the same journey as a newer plan has it, its timeline eases to the new
/// times, and to a new [axis]; its times change in place, and the rest fades into
/// what it is now.
///
/// Given a [scrub], its timeline scrubs (see [ScrubGraph]): the readout takes the place
/// of when it leaves and arrives, and what follows that.
class JourneyRow extends StatelessWidget {
  const JourneyRow({
    super.key,
    required this.journey,
    required this.axis,
    required this.now,
    required this.onTap,
    this.selected = false,
    this.metrics,
    this.scrub,
  });

  final Journey journey;

  /// The span of time the list's timelines share.
  final TimelineAxis axis;
  final DateTime now;
  final VoidCallback? onTap;
  final bool selected;

  /// How rows are laid out, if the list has worked it out.
  final OptionRowMetrics? metrics;

  /// What the page does as the row's timeline is scrubbed, or null if it isn't.
  final ScrubSink? scrub;

  /// Between when to go and how long it takes, on the one line, and before the mark
  /// of a stop that has moved.
  static const _gap = 8.0;

  /// How wide the mark of a stop that has moved is.
  static const _warningWidth = 16.0;

  static const _fade = Duration(milliseconds: 200);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final metrics = this.metrics ?? OptionRowMetrics.of(context);
    final offset = journeyUtcOffset(journey);
    final range =
        '${clock(context, journey.departure, utcOffset: offset)}'
        ' – '
        '${clock(context, journey.arrival, utcOffset: offset)}';
    final facts = journeyFacts(journey);
    final moved = journey.legs.whereType<StreetLeg>().any(
      (leg) => leg.stopMoved.isNotEmpty,
    );
    final goIn = goInSpan(theme, journey, now);
    final others = otherRoutes(journey);
    final took = formatDuration(journey.duration.toDouble());
    // One line of its own width, shrunk to fit rather than cut off.
    Widget whole(Widget text, {required double height}) => SizedBox(
      height: height,
      child: FittedBox(
        fit: BoxFit.scaleDown,
        alignment: AlignmentDirectional.centerStart,
        child: text,
      ),
    );
    final headline = AnimatedDefaultTextStyle(
      duration: _fade,
      curve: Curves.easeOut,
      style: metrics.headline.copyWith(
        color: selected ? scheme.primary : scheme.onSurface,
      ),
      child: whole(
        Text.rich(goIn, maxLines: 1, softWrap: false),
        height: metrics.headlineHeight,
      ),
    );
    return RowScrubber(
      track: JourneyScrubTrack(
        journey: journey,
        axis: axis,
        clock: (time) => clock(context, time, utcOffset: offset),
      ),
      sink: scrub,
      onSelect: onTap,
      builder: (context, scrubbing) => Semantics(
        container: true,
        button: true,
        selected: selected,
        // All of it, whatever the row has room to show; its timeline is a slider of
        // its own when it scrubs.
        label: [
          goIn.toPlainText(),
          took,
          range,
          ...facts,
          timelineLabel(timelinePieces(journey), others: others),
          if (moved) 'A stop has moved',
        ].join(', '),
        excludeSemantics: scrubbing == null,
        onTap: onTap,
        child: OptionRowFrame(
          metrics: metrics,
          selected: selected,
          onTap: onTap,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (metrics.roomy) ...[
                ExcludeSemantics(child: headline),
                ExcludeSemantics(
                  child: DefaultTextStyle.merge(
                    style: metrics.arrival,
                    child: whole(
                      Text(took, maxLines: 1, softWrap: false),
                      height: metrics.arrivalHeight,
                    ),
                  ),
                ),
              ] else
                ExcludeSemantics(
                  child: LayoutBuilder(
                    builder: (context, constraints) => Row(
                      crossAxisAlignment: CrossAxisAlignment.baseline,
                      textBaseline: TextBaseline.alphabetic,
                      children: [
                        // All of it, in as much of the line as it needs: it shrinks
                        // only if the whole line is too little.
                        ConstrainedBox(
                          constraints: BoxConstraints(
                            maxWidth: constraints.maxWidth - _gap,
                          ),
                          child: headline,
                        ),
                        const SizedBox(width: _gap),
                        // The one that gives way if the two can't share the line.
                        Expanded(
                          child: Text(
                            took,
                            style: metrics.arrival,
                            textAlign: TextAlign.end,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              const SizedBox(height: OptionRowMetrics.gap),
              ExcludeSemantics(
                child: SizedBox(
                  height: metrics.nameHeight,
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      // What it says last gives way first; a scrub's readout takes
                      // the place of all of it.
                      Expanded(
                        child: ScrubReadout(
                          scrub: scrubbing,
                          style: metrics.name.copyWith(color: scheme.primary),
                          lines: metrics.textLines,
                          // Its times change in place; anything else fades into
                          // what it is now, where it is.
                          child: AnimatedSwitcher(
                            duration: _fade,
                            switchInCurve: Curves.easeOut,
                            switchOutCurve: Curves.easeOut,
                            layoutBuilder: (current, previous) => Stack(
                              fit: StackFit.expand,
                              children: [...previous, ?current],
                            ),
                            child: Text(
                              [range, ...facts].join(' · '),
                              key: ValueKey(facts.join(' · ')),
                              style: metrics.name,
                              maxLines: metrics.textLines,
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                        ),
                      ),
                      // Beside the first line, whatever that has room to say.
                      if (moved) ...[
                        const SizedBox(width: _gap),
                        SizedBox(
                          height: textLinesHeight(context, metrics.name),
                          child: Icon(
                            Icons.wrong_location,
                            size: _warningWidth,
                            color: scheme.error,
                            semanticLabel: 'A stop has moved',
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ),
              // Its zone takes the gap above it and the line under it, to be tall
              // enough to touch.
              ScrubGraph(
                scrub: scrubbing,
                label: 'Along the journey',
                padding: const EdgeInsets.only(top: OptionRowMetrics.gap),
                child: ExcludeSemantics(
                  excluding: scrubbing != null,
                  child: JourneyTimeline(
                    journey: journey,
                    axis: axis,
                    others: others,
                    reserveOthers: true,
                    eases: true,
                    room: routeGraphHeight,
                    othersGap: OptionRowMetrics.gap,
                    othersHeight: textLinesHeight(context, metrics.caption),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A plan's journeys as rows, each in a [SheetSlot] the sheet can hold above its
/// buttons, on one axis of time.
///
/// A row is its journey's for as long as a plan has that journey ([keys]), so a newer
/// plan changes it in place: its times, and its timeline, which eases to where it now
/// is. Rows that a newer plan adds grow and fade in, those it drops shrink and fade
/// out, and those it puts in another order go from where they were and come in where
/// they are; the rest slide to make room. The selected row is never one of those
/// that goes and comes, so a row the sheet holds stays held.
class JourneyRows extends StatefulWidget {
  const JourneyRows({
    super.key,
    required this.journeys,
    required this.keys,
    required this.selected,
    required this.axis,
    required this.now,
    required this.opening,
    required this.reveal,
    required this.onSelect,
    this.scrub,
  }) : assert(journeys.length == keys.length);

  /// The ID of the slot of the row of the journey with [key], for
  /// [SheetContent.held].
  static Object slotId(JourneyKey key) => (#directionsJourney, key);

  /// The plan's journeys, in its order, and which each is ([journeyKeys]).
  final List<Journey> journeys;
  final List<JourneyKey> keys;

  /// The index of the selected one of [journeys].
  final int selected;
  final TimelineAxis axis;
  final DateTime now;

  /// The ID of a [SheetSection] that ends under the last row the sheet first opens to
  /// show, or null to mark none.
  final Object? opening;

  /// Fades the rows in, and out, with the rest of what the page shows around its
  /// content.
  final Animation<double> reveal;

  /// Called with the index in [journeys] of a row that's tapped.
  final ValueChanged<int> onSelect;

  /// What the page does as a row's timeline is scrubbed, or null if they aren't.
  final ScrubSink? scrub;

  /// The slot the sheet holds: the selected journey's.
  Object get held => slotId(keys[selected]);

  @override
  State<JourneyRows> createState() => _JourneyRowsState();
}

/// A row of the list: a journey's, or what's left of one on its way out.
class _Entry {
  _Entry(this.key, this.journey, this.generation, {required this.appears});

  final JourneyKey key;
  Journey journey;

  /// Which of the rows this key has had this is: one that goes and comes back
  /// somewhere else is two rows for a while.
  final int generation;

  /// Whether it grows in, rather than being there from the first.
  final bool appears;
  bool leaving = false;

  /// Whether its slot has passed to the row that took its place further up or down
  /// the list.
  bool moved = false;
  bool selected = false;

  Object get id => (key, generation);
}

class _JourneyRowsState extends State<JourneyRows> {
  late List<_Entry> _entries = [
    for (final (i, key) in widget.keys.indexed)
      _Entry(key, widget.journeys[i], 0, appears: false)
        ..selected = i == widget.selected,
  ];
  int _generation = 0;

  @override
  void didUpdateWidget(JourneyRows oldWidget) {
    super.didUpdateWidget(oldWidget);
    final keys = widget.keys;
    final showing = {
      for (final e in _entries)
        if (!e.leaving) e.key: e,
    };
    // The rows that stay where they are among each other: the selected one, and
    // the most of the others that are in the same order before and after.
    final were = [
      for (final e in _entries)
        if (!e.leaving && keys.contains(e.key)) e.key,
    ];
    final now = [
      for (final key in keys)
        if (showing.containsKey(key)) key,
    ];
    final stays = _inOrder(were, now, keys.elementAtOrNull(widget.selected));
    // Each row going stays after the nearest row before it that stays.
    final after = <JourneyKey?, List<_Entry>>{};
    JourneyKey? kept;
    for (final e in _entries) {
      if (!e.leaving && stays.contains(e.key)) {
        kept = e.key;
        continue;
      }
      e.leaving = true;
      (after[kept] ??= []).add(e);
    }
    _entries = [
      ...?after[null],
      for (final (i, key) in keys.indexed) ...[
        if (stays.contains(key))
          showing[key]!
            ..journey = widget.journeys[i]
            ..selected = i == widget.selected
        else
          _Entry(key, widget.journeys[i], ++_generation, appears: true)
            ..selected = i == widget.selected,
        ...?after[key],
      ],
    ];
    // A row on its way out leaves its slot to the row its journey has now.
    for (final e in _entries) {
      if (e.leaving && keys.contains(e.key)) e.moved = true;
    }
  }

  /// The keys of [were] and [now], the same keys in two orders, that are in the same
  /// order in both, as many as can be with [selected] among them if it's there.
  static Set<JourneyKey> _inOrder(
    List<JourneyKey> were,
    List<JourneyKey> now,
    JourneyKey? selected,
  ) {
    final a = selected == null ? -1 : were.indexOf(selected);
    final b = selected == null ? -1 : now.indexOf(selected);
    if (selected == null || a < 0 || b < 0) {
      return _longestCommon(were, now).toSet();
    }
    return {
      ..._longestCommon(were.sublist(0, a), now.sublist(0, b)),
      selected,
      ..._longestCommon(were.sublist(a + 1), now.sublist(b + 1)),
    };
  }

  /// The longest run of keys, not necessarily next to each other, in the same order
  /// in [a] and [b].
  static List<JourneyKey> _longestCommon(
    List<JourneyKey> a,
    List<JourneyKey> b,
  ) {
    final lengths = List.generate(
      a.length + 1,
      (_) => List.filled(b.length + 1, 0),
    );
    for (var i = a.length - 1; i >= 0; i--) {
      for (var j = b.length - 1; j >= 0; j--) {
        lengths[i][j] = a[i] == b[j]
            ? lengths[i + 1][j + 1] + 1
            : math.max(lengths[i + 1][j], lengths[i][j + 1]);
      }
    }
    final out = <JourneyKey>[];
    var i = 0, j = 0;
    while (i < a.length && j < b.length) {
      if (a[i] == b[j]) {
        out.add(a[i]);
        i++;
        j++;
      } else if (lengths[i + 1][j] >= lengths[i][j + 1]) {
        i++;
      } else {
        j++;
      }
    }
    return out;
  }

  void _gone(_Entry entry) {
    if (!mounted) return;
    setState(() => _entries.remove(entry));
  }

  @override
  Widget build(BuildContext context) {
    final metrics = OptionRowMetrics.of(context);
    final opening = math.min(widget.keys.length, OptionList.shownAtFirst);
    var index = 0;
    final children = <Widget>[];
    for (final entry in _entries) {
      final at = entry.leaving ? null : index++;
      children.add(
        _JourneySlot(
          key: ValueKey(entry.id),
          // One that has moved leaves its slot to the row that took its place.
          id: entry.moved ? null : JourneyRows.slotId(entry.key),
          appears: entry.appears,
          leaving: entry.leaving,
          height: metrics.height,
          reveal: widget.reveal,
          onGone: () => _gone(entry),
          child: JourneyRow(
            journey: entry.journey,
            axis: widget.axis,
            now: widget.now,
            selected: entry.selected && !entry.leaving,
            metrics: metrics,
            onTap: at == null ? null : () => widget.onSelect(at),
            scrub: entry.leaving ? null : widget.scrub,
          ),
        ),
      );
      // Its end is the end of the last row the sheet first opens to show.
      if (widget.opening case final id? when at == opening - 1) {
        children.add(
          SheetSection(
            key: const ValueKey(#opening),
            id: id,
            child: const SizedBox(width: double.infinity),
          ),
        );
      }
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: children,
    );
  }
}

/// A row's place in the list, [height] tall, in the slot [id] if it has one: it grows
/// and fades in if it [appears], and shrinks and fades out once it's [leaving], then
/// calls [onGone]. The fading is within the slot, so it shows on a row the sheet
/// holds too.
class _JourneySlot extends StatefulWidget {
  const _JourneySlot({
    super.key,
    required this.id,
    required this.appears,
    required this.leaving,
    required this.height,
    required this.reveal,
    required this.onGone,
    required this.child,
  });

  final Object? id;
  final bool appears;
  final bool leaving;
  final double height;
  final Animation<double> reveal;
  final VoidCallback onGone;
  final Widget child;

  @override
  State<_JourneySlot> createState() => _JourneySlotState();
}

class _JourneySlotState extends State<_JourneySlot>
    with SingleTickerProviderStateMixin {
  late final _present =
      AnimationController(
        vsync: this,
        duration: rowDuration,
        value: widget.appears ? 0 : 1,
      )..addStatusListener((status) {
        if (status.isDismissed && widget.leaving) widget.onGone();
      });
  late final _curve = CurvedAnimation(parent: _present, curve: rowCurve);

  @override
  void initState() {
    super.initState();
    if (widget.appears) _present.forward();
  }

  @override
  void didUpdateWidget(_JourneySlot oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.leaving && !oldWidget.leaving) _present.reverse();
  }

  @override
  void dispose() {
    _curve.dispose();
    _present.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final row = SizedBox(
      height: widget.height,
      child: FadeTransition(
        opacity: widget.reveal,
        child: FadeTransition(
          opacity: _curve,
          child: IgnorePointer(ignoring: widget.leaving, child: widget.child),
        ),
      ),
    );
    return SizeTransition(
      sizeFactor: _curve,
      alignment: Alignment.topLeft,
      child: SizedBox(
        width: double.infinity,
        child: switch (widget.id) {
          final id? => SheetSlot(id: id, child: row),
          null => row,
        },
      ),
    );
  }
}

/// How live a plan's times are, in a few words.
({String text, bool live}) realtimeSummary(TransitPlan plan) => switch (plan) {
  TransitPlan(feedsReported: 0) => (text: 'Scheduled times', live: false),
  TransitPlan(feedsApplied: 0) => (text: 'Live data out of date', live: false),
  _ when plan.feedsApplied == plan.feedsReported => (
    text: 'Live times',
    live: true,
  ),
  _ => (
    text: 'Live times from ${plan.feedsApplied} of ${plan.feedsReported} feeds',
    live: true,
  ),
};

/// What the control for how stops are reached says of [access].
String accessToStops(TransitAccess access) => switch (access) {
  TransitAccess.walking => 'Walking to stops',
  TransitAccess.cycling => 'Cycling to stops',
  TransitAccess.wheelchair => 'Wheelchair to stops',
};

/// The line above transit's rows: how live [plan]'s times are, and how stops are
/// reached, which opens a menu of the ways; then the times along [axis].
///
/// It's as tall whatever it says: with no plan the status is empty, and with no axis
/// the times are, each keeping its room. At the roomier text sizes (see
/// [roomyTextScale]) the status has a line to itself above the control.
class TransitStrip extends StatelessWidget {
  const TransitStrip({
    super.key,
    required this.plan,
    required this.axis,
    required this.access,
    required this.onAccess,
  });

  /// The plan the rows are of, if there is one.
  final TransitPlan? plan;

  /// The rows' axis, if there are rows.
  final TimelineAxis? axis;
  final TransitAccess access;
  final ValueChanged<TransitAccess> onAccess;

  /// The least the control is tall, to be tapped.
  static const _controlHeight = 36.0;

  /// The most of the line's width the control takes, the rest being the status's.
  static const _controlShare = 0.6;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final metrics = OptionRowMetrics.of(context);
    final style = theme.textTheme.bodySmall!;
    final line = textLinesHeight(context, style);
    final summary = plan == null ? null : realtimeSummary(plan!);
    final color = summary?.live ?? false
        ? liveColor(theme)
        : theme.colorScheme.onSurfaceVariant;
    final status = SizedBox(
      height: line,
      child: summary == null
          ? null
          : Row(
              children: [
                Icon(
                  summary.live ? Icons.sensors : Icons.schedule,
                  size: 16,
                  color: color,
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    summary.text,
                    style: style.copyWith(color: color),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
    );
    final control = _AccessControl(
      access: access,
      onChanged: onAccess,
      height: math.max(_controlHeight, line + 12),
    );
    final inset = metrics.margin + metrics.padding.left;
    return Padding(
      padding: EdgeInsets.fromLTRB(inset, 0, inset, 2),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (metrics.roomy) ...[
            status,
            Align(alignment: AlignmentDirectional.centerStart, child: control),
          ] else
            // The control at the end of the line, as wide as it is and no wider
            // than most of the line, and the status in what's left.
            LayoutBuilder(
              builder: (context, constraints) => Row(
                children: [
                  Expanded(child: status),
                  const SizedBox(width: 8),
                  ConstrainedBox(
                    constraints: BoxConstraints(
                      maxWidth: constraints.maxWidth * _controlShare,
                    ),
                    child: control,
                  ),
                ],
              ),
            ),
          SizedBox(
            height: TimelineTicks.heightOf(context),
            child: switch (axis) {
              final axis? => TimelineTicks(axis: axis),
              null => null,
            },
          ),
        ],
      ),
    );
  }
}

/// How stops are reached, as a few words that open a menu of the ways: the platform's.
class _AccessControl extends StatelessWidget {
  const _AccessControl({
    required this.access,
    required this.onChanged,
    required this.height,
  });

  final TransitAccess access;
  final ValueChanged<TransitAccess> onChanged;
  final double height;

  /// How far across its box the drop-down arrow's glyph reaches: it's drawn from 7
  /// to 17 of the icon's 24 units.
  static const _arrowInk = 17 / 24;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final primary = theme.colorScheme.primary;
    // One line, shrunk to fit rather than cut off.
    final label = SizedBox(
      height: height,
      child: FittedBox(
        fit: BoxFit.scaleDown,
        alignment: AlignmentDirectional.centerEnd,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              accessToStops(access),
              style: theme.textTheme.labelLarge!.copyWith(color: primary),
              maxLines: 1,
              softWrap: false,
            ),
            // The arrow's glyph ends short of its box, so the box is cut to where
            // the arrow ends: the control then ends flush with what's under it.
            Align(
              alignment: AlignmentDirectional.centerStart,
              widthFactor: _arrowInk,
              child: Icon(Icons.arrow_drop_down, size: 20, color: primary),
            ),
          ],
        ),
      ),
    );
    void choose(TransitAccess option) {
      if (option != access) onChanged(option);
    }

    if (isCupertinoOf(context)) {
      return CupertinoMenuAnchor(
        useRootOverlay: true,
        menuChildren: [
          for (final option in TransitAccess.values)
            CupertinoMenuItem(
              trailing: option == access
                  ? const Icon(CupertinoIcons.checkmark)
                  : null,
              onPressed: () => choose(option),
              child: Text(option.label),
            ),
        ],
        builder: (context, controller, child) => Semantics(
          button: true,
          child: CupertinoButton(
            padding: EdgeInsets.zero,
            minimumSize: Size(0, height),
            onPressed: () =>
                controller.isOpen ? controller.close() : controller.open(),
            child: label,
          ),
        ),
      );
    }
    return PopupMenuButton<TransitAccess>(
      tooltip: 'How to get to stops',
      initialValue: access,
      onSelected: choose,
      itemBuilder: (context) => [
        for (final option in TransitAccess.values)
          PopupMenuItem(
            value: option,
            child: Row(
              children: [
                Icon(accessIcon(option), size: 20),
                const SizedBox(width: 12),
                Flexible(child: Text(option.label)),
              ],
            ),
          ),
      ],
      child: label,
    );
  }
}
