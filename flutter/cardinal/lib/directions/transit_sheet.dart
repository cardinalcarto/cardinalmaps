/// What the directions sheet shows for transit: how to get to stops, the journeys of the
/// next hour as cards, and one journey's legs in detail.
///
/// The layout follows Murena Maps' transit screens, with less padding.
library;

import 'package:material_ui/material_ui.dart';

import 'format.dart';
import 'transit_plan.dart';

/// Chips for how to get to, from, and between stops.
class TransitAccessRow extends StatelessWidget {
  const TransitAccessRow({
    super.key,
    required this.access,
    required this.onChanged,
  });

  final TransitAccess access;
  final ValueChanged<TransitAccess> onChanged;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(12, 0, 12, 4),
    child: Wrap(
      spacing: 8,
      children: [
        for (final option in TransitAccess.values)
          ChoiceChip(
            avatar: Icon(accessIcon(option), size: 18),
            label: Text(option.label),
            selected: option == access,
            showCheckmark: false,
            visualDensity: VisualDensity.compact,
            onSelected: (_) => onChanged(option),
          ),
      ],
    ),
  );
}

IconData accessIcon(TransitAccess access) => switch (access) {
  TransitAccess.walking => Icons.directions_walk,
  TransitAccess.cycling => Icons.directions_bike,
  TransitAccess.wheelchair => Icons.accessible,
};

IconData _streetIcon(String mode) => switch (mode) {
  'cycling' => Icons.directions_bike,
  'wheelchair' => Icons.accessible,
  _ => Icons.directions_walk,
};

String _streetVerb(String mode) => switch (mode) {
  'cycling' => 'Bike',
  'wheelchair' => 'Go',
  _ => 'Walk',
};

IconData vehicleIcon(VehicleKind kind) => switch (kind) {
  VehicleKind.bus => Icons.directions_bus,
  VehicleKind.tram => Icons.tram,
  VehicleKind.subway => Icons.subway,
  VehicleKind.rail => Icons.train,
  VehicleKind.ferry => Icons.directions_boat,
  VehicleKind.cableway => Icons.airline_seat_recline_normal,
  VehicleKind.other => Icons.directions_transit,
};

/// Whether the plan's times are live, and from how many feeds.
class RealtimeLine extends StatelessWidget {
  const RealtimeLine({super.key, required this.plan});

  final TransitPlan plan;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final live = plan.feedsApplied > 0;
    final text = switch (plan) {
      TransitPlan(feedsReported: 0) => 'Scheduled times: no live data',
      TransitPlan(feedsApplied: 0) =>
        'Scheduled times: live data is out of date',
      _ =>
        'Live times from ${plan.feedsApplied} of ${plan.feedsReported} '
            'real-time feeds',
    };
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 2, 16, 4),
      child: Row(
        children: [
          Icon(
            live ? Icons.sensors : Icons.schedule,
            size: 16,
            color: live
                ? _liveColor(theme)
                : theme.colorScheme.onSurfaceVariant,
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              text,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// What to say when a plan has no journeys.
String noJourneysMessage(TransitPlan plan, TransitAccess access) {
  final getting = switch (access) {
    TransitAccess.walking => 'walking',
    TransitAccess.cycling => 'cycling',
    TransitAccess.wheelchair => 'wheelchair',
  };
  return switch (plan.noJourneysReason) {
    NoJourneysReason.noAccess => 'No stops within $getting range of the start',
    NoJourneysReason.noEgress =>
      'No stops within $getting range of the destination',
    NoJourneysReason.slowerThanDirect =>
      plan.directSeconds != null
          ? 'Going there directly is faster '
                '(${formatDuration(plan.directSeconds!.toDouble())})'
          : 'Going there directly is faster',
    _ => 'No transit leaves in the next hour for this trip',
  };
}

Color _liveColor(ThemeData theme) => theme.brightness == Brightness.dark
    ? const Color(0xff7fd99a)
    : const Color(0xff1b7a3a);

/// A clock time as the rider's settings show it, at a stop's UTC offset if it's known.
String clock(BuildContext context, DateTime time, {int? utcOffset}) {
  final local = utcOffset == null
      ? time.toLocal()
      : time.toUtc().add(Duration(seconds: utcOffset));
  return MaterialLocalizations.of(context).formatTimeOfDay(
    TimeOfDay(hour: local.hour, minute: local.minute),
    alwaysUse24HourFormat: MediaQuery.alwaysUse24HourFormatOf(context),
  );
}

/// The offset of the first stop the journey visits, for its times.
int? _journeyOffset(Journey journey) =>
    journey.rides.firstOrNull?.boarding.utcOffset;

/// How a ride's departure stands against its schedule.
({String text, Color color}) rideStatus(ThemeData theme, TransitLeg ride) {
  if (!ride.realtime) {
    return (text: 'Scheduled', color: theme.colorScheme.onSurfaceVariant);
  }
  if (ride.tripState == 'added' || ride.tripState == 'replaced') {
    return (text: 'Live, changed trip', color: _liveColor(theme));
  }
  final delay = ride.boarding.departureDelay ?? ride.delaySeconds ?? 0;
  final minutes = (delay.abs() / 60).round();
  if (minutes == 0) return (text: 'On time', color: _liveColor(theme));
  if (delay > 0) {
    return (text: '$minutes min late', color: theme.colorScheme.error);
  }
  return (text: '$minutes min early', color: _liveColor(theme));
}

Color? _parseColor(String? rrggbb) {
  if (rrggbb == null || !RegExp(r'^[0-9a-fA-F]{6}$').hasMatch(rrggbb)) {
    return null;
  }
  return Color(0xff000000 | int.parse(rrggbb, radix: 16));
}

/// A ride's name: its route, and where it's headed.
String rideTitle(TransitLeg ride) {
  final route = ride.route?.name ?? '';
  final name = route.isEmpty
      ? (ride.route?.kind ?? VehicleKind.other).label
      : route;
  final headsign = ride.headsign;
  return headsign == null ? name : '$name to $headsign';
}

/// A stay-seated ride's later trips: what it continues as, and where.
List<({String stop, String title})> continuations(TransitLeg ride) => [
  for (final segment in ride.segments.skip(1))
    (
      stop: ride
          .stops[segment.firstStop.clamp(0, ride.stops.length - 1)]
          .stop
          .name,
      title: [
        segment.route?.name ?? '',
        if (segment.headsign != null) 'to ${segment.headsign}',
      ].where((part) => part.isNotEmpty).join(' '),
    ),
];

/// A leg's icon in a circle, in its route's colors.
class _LegIcon extends StatelessWidget {
  const _LegIcon({required this.leg, this.size = 28});

  final JourneyLeg leg;
  final double size;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (icon, background, foreground) = switch (leg) {
      TransitLeg(:final route) => (
        vehicleIcon(route?.kind ?? VehicleKind.other),
        _parseColor(route?.color) ?? scheme.primary,
        _parseColor(route?.textColor) ?? scheme.onPrimary,
      ),
      StreetLeg(:final mode) => (
        _streetIcon(mode),
        scheme.secondaryContainer,
        scheme.onSecondaryContainer,
      ),
    };
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        color: background,
        shape: BoxShape.circle,
        border: Border.all(color: scheme.outlineVariant, width: 0.5),
      ),
      child: Icon(icon, size: size * 0.6, color: foreground),
    );
  }
}

/// A moved stop, beside the walk to or from it: the agency only says where it went in
/// words.
class StopMovedBanner extends StatelessWidget {
  const StopMovedBanner({super.key, required this.notice, this.full = true});

  final StopMovedNotice notice;

  /// Whether to show the alert's whole description, not just its header.
  final bool full;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final name = notice.stopName ?? 'This stop';
    final header = notice.header;
    final description = notice.description;
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
      decoration: BoxDecoration(
        color: scheme.errorContainer,
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.wrong_location, size: 20, color: scheme.onErrorContainer),
          const SizedBox(width: 8),
          Expanded(
            child: DefaultTextStyle.merge(
              style: TextStyle(color: scheme.onErrorContainer),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    '$name has moved',
                    style: theme.textTheme.labelLarge?.copyWith(
                      color: scheme.onErrorContainer,
                    ),
                  ),
                  if (header != null)
                    Text(header, style: theme.textTheme.bodySmall),
                  if (full && description != null && description != header)
                    Padding(
                      padding: const EdgeInsets.only(top: 2),
                      child: Text(
                        description,
                        style: theme.textTheme.bodySmall,
                      ),
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A journey in the list: when it leaves and arrives, how long it takes, and its legs.
class JourneyCard extends StatelessWidget {
  const JourneyCard({
    super.key,
    required this.journey,
    required this.miles,
    required this.selected,
    required this.onTap,
    required this.wheelchair,
  });

  final Journey journey;
  final bool miles;
  final bool selected;
  final VoidCallback onTap;

  /// Whether to say what's known about each ride's accessibility.
  final bool wheelchair;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final offset = _journeyOffset(journey);
    final minutes = journey.firstBoarding?.difference(DateTime.now()).inMinutes;
    return Card.filled(
      margin: const EdgeInsets.symmetric(vertical: 4),
      clipBehavior: Clip.antiAlias,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(12),
        side: selected
            ? BorderSide(color: scheme.primary, width: 1.5)
            : BorderSide.none,
      ),
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 8),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '${clock(context, journey.departure, utcOffset: offset)}'
                          ' – '
                          '${clock(context, journey.arrival, utcOffset: offset)}',
                          style: theme.textTheme.titleMedium,
                        ),
                        Text(
                          [
                            if (minutes != null)
                              minutes <= 0
                                  ? 'Board now'
                                  : 'Board in ${formatDuration(minutes * 60.0)}',
                            journey.transfers == 1
                                ? '1 transfer'
                                : '${journey.transfers} transfers',
                          ].join(' · '),
                          style: theme.textTheme.bodySmall?.copyWith(
                            color: scheme.onSurfaceVariant,
                          ),
                        ),
                      ],
                    ),
                  ),
                  Text(
                    formatDuration(journey.duration.toDouble()),
                    style: theme.textTheme.titleLarge?.copyWith(
                      color: scheme.primary,
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 6),
              for (final (i, leg) in journey.legs.indexed)
                _TimelineRow(
                  leg: leg,
                  last: i == journey.legs.length - 1,
                  child: switch (leg) {
                    StreetLeg() => _StreetSummary(leg: leg, miles: miles),
                    TransitLeg() => _RideSummary(
                      ride: leg,
                      wheelchair: wheelchair,
                    ),
                  },
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A leg's icon, a line down to the next one, and what's said about it.
class _TimelineRow extends StatelessWidget {
  const _TimelineRow({
    required this.leg,
    required this.last,
    required this.child,
  });

  final JourneyLeg leg;
  final bool last;
  final Widget child;

  @override
  Widget build(BuildContext context) => IntrinsicHeight(
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SizedBox(
          width: 28,
          child: Column(
            children: [
              _LegIcon(leg: leg),
              if (!last)
                Expanded(
                  child: Container(
                    width: 2,
                    margin: const EdgeInsets.symmetric(vertical: 2),
                    color: Theme.of(context).colorScheme.outlineVariant,
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(width: 10),
        Expanded(
          child: Padding(
            padding: EdgeInsets.only(bottom: last ? 0 : 6),
            child: child,
          ),
        ),
      ],
    ),
  );
}

class _StreetSummary extends StatelessWidget {
  const _StreetSummary({required this.leg, required this.miles});

  final StreetLeg leg;
  final bool miles;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final distance = leg.walk?.distance;
    final to = leg.to?.name;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _TitleRow(
          title: _streetVerb(leg.mode),
          duration: leg.duration,
          style: theme.textTheme.bodyLarge,
        ),
        Text(
          [
            if (distance != null) formatDistance(distance, miles: miles),
            if (to != null) 'to $to',
          ].join(' '),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: theme.textTheme.bodySmall?.copyWith(
            color: theme.colorScheme.onSurfaceVariant,
          ),
        ),
        for (final notice in leg.stopMoved)
          StopMovedBanner(notice: notice, full: false),
      ],
    );
  }
}

class _RideSummary extends StatelessWidget {
  const _RideSummary({required this.ride, required this.wheelchair});

  final TransitLeg ride;
  final bool wheelchair;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final status = rideStatus(theme, ride);
    final boarding = ride.boarding;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _TitleRow(
          title: rideTitle(ride),
          duration: ride.duration,
          style: theme.textTheme.bodyLarge,
        ),
        Text.rich(
          TextSpan(
            children: [
              TextSpan(
                text:
                    '${clock(context, boarding.departure, utcOffset: boarding.utcOffset)}'
                    ' from ${boarding.stop.name} · ',
              ),
              TextSpan(
                text: status.text,
                style: TextStyle(color: status.color),
              ),
            ],
          ),
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: muted,
        ),
        for (final next in continuations(ride))
          _IconLine(
            icon: Icons.airline_seat_recline_normal,
            text: 'Stay on at ${next.stop} as ${next.title}',
            style: muted,
          ),
        Text(
          [
            if (ride.agency != null) ride.agency!,
            ride.stopsBetween + 1 == 1
                ? '1 stop'
                : '${ride.stopsBetween + 1} stops',
          ].join(' · '),
          style: muted,
        ),
        if (wheelchair || ride.accessibility == Accessibility.inaccessible)
          _AccessibilityLine(accessibility: ride.accessibility),
        for (final alert in ride.alerts.where((a) => a.effect != 'stop_moved'))
          if (alert.header case final header?)
            _IconLine(
              icon: Icons.warning_amber,
              text: header,
              style: muted?.copyWith(color: theme.colorScheme.error),
              color: theme.colorScheme.error,
              maxLines: 2,
            ),
      ],
    );
  }
}

class _TitleRow extends StatelessWidget {
  const _TitleRow({
    required this.title,
    required this.duration,
    required this.style,
  });

  final String title;
  final int duration;
  final TextStyle? style;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(
          child: Text(
            title,
            style: style,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
          ),
        ),
        const SizedBox(width: 8),
        Text(
          formatDuration(duration.toDouble()),
          style: theme.textTheme.bodyMedium?.copyWith(
            color: theme.colorScheme.primary,
          ),
        ),
      ],
    );
  }
}

class _IconLine extends StatelessWidget {
  const _IconLine({
    required this.icon,
    required this.text,
    required this.style,
    this.color,
    this.maxLines = 1,
  });

  final IconData icon;
  final String text;
  final TextStyle? style;
  final Color? color;
  final int maxLines;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(top: 1),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(top: 1, right: 4),
          child: Icon(
            icon,
            size: 14,
            color: color ?? Theme.of(context).colorScheme.onSurfaceVariant,
          ),
        ),
        Expanded(
          child: Text(
            text,
            style: style,
            maxLines: maxLines,
            overflow: TextOverflow.ellipsis,
          ),
        ),
      ],
    ),
  );
}

class _AccessibilityLine extends StatelessWidget {
  const _AccessibilityLine({required this.accessibility});

  final Accessibility accessibility;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final (icon, text, color) = switch (accessibility) {
      Accessibility.accessible => (
        Icons.accessible,
        'Wheelchair accessible',
        _liveColor(theme),
      ),
      Accessibility.inaccessible => (
        Icons.not_accessible,
        'Not wheelchair accessible',
        theme.colorScheme.error,
      ),
      Accessibility.unknown => (
        Icons.accessible,
        'Wheelchair access unknown',
        theme.colorScheme.onSurfaceVariant,
      ),
    };
    return _IconLine(
      icon: icon,
      text: text,
      color: color,
      style: theme.textTheme.bodySmall?.copyWith(color: color),
    );
  }
}

/// A journey's times, transfers, and time on foot, above its legs.
class JourneySummary extends StatelessWidget {
  const JourneySummary({super.key, required this.journey, required this.miles});

  final Journey journey;
  final bool miles;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final offset = _journeyOffset(journey);
    final muted = theme.textTheme.bodyMedium?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    return Card.filled(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'Depart ${clock(context, journey.departure, utcOffset: offset)}',
                    style: theme.textTheme.bodyLarge,
                  ),
                  Text(
                    'Arrive ${clock(context, journey.arrival, utcOffset: offset)}',
                    style: theme.textTheme.bodyLarge,
                  ),
                  if (journey.streetMeters > 0)
                    Text(
                      '${formatDistance(journey.streetMeters, miles: miles)} '
                      '${_streetDistanceWord(journey)}',
                      style: muted,
                    ),
                ],
              ),
            ),
            Column(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                Text(
                  formatDuration(journey.duration.toDouble()),
                  style: theme.textTheme.headlineSmall?.copyWith(
                    color: theme.colorScheme.primary,
                  ),
                ),
                Text(
                  journey.transfers == 1
                      ? '1 transfer'
                      : '${journey.transfers} transfers',
                  style: muted,
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// How a journey's distance off vehicles is covered, after its distance.
String _streetDistanceWord(Journey journey) =>
    switch (journey.legs.whereType<StreetLeg>().firstOrNull?.mode) {
      'cycling' => 'by bike',
      'wheelchair' => 'by wheelchair',
      _ => 'on foot',
    };

/// One leg of a journey in detail.
class LegCard extends StatelessWidget {
  const LegCard({
    super.key,
    required this.leg,
    required this.miles,
    required this.cycling,
    required this.start,
    required this.destination,
  });

  final JourneyLeg leg;
  final bool miles;

  /// Whether the rider has a bike, so whether bikes are allowed matters.
  final bool cycling;

  /// What the trip's ends are called, for the first and last legs.
  final String start;
  final String destination;

  @override
  Widget build(BuildContext context) => Card.filled(
    margin: const EdgeInsets.symmetric(vertical: 4),
    clipBehavior: Clip.antiAlias,
    child: Padding(
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
      child: switch (leg) {
        final StreetLeg leg => _StreetDetail(
          leg: leg,
          miles: miles,
          start: start,
          destination: destination,
        ),
        final TransitLeg ride => _RideDetail(ride: ride, cycling: cycling),
      },
    ),
  );
}

class _LegHeader extends StatelessWidget {
  const _LegHeader({required this.leg, required this.title, this.subtitle});

  final JourneyLeg leg;
  final String title;
  final String? subtitle;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Row(
      children: [
        _LegIcon(leg: leg, size: 36),
        const SizedBox(width: 10),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(title, style: theme.textTheme.titleMedium),
              if (subtitle != null)
                Text(
                  subtitle!,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
            ],
          ),
        ),
        Text(
          formatDuration(leg.duration.toDouble()),
          style: theme.textTheme.titleMedium?.copyWith(
            color: theme.colorScheme.primary,
          ),
        ),
      ],
    );
  }
}

/// From where, at what time, to where, at what time.
class _FromTo extends StatelessWidget {
  const _FromTo({
    required this.from,
    required this.fromTime,
    required this.to,
    required this.toTime,
  });

  final String from;
  final String fromTime;
  final String to;
  final String toTime;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final label = theme.textTheme.labelSmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final time = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    Widget end(String what, String name, String at, CrossAxisAlignment align) =>
        Expanded(
          child: Column(
            crossAxisAlignment: align,
            children: [
              Text(what, style: label),
              Text(
                name,
                style: theme.textTheme.bodyMedium,
                textAlign: align == CrossAxisAlignment.end
                    ? TextAlign.end
                    : TextAlign.start,
              ),
              Text(at, style: time),
            ],
          ),
        );
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          end('From', from, fromTime, CrossAxisAlignment.start),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 12),
            child: Icon(
              Icons.arrow_forward,
              size: 18,
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          end('To', to, toTime, CrossAxisAlignment.end),
        ],
      ),
    );
  }
}

class _StreetDetail extends StatelessWidget {
  const _StreetDetail({
    required this.leg,
    required this.miles,
    required this.start,
    required this.destination,
  });

  final StreetLeg leg;
  final bool miles;
  final String start;
  final String destination;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final walk = leg.walk;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _LegHeader(
          leg: leg,
          title: _streetVerb(leg.mode),
          subtitle: walk == null
              ? null
              : formatDistance(walk.distance, miles: miles),
        ),
        for (final notice in leg.stopMoved) StopMovedBanner(notice: notice),
        _FromTo(
          from: leg.from?.name ?? start,
          fromTime: 'Leave ${clock(context, leg.departure)}',
          to: leg.to?.name ?? destination,
          toTime: 'Arrive ${clock(context, leg.arrival)}',
        ),
        if (leg.mode == 'wheelchair' &&
            leg.accessibility != Accessibility.unknown)
          _AccessibilityLine(accessibility: leg.accessibility),
        if (walk != null && walk.maneuvers.isNotEmpty)
          Theme(
            data: theme.copyWith(dividerColor: Colors.transparent),
            child: ExpansionTile(
              tilePadding: EdgeInsets.zero,
              dense: true,
              visualDensity: VisualDensity.compact,
              title: Text(
                walk.maneuvers.length == 1
                    ? '1 step'
                    : '${walk.maneuvers.length} steps',
                style: theme.textTheme.bodyMedium,
              ),
              children: [
                for (final m in walk.maneuvers)
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 3),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Expanded(
                          child: Text(
                            m.instruction,
                            style: theme.textTheme.bodySmall,
                          ),
                        ),
                        if (m.distance > 0)
                          Text(
                            formatDistance(m.distance, miles: miles),
                            style: theme.textTheme.bodySmall?.copyWith(
                              color: theme.colorScheme.onSurfaceVariant,
                            ),
                          ),
                      ],
                    ),
                  ),
              ],
            ),
          ),
      ],
    );
  }
}

class _RideDetail extends StatelessWidget {
  const _RideDetail({required this.ride, required this.cycling});

  final TransitLeg ride;
  final bool cycling;

  String _visitTime(
    BuildContext context,
    StopVisit visit,
    DateTime actual,
    DateTime? scheduled,
  ) {
    final now = clock(context, actual, utcOffset: visit.utcOffset);
    if (scheduled == null ||
        (actual.difference(scheduled).inSeconds).abs() < 60) {
      return now;
    }
    return '$now (scheduled ${clock(context, scheduled, utcOffset: visit.utcOffset)})';
  }

  static String _stopName(StopRef stop) => [
    stop.name,
    if (stop.platform != null) '(platform ${stop.platform})',
  ].join(' ');

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final status = rideStatus(theme, ride);
    final boarding = ride.boarding, alighting = ride.alighting;
    final vehicle = ride.vehicle;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _LegHeader(
          leg: ride,
          title: rideTitle(ride),
          subtitle: [
            if (ride.agency != null) ride.agency!,
            (ride.route?.kind ?? VehicleKind.other).label,
          ].join(' · '),
        ),
        _FromTo(
          from: _stopName(boarding.stop),
          fromTime:
              'Depart ${_visitTime(context, boarding, boarding.departure, boarding.scheduledDeparture)}',
          to: _stopName(alighting.stop),
          toTime:
              'Arrive ${_visitTime(context, alighting, alighting.arrival, alighting.scheduledArrival)}',
        ),
        const SizedBox(height: 4),
        _IconLine(
          icon: ride.realtime ? Icons.sensors : Icons.schedule,
          text: ride.realtime ? 'Live: ${status.text}' : 'Scheduled time',
          color: status.color,
          style: theme.textTheme.bodySmall?.copyWith(color: status.color),
        ),
        if (ride.waitSeconds >= 60)
          _IconLine(
            icon: Icons.hourglass_empty,
            text:
                'Wait ${formatDuration(ride.waitSeconds.toDouble())} to board',
            style: muted,
          ),
        for (final next in continuations(ride))
          _IconLine(
            icon: Icons.airline_seat_recline_normal,
            text:
                'Stay on at ${next.stop}: the vehicle continues as ${next.title}',
            style: muted,
            maxLines: 3,
          ),
        _AccessibilityLine(accessibility: ride.accessibility),
        if (cycling && ride.bikesAllowed == 'yes')
          _IconLine(
            icon: Icons.pedal_bike,
            text: 'Bikes allowed',
            style: muted,
          ),
        if (vehicle != null &&
            (vehicle.label != null || vehicle.occupancy != null))
          _IconLine(
            icon: Icons.directions_bus_filled_outlined,
            text: [
              if (vehicle.label != null) 'Vehicle ${vehicle.label}',
              if (vehicle.occupancy != null) _occupancy(vehicle.occupancy!),
            ].join(' · '),
            style: muted,
          ),
        for (final alert in ride.alerts) _AlertTile(alert: alert),
        Theme(
          data: theme.copyWith(dividerColor: Colors.transparent),
          child: ExpansionTile(
            tilePadding: EdgeInsets.zero,
            dense: true,
            visualDensity: VisualDensity.compact,
            title: Text(
              ride.stopsBetween + 1 == 1
                  ? '1 stop'
                  : '${ride.stopsBetween + 1} stops',
              style: theme.textTheme.bodyMedium,
            ),
            children: [
              for (final visit in ride.stops)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 2),
                  child: Row(
                    children: [
                      SizedBox(
                        width: 64,
                        child: Text(
                          clock(
                            context,
                            visit.departure,
                            utcOffset: visit.utcOffset,
                          ),
                          style: muted,
                        ),
                      ),
                      Expanded(
                        child: Text(
                          visit.stop.name,
                          style: theme.textTheme.bodySmall?.copyWith(
                            decoration: visit.skipped
                                ? TextDecoration.lineThrough
                                : null,
                          ),
                        ),
                      ),
                      if (visit.stop.accessibility ==
                          Accessibility.inaccessible)
                        Icon(
                          Icons.not_accessible,
                          size: 14,
                          color: theme.colorScheme.error,
                        ),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }

  static String _occupancy(String status) => switch (status) {
    'empty' => 'Empty',
    'many_seats_available' => 'Many seats',
    'few_seats_available' => 'Few seats',
    'standing_room_only' => 'Standing room only',
    'crushed_standing_room_only' => 'Very full',
    'full' => 'Full',
    'not_accepting_passengers' => 'Not taking riders',
    _ => status.replaceAll('_', ' '),
  };
}

class _AlertTile extends StatelessWidget {
  const _AlertTile({required this.alert});

  final LegAlert alert;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final header = alert.header ?? alert.effect?.replaceAll('_', ' ');
    if (header == null) return const SizedBox.shrink();
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.warning_amber, size: 18, color: scheme.error),
          const SizedBox(width: 6),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(header, style: theme.textTheme.labelLarge),
                if (alert.description case final d? when d != header)
                  Text(d, style: theme.textTheme.bodySmall),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
